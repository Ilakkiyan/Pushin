use crate::model::{Block, ScheduleResult, Settings};
use crate::scheduler::{self, Interval};
use crate::{db, habits, scheduler::SchedulePref};
use anyhow::Result;
use chrono::{Duration, Local, NaiveDateTime, Timelike};
use rusqlite::Connection;
use std::collections::HashSet;

/// Awake window habit occurrences are re-flowed into (mirrors `commands::HABIT_DAY_*`).
const HABIT_DAY_START_H: u32 = 7;
const HABIT_DAY_END_H: u32 = 22;

/// Move habit occurrences that collide with a real (non-habit) event to a free slot on their day. When
/// the user drops a fixed event (a meeting, "watch the game") on top of where a habit already sits, the
/// habit should step aside rather than overlap it. Future occurrences only — never disturbs the past or
/// one already in progress; if the day has no room, the habit is left where it is. Runs before task
/// scheduling so tasks then plan around the moved habit.
fn resolve_habit_conflicts(conn: &Connection, now: NaiveDateTime) -> Result<()> {
    let events = db::list_events(conn)?;
    let blocks = db::list_blocks(conn)?;

    // `occupied` = everything a movable habit must avoid. Seed it with the immovable stuff — real
    // (non-habit) events and task blocks — then grow it with each habit as its position is finalized,
    // so habits also never overlap EACH OTHER (an earlier-starting habit wins; a later one steps aside).
    let mut occupied: Vec<Interval> = Vec::new();
    for e in events.iter().filter(|e| e.kind != "habit") {
        if let (Some(s), Some(en)) = (scheduler::parse_dt(&e.start), scheduler::parse_dt(&e.end)) {
            occupied.push(Interval { start: s, end: en });
        }
    }
    for b in &blocks {
        if let (Some(s), Some(en)) = (scheduler::parse_dt(&b.start), scheduler::parse_dt(&b.end)) {
            occupied.push(Interval { start: s, end: en });
        }
    }

    // Habit occurrences, earliest first (ISO timestamps sort chronologically).
    let mut habit_evs: Vec<&crate::model::Event> = events.iter().filter(|e| e.kind == "habit").collect();
    habit_evs.sort_by(|a, b| a.start.cmp(&b.start));

    for ev in habit_evs {
        let (hs, he) = match (scheduler::parse_dt(&ev.start), scheduler::parse_dt(&ev.end)) {
            (Some(s), Some(e)) => (s, e),
            _ => continue,
        };
        // A past/in-progress occurrence can't move — it just occupies its slot for the ones after it.
        if he <= now {
            occupied.push(Interval { start: hs, end: he });
            continue;
        }
        if !occupied.iter().any(|o| o.start < he && hs < o.end) {
            occupied.push(Interval { start: hs, end: he }); // no collision — keep it, and reserve its slot
            continue;
        }

        // Collides with a fixed event or an already-placed habit → re-flow into a free gap on its day.
        let day = hs.date();
        let dur = (he - hs).num_minutes().max(1);
        let day_lo = day.and_hms_opt(0, 0, 0).unwrap();
        let day_hi = day.and_hms_opt(23, 59, 59).unwrap();
        let busy: Vec<Interval> = occupied.iter().filter(|o| o.end > day_lo && o.start < day_hi).copied().collect();

        // Awake window for the day; never re-place into the past when it's today.
        let mut window_start = day.and_hms_opt(HABIT_DAY_START_H, 0, 0).unwrap();
        let window_end = day.and_hms_opt(HABIT_DAY_END_H, 0, 0).unwrap();
        if day == now.date() {
            let rounded = ((now.hour() as i64 * 60 + now.minute() as i64) + 14) / 15 * 15;
            let candidate = day_lo + Duration::minutes(rounded);
            if candidate > window_start {
                window_start = candidate.min(window_end);
            }
        }

        if let Some((ns, ne)) = habits::find_habit_slot(&busy, window_start, window_end, dur) {
            // find_habit_slot's packed-day fallback ignores `busy`, so only move when the new slot is
            // genuinely clear — otherwise leave the habit put (and reserve its original slot).
            if !occupied.iter().any(|o| o.start < ne && ns < o.end) {
                db::update_event(conn, ev.id, &ev.title, &scheduler::fmt_dt(ns), &scheduler::fmt_dt(ne))?;
                occupied.push(Interval { start: ns, end: ne });
                continue;
            }
        }
        occupied.push(Interval { start: hs, end: he });
    }
    Ok(())
}

/// Recompute the schedule from the current DB state and persist the new blocks.
pub fn reschedule_inner(conn: &mut Connection, settings: &Settings) -> Result<ScheduleResult> {
    let mut tasks = db::list_tasks(conn)?;
    // Adaptive estimate: bias not-done task durations by what completed tasks ACTUALLY took
    // (focus-tracked). A soft input — `estimation_factor` is 1.0 (no change) until there's history,
    // so the scheduler stays deterministic and its tests are unaffected. Stored estimates are not
    // mutated; only this scheduling pass is rescaled.
    let factor = scheduler::estimation_factor(&db::estimation_samples(conn).unwrap_or_default());
    if (factor - 1.0).abs() > 1e-6 {
        for t in &mut tasks {
            if t.status != "done" && t.status != "in_progress" {
                t.estimated_minutes = ((t.estimated_minutes as f64 * factor).round() as i64).max(15);
            }
        }
    }
    let now = Local::now().naive_local();

    // Re-flow habits off any real event they now overlap (e.g. a just-added meeting landed on a habit)
    // BEFORE planning tasks, so both habits and tasks end up clear of fixed events.
    resolve_habit_conflicts(conn, now)?;

    let events = db::list_events(conn)?;
    let blocks = db::list_blocks(conn)?;

    let fixed: Vec<Interval> = events
        .iter()
        .filter_map(|e| match (scheduler::parse_dt(&e.start), scheduler::parse_dt(&e.end)) {
            (Some(s), Some(en)) => Some(Interval { start: s, end: en }),
            _ => None,
        })
        .collect();

    let locked: Vec<(i64, Interval)> = blocks
        .iter()
        .filter(|b| b.locked)
        .filter_map(|b| match (scheduler::parse_dt(&b.start), scheduler::parse_dt(&b.end)) {
            (Some(s), Some(en)) => Some((b.task_id, Interval { start: s, end: en })),
            _ => None,
        })
        .collect();

    // **Stability.** Instead of re-packing the whole calendar every time a task is added/changed (which
    // makes existing scheduled tasks jump around), keep existing UNLOCKED future blocks where they are:
    // hand them to the scheduler as extra "locked" intervals for this pass — so it plans new work AROUND
    // them and still honours dependency timing (locked ends feed the DAG) — then re-emit them as unlocked
    // blocks. A block that now collides with a fixed event or a real locked block, or whose task is gone/
    // done, is dropped so that task reschedules cleanly.
    let active_ids: HashSet<i64> = tasks.iter().filter(|t| t.status != "done").map(|t| t.id).collect();
    let is_busy = |iv: &Interval| {
        fixed.iter().any(|f| f.start < iv.end && iv.start < f.end) || locked.iter().any(|(_, l)| l.start < iv.end && iv.start < l.end)
    };
    let sticky: Vec<(i64, Interval)> = blocks
        .iter()
        .filter(|b| !b.locked && active_ids.contains(&b.task_id))
        .filter_map(|b| match (scheduler::parse_dt(&b.start), scheduler::parse_dt(&b.end)) {
            (Some(s), Some(e)) if e > now && !is_busy(&Interval { start: s, end: e }) => Some((b.task_id, Interval { start: s, end: e })),
            _ => None,
        })
        .collect();

    let mut combined_locked = locked.clone();
    combined_locked.extend(sticky.iter().copied());

    let task_ids: Vec<i64> = tasks.iter().map(|t| t.id).collect();
    let prefs: std::collections::HashMap<i64, SchedulePref> = db::resolve_task_prefs(conn, &task_ids).unwrap_or_default();
    let mut result = scheduler::schedule_with_prefs(now, settings, &tasks, &fixed, &combined_locked, &prefs);
    // Re-emit the kept blocks (as unlocked) so they persist at their current positions.
    for (tid, iv) in &sticky {
        result.blocks.push(Block {
            id: 0,
            task_id: *tid,
            start: scheduler::fmt_dt(iv.start),
            end: scheduler::fmt_dt(iv.end),
            locked: false,
            provider: None,
            external_id: None,
            sync_state: None,
        });
    }
    db::replace_unlocked_blocks(conn, &result.blocks)?;

    let scheduled_ids: std::collections::HashSet<i64> = db::list_blocks(conn)?.iter().map(|b| b.task_id).collect();
    for t in &tasks {
        if t.status == "done" || t.status == "in_progress" {
            continue;
        }
        let new = if scheduled_ids.contains(&t.id) { "scheduled" } else { "todo" };
        if new != t.status {
            db::set_task_status(conn, t.id, new)?;
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn add_task(conn: &Connection, title: &str, minutes: i64) -> i64 {
        db::insert_task(conn, None, title, "", minutes, None, 2, minutes.max(15), 240, &[]).unwrap()
    }
    fn block_start(conn: &Connection, task_id: i64) -> Option<String> {
        db::list_blocks(conn).unwrap().into_iter().find(|b| b.task_id == task_id).map(|b| b.start)
    }

    #[test]
    fn adding_a_task_keeps_existing_blocks_put() {
        // The stability guarantee: adding a new task slots it in AROUND the existing schedule instead of
        // re-packing the calendar (which used to make already-scheduled tasks jump around).
        let mut conn = db::test_conn();
        let s = Settings::default();
        let a = add_task(&conn, "Alpha", 60);
        let b = add_task(&conn, "Bravo", 60);
        reschedule_inner(&mut conn, &s).unwrap();
        let (a0, b0) = (block_start(&conn, a), block_start(&conn, b));
        assert!(a0.is_some() && b0.is_some(), "both existing tasks are scheduled");

        let c = add_task(&conn, "Charlie", 60);
        reschedule_inner(&mut conn, &s).unwrap();

        assert_eq!(block_start(&conn, a), a0, "existing task Alpha did not move");
        assert_eq!(block_start(&conn, b), b0, "existing task Bravo did not move");
        assert!(block_start(&conn, c).is_some(), "the new task Charlie got scheduled");
    }

    #[test]
    fn a_habit_moves_off_a_newly_overlapping_event() {
        // Reproduces the "watch the game 7–9pm landed on top of the Gym habit" bug: a future habit that
        // now overlaps a real (non-habit) event must be re-flowed to a free slot, not left overlapping.
        let mut conn = db::test_conn();
        let s = Settings::default();
        let day = (Local::now().naive_local().date()) + chrono::Duration::days(2);
        let at = |h: u32, m: u32| scheduler::fmt_dt(day.and_hms_opt(h, m, 0).unwrap());

        // Habit occurrence sits 6:30–7:30pm.
        db::insert_event(&conn, "Gym", &at(18, 30), &at(19, 30), "habit").unwrap();
        // User adds a fixed event 7–9pm right on top of it.
        db::insert_event(&conn, "Watch World Cup Game", &at(19, 0), &at(21, 0), "fixed").unwrap();

        reschedule_inner(&mut conn, &s).unwrap();

        let events = db::list_events(&conn).unwrap();
        let game = events.iter().find(|e| e.title == "Watch World Cup Game").unwrap();
        let gym = events.iter().find(|e| e.title == "Gym").unwrap();
        let (gs, ge) = (scheduler::parse_dt(&game.start).unwrap(), scheduler::parse_dt(&game.end).unwrap());
        let (hs, he) = (scheduler::parse_dt(&gym.start).unwrap(), scheduler::parse_dt(&gym.end).unwrap());

        assert!(gs.to_string().starts_with(&day.to_string()), "sanity: same day");
        assert!(!(gs < he && hs < ge), "Gym habit ({hs}–{he}) must no longer overlap the game ({gs}–{ge})");
        assert_eq!((he - hs).num_minutes(), 60, "habit keeps its 60-min duration");
    }

    #[test]
    fn two_overlapping_habits_get_separated() {
        // Habits shouldn't overlap each other either: the earlier-starting one stays, the later steps aside.
        let mut conn = db::test_conn();
        let s = Settings::default();
        let day = (Local::now().naive_local().date()) + chrono::Duration::days(2);
        let at = |h: u32, m: u32| scheduler::fmt_dt(day.and_hms_opt(h, m, 0).unwrap());

        db::insert_event(&conn, "Gym", &at(18, 0), &at(19, 0), "habit").unwrap();
        db::insert_event(&conn, "Walk", &at(18, 30), &at(19, 0), "habit").unwrap(); // overlaps Gym

        reschedule_inner(&mut conn, &s).unwrap();

        let events = db::list_events(&conn).unwrap();
        let gym = events.iter().find(|e| e.title == "Gym").unwrap();
        let walk = events.iter().find(|e| e.title == "Walk").unwrap();
        let (gs, ge) = (scheduler::parse_dt(&gym.start).unwrap(), scheduler::parse_dt(&gym.end).unwrap());
        let (ws, we) = (scheduler::parse_dt(&walk.start).unwrap(), scheduler::parse_dt(&walk.end).unwrap());

        assert_eq!(scheduler::fmt_dt(gs), at(18, 0), "earlier-starting Gym stays put");
        assert!(!(gs < we && ws < ge), "Walk ({ws}–{we}) must no longer overlap Gym ({gs}–{ge})");
    }
}
