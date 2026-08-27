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

/// The day-rollover sweep: work whose planned time came and went, kicked forward.
///
/// A block is **stale** once it ends before midnight of `now`'s date — i.e. it belongs to a day that
/// is over. Stale blocks of tasks that are still active get deleted (pinned ones included: a pin
/// says "do it at *this* time", and a day later there is no such time left to hold), the task is
/// counted as missed once for that day, and the scheduler pass that follows re-places the freed
/// minutes in the next available slot. Nothing that belongs to *today* is touched, so the calendar
/// never yanks work out from under you mid-day — a block you blew past at 9am sits there until the
/// day turns over, then moves.
///
/// Deliberately narrow:
/// - **Only tasks.** A meeting you didn't attend is history, not something to re-plan; habits have
///   their own occurrence/streak logic that rolling a miss forward would corrupt.
/// - **Done/archived tasks are not swept.** The sweep is about re-planning unfinished work, and
///   there is none. Note this is narrower than "their blocks are kept": ticking a task off
///   removes its auto-scheduled block on the next `reschedule` (see `reschedule_inner`), which
///   is deliberate — the calendar shows what is PLANNED, and the done bin in the task list is
///   the record of what was finished. Only a *pinned* block outlives completion.
/// - **Idempotent.** `mark_task_missed` only counts the first sweep of a given local date, so the
///   dozens of reschedules a normal day triggers can't inflate the count.
///
/// Returns how many tasks were newly counted as missed.
pub fn sweep_missed(conn: &Connection, now: NaiveDateTime) -> Result<usize> {
    let today = now.date();
    let day_start = today.and_hms_opt(0, 0, 0).unwrap();
    let today_str = today.format("%Y-%m-%d").to_string();

    let tasks = db::list_tasks(conn)?;
    let blocks = db::list_blocks(conn)?;

    let mut stale_ids: Vec<i64> = Vec::new();
    let mut rolled = 0usize;
    for t in tasks.iter().filter(|t| t.is_active()) {
        let stale: Vec<i64> = blocks
            .iter()
            .filter(|b| b.task_id == t.id)
            .filter(|b| scheduler::parse_dt(&b.end).is_some_and(|e| e <= day_start))
            .map(|b| b.id)
            .collect();
        if stale.is_empty() {
            continue;
        }
        stale_ids.extend(stale);
        if db::mark_task_missed(conn, t.id, &today_str)? {
            rolled += 1;
        }
    }
    db::delete_blocks(conn, &stale_ids)?;
    Ok(rolled)
}

/// Recompute the schedule from the current DB state and persist the new blocks.
pub fn reschedule_inner(conn: &mut Connection, settings: &Settings) -> Result<ScheduleResult> {
    let now = Local::now().naive_local();

    // Kick unfinished work from days gone by forward FIRST — the sweep deletes stale blocks (pinned
    // ones included), so everything below must be read after it, or this pass would plan around
    // blocks that are about to disappear.
    sweep_missed(conn, now)?;

    let mut tasks = db::list_tasks(conn)?;
    // Adaptive estimate: bias not-done task durations by what completed tasks ACTUALLY took
    // (focus-tracked). A soft input — `estimation_factor` is 1.0 (no change) until there's history,
    // so the scheduler stays deterministic and its tests are unaffected. Stored estimates are not
    // mutated; only this scheduling pass is rescaled.
    let factor = scheduler::estimation_factor(&db::estimation_samples(conn).unwrap_or_default());
    if (factor - 1.0).abs() > 1e-6 {
        for t in &mut tasks {
            if t.is_active() && t.status != "in_progress" {
                t.estimated_minutes = ((t.estimated_minutes as f64 * factor).round() as i64).max(15);
            }
        }
    }

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
    let active_ids: HashSet<i64> = tasks.iter().filter(|t| t.is_active()).map(|t| t.id).collect();
    let is_busy = |iv: &Interval| {
        fixed.iter().any(|f| f.start < iv.end && iv.start < f.end) || locked.iter().any(|(_, l)| l.start < iv.end && iv.start < l.end)
    };
    //
    // The cutoff is **midnight today**, not `now`: a block whose time you blew past this morning is
    // kept (and its minutes still count against the task's estimate), so the calendar doesn't
    // rearrange itself under you the moment a block ends. `sweep_missed` above is what eventually
    // clears it — once the day is over.
    let today_start = now.date().and_hms_opt(0, 0, 0).unwrap();
    let sticky: Vec<(i64, Interval)> = blocks
        .iter()
        .filter(|b| !b.locked && active_ids.contains(&b.task_id))
        .filter_map(|b| match (scheduler::parse_dt(&b.start), scheduler::parse_dt(&b.end)) {
            (Some(s), Some(e)) if e > today_start && !is_busy(&Interval { start: s, end: e }) => {
                Some((b.task_id, Interval { start: s, end: e }))
            }
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
        if !t.is_active() || t.status == "in_progress" {
            continue;
        }
        let new = if scheduled_ids.contains(&t.id) { "scheduled" } else { "todo" };
        if new != t.status {
            db::set_task_status(conn, t.id, new)?;
        }
    }
    Ok(result)
}

/// Move a task's WHOLE scheduled body of work so it begins at `start`, re-flowing it around
/// whatever is actually in the way.
///
/// This is what a calendar drag calls. Dragging used to pin the one chunk under the cursor
/// (`db::set_block_locked`), which is wrong for any task the scheduler had split around a meeting:
/// the dragged half moved, the other half stayed pinned where it was, and the task read as two
/// identically-titled events that could never be reunited. Here the task's chunks are pooled and
/// re-laid from the drop point, so they **merge back into one block wherever the space allows** and
/// only **split again around real obstacles** — matching what the user sees on the grid.
///
/// The free-time view is [`scheduler::free_after`], not [`scheduler::free_slots`]: a drag is an
/// explicit instruction, so it may land outside work hours or in the sleep window. The one thing it
/// will not do is overlap a fixed event, a habit, or another task's block — it slides past those.
///
/// The new blocks are **pinned**, matching the old drag behavior: you moved it there on purpose, so
/// the next reschedule leaves it alone.
pub fn move_task_to(
    conn: &mut Connection,
    settings: &Settings,
    task_id: i64,
    start: NaiveDateTime,
) -> Result<ScheduleResult> {
    let tasks = db::list_tasks(conn)?;
    let Some(task) = tasks.iter().find(|t| t.id == task_id) else {
        return reschedule_inner(conn, settings);
    };
    // A finished or archived task's blocks are the RECORD of when the work happened — the same
    // reason `sweep_missed` refuses to touch them. Re-laying them would rewrite history from a
    // stray drag on a block that is still drawn on the calendar.
    if !task.is_active() {
        return reschedule_inner(conn, settings);
    }
    let events = db::list_events(conn)?;
    let blocks = db::list_blocks(conn)?;

    let span = |s: &str, e: &str| match (scheduler::parse_dt(s), scheduler::parse_dt(e)) {
        (Some(s), Some(e)) if e > s => Some(Interval { start: s, end: e }),
        _ => None,
    };

    // How much work is being moved: whatever this task currently occupies on the calendar. A task
    // with no blocks yet (dragged straight out of the task list) falls back to its estimate.
    let mine: Vec<Interval> = blocks.iter().filter(|b| b.task_id == task_id).filter_map(|b| span(&b.start, &b.end)).collect();
    let total: i64 = mine.iter().map(|iv| (iv.end - iv.start).num_minutes()).sum();
    let total = if total > 0 { total } else { task.estimated_minutes.max(0) };
    if total <= 0 {
        return reschedule_inner(conn, settings);
    }

    // Everything the moved task has to flow around — every OTHER task's blocks, plus every event
    // (habit occurrences are events too).
    let mut busy: Vec<Interval> = events.iter().filter_map(|e| span(&e.start, &e.end)).collect();
    busy.extend(blocks.iter().filter(|b| b.task_id != task_id).filter_map(|b| span(&b.start, &b.end)));
    busy.sort_by_key(|i| i.start);

    // Chunking rule stays the task's own: dragging must not manufacture fragments smaller than the
    // task says it can be worked in.
    let min_chunk = task.min_chunk_minutes.max(1);
    let mut free = scheduler::free_after(start, &busy, settings.horizon_days.max(1));
    let (placed, _, _) = scheduler::place(&mut free, start, total, min_chunk, None);

    // Nothing could be placed at all (dropped into a wall of busy time with no gap big enough) —
    // leave the task exactly as it was rather than silently deleting its blocks.
    if placed.is_empty() {
        return reschedule_inner(conn, settings);
    }

    let spans: Vec<(String, String)> =
        placed.iter().map(|iv| (scheduler::fmt_dt(iv.start), scheduler::fmt_dt(iv.end))).collect();
    db::replace_task_blocks(conn, task_id, &spans)?;
    reschedule_inner(conn, settings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Datelike; // only the drag fixtures need weekday maths

    fn add_task(conn: &Connection, title: &str, minutes: i64) -> i64 {
        db::insert_task(conn, None, title, "", minutes, None, 2, minutes.max(15), 240, &[]).unwrap()
    }
    /// Like `add_task` but with an explicit min-chunk. `add_task` sets min_chunk to the whole
    /// estimate, which makes the task physically un-splittable — fine for the rollover tests, but it
    /// would make the drag split/merge tests below pass for the wrong reason.
    fn add_splittable_task(conn: &Connection, title: &str, minutes: i64, min_chunk: i64) -> i64 {
        db::insert_task(conn, None, title, "", minutes, None, 2, min_chunk, 240, &[]).unwrap()
    }
    fn block_start(conn: &Connection, task_id: i64) -> Option<String> {
        db::list_blocks(conn).unwrap().into_iter().find(|b| b.task_id == task_id).map(|b| b.start)
    }

    /// Put a block straight into the DB at an arbitrary (possibly past) time — the scheduler will
    /// never produce one, so the rollover tests have to plant them by hand.
    fn plant_block(conn: &Connection, task_id: i64, start: NaiveDateTime, mins: i64, locked: bool) -> i64 {
        conn.execute(
            "INSERT INTO blocks(task_id, start, end, locked) VALUES(?1, ?2, ?3, ?4)",
            rusqlite::params![
                task_id,
                scheduler::fmt_dt(start),
                scheduler::fmt_dt(start + Duration::minutes(mins)),
                locked as i64
            ],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    /// A fixed hour on a past day, independent of the wall clock.
    ///
    /// The rollover tests used to plant blocks at `now - N days`, which quietly breaks late at
    /// night: run at 23:30 and a 60-minute block planted "yesterday" *ends* at 00:30 **today**, so
    /// the end-based sweep correctly leaves it alone and every assertion below flips. Four of these
    /// tests were red on `main` for exactly that reason. Anchor to 09:00 on the target date so the
    /// block is unambiguously inside a day that is over, at any hour the suite happens to run.
    fn days_ago_at(now: NaiveDateTime, days: i64, hour: u32) -> NaiveDateTime {
        (now.date() - Duration::days(days)).and_hms_opt(hour, 0, 0).unwrap()
    }

    fn task_by_id(conn: &Connection, id: i64) -> crate::model::Task {
        db::list_tasks(conn).unwrap().into_iter().find(|t| t.id == id).unwrap()
    }

    #[test]
    fn a_missed_task_is_kicked_to_the_next_available_slot() {
        // The headline behavior: yesterday's block came and went without the task being done, so the
        // block is gone and the task is planned again — in the future, never back in the past.
        let mut conn = db::test_conn();
        let s = Settings::default();
        let now = Local::now().naive_local();
        let t = add_task(&conn, "Missed essay", 60);
        plant_block(&conn, t, days_ago_at(now, 1, 9), 60, false);

        reschedule_inner(&mut conn, &s).unwrap();

        let blocks: Vec<Block> = db::list_blocks(&conn).unwrap().into_iter().filter(|b| b.task_id == t).collect();
        assert_eq!(blocks.len(), 1, "exactly one block — the stale one replaced, not added to");
        let start = scheduler::parse_dt(&blocks[0].start).unwrap();
        assert!(start >= now, "re-placed in the future ({start}), not left in the past");
        assert_eq!(task_by_id(&conn, t).missed_count, 1, "counted as missed once");
    }

    #[test]
    fn a_pinned_block_that_has_passed_rolls_and_loses_its_pin() {
        // A pin means "do it at THIS time". Once that time is a day gone there is nothing left to
        // pin to, so the sweep is allowed to drop it — otherwise the task sits in yesterday forever
        // looking "scheduled" while its time has quietly evaporated.
        let mut conn = db::test_conn();
        let s = Settings::default();
        let now = Local::now().naive_local();
        let t = add_task(&conn, "Pinned work", 60);
        plant_block(&conn, t, days_ago_at(now, 1, 9), 60, true);

        reschedule_inner(&mut conn, &s).unwrap();

        let blocks: Vec<Block> = db::list_blocks(&conn).unwrap().into_iter().filter(|b| b.task_id == t).collect();
        assert_eq!(blocks.len(), 1, "the stale pin is replaced by one fresh block");
        assert!(!blocks[0].locked, "the re-placed block is no longer pinned");
        assert!(scheduler::parse_dt(&blocks[0].start).unwrap() >= now, "moved forward");
        assert_eq!(task_by_id(&conn, t).missed_count, 1);
    }

    #[test]
    fn a_task_whose_deadline_already_passed_still_gets_scheduled() {
        // The bug that made the whole feature moot: placement was capped at the deadline, so a task
        // you'd already blown past had a zero-width window and got NO block at all — it vanished
        // from the calendar precisely when you most needed to see it. It must be planned into the
        // next free slot AND still reported as a deadline miss.
        let mut conn = db::test_conn();
        let s = Settings::default();
        let now = Local::now().naive_local();
        let yesterday = scheduler::fmt_dt(now - Duration::days(1));
        let t = db::insert_task(&conn, None, "Late essay", "", 60, Some(&yesterday), 3, 30, 120, &[]).unwrap();

        let r = reschedule_inner(&mut conn, &s).unwrap();

        let blocks: Vec<Block> = db::list_blocks(&conn).unwrap().into_iter().filter(|b| b.task_id == t).collect();
        // Assert it is SCHEDULED, not that it landed in exactly one block. The bug this guards is
        // "no block at all"; how many chunks it takes is the scheduler's business and depends on the
        // clock — run this after ~16:00 and a 60m task with a 30m min-chunk correctly splits across
        // the work-day boundary, which made the old `len() == 1` fail every afternoon.
        assert!(!blocks.is_empty(), "an overdue task is still put on the calendar");
        let scheduled: i64 = blocks
            .iter()
            .map(|b| {
                let (st, en) = (scheduler::parse_dt(&b.start).unwrap(), scheduler::parse_dt(&b.end).unwrap());
                (en - st).num_minutes()
            })
            .sum();
        assert_eq!(scheduled, 60, "all of its estimate is placed, however it is chunked");
        let earliest = blocks.iter().map(|b| scheduler::parse_dt(&b.start).unwrap()).min().unwrap();
        assert!(earliest >= now, "and it is placed in the future, not left in the past");
        assert!(
            r.conflicts.iter().any(|c| matches!(c, crate::model::Conflict::DeadlineMiss { task_id, .. } if *task_id == t)),
            "and it's still flagged as past its deadline: {:?}",
            r.conflicts
        );
    }

    #[test]
    fn a_block_missed_earlier_today_does_not_move_until_the_day_is_over() {
        // The user's chosen cadence: nothing is rearranged mid-day. A block you blew past at 9am
        // stays sitting at 9am — the sweep only kicks work forward once the day it belonged to is
        // actually over, so the calendar never reshuffles itself while you're looking at it.
        let mut conn = db::test_conn();
        let s = Settings::default();
        let now = Local::now().naive_local();
        // A block that started at midnight and has already ended — same day, firmly in the past.
        let start = now.date().and_hms_opt(0, 0, 0).unwrap();
        let mins = ((now - start).num_minutes() - 5).max(1);
        if mins < 15 {
            return; // running within 20 minutes of midnight — no room for an already-passed block today
        }
        // The task's estimate is exactly the planted block's length, so the sticky block covers all
        // of it and the scheduler has nothing left to place — which is what makes "exactly one
        // block" mean "the one that was already there". A fixed 60-minute estimate made this test
        // depend on the wall clock: for the 50 minutes after midnight the elapsed day is shorter
        // than the estimate, the scheduler correctly adds a second block for the remainder, and the
        // assertion below flipped for reasons that had nothing to do with the rollover rule.
        let t = add_task(&conn, "Blew past it", mins);
        plant_block(&conn, t, start, mins, false);

        reschedule_inner(&mut conn, &s).unwrap();

        let blocks: Vec<Block> = db::list_blocks(&conn).unwrap().into_iter().filter(|b| b.task_id == t).collect();
        assert_eq!(blocks.len(), 1, "today's block is kept, not swapped for a new one");
        assert_eq!(scheduler::parse_dt(&blocks[0].start).unwrap(), start, "and it did not move");
        assert_eq!(task_by_id(&conn, t).missed_count, 0, "not counted as missed while its day is still running");
    }

    #[test]
    fn sweeping_twice_in_a_day_counts_the_miss_once() {
        // `reschedule` fires on nearly every user action, so the sweep runs dozens of times a day.
        // The count has to track misses, not reschedules.
        let mut conn = db::test_conn();
        let s = Settings::default();
        let now = Local::now().naive_local();
        let t = add_task(&conn, "Missed twice over", 60);
        plant_block(&conn, t, days_ago_at(now, 1, 9), 60, false);

        reschedule_inner(&mut conn, &s).unwrap();
        reschedule_inner(&mut conn, &s).unwrap();
        reschedule_inner(&mut conn, &s).unwrap();

        assert_eq!(task_by_id(&conn, t).missed_count, 1, "three reschedules, one missed day, one count");
    }

    #[test]
    fn a_finished_task_keeps_its_history_and_is_never_swept() {
        // A done task's past blocks are the record of when the work happened — the sweep must leave
        // them alone and must not mark a finished task as missed.
        let conn = db::test_conn();
        let now = Local::now().naive_local();
        let t = add_task(&conn, "Already done", 60);
        plant_block(&conn, t, days_ago_at(now, 1, 9), 60, true);
        db::set_task_status(&conn, t, "done").unwrap();

        let rolled = sweep_missed(&conn, now).unwrap();

        assert_eq!(rolled, 0);
        assert_eq!(db::list_blocks(&conn).unwrap().len(), 1, "the completed block stays as history");
        assert_eq!(task_by_id(&conn, t).missed_count, 0);
    }

    #[test]
    fn repeated_misses_accumulate_across_days() {
        // Missing it again on a later day bumps the count again — that's the "you keep pushing this
        // one" signal. Driven through `sweep_missed` directly so two distinct days can be simulated.
        let conn = db::test_conn();
        let now = Local::now().naive_local();
        let t = add_task(&conn, "Perpetually deferred", 60);

        plant_block(&conn, t, days_ago_at(now, 3, 9), 60, false);
        assert_eq!(sweep_missed(&conn, days_ago_at(now, 2, 12)).unwrap(), 1);
        plant_block(&conn, t, days_ago_at(now, 2, 9), 60, false);
        assert_eq!(sweep_missed(&conn, days_ago_at(now, 1, 12)).unwrap(), 1);

        let task = task_by_id(&conn, t);
        assert_eq!(task.missed_count, 2, "two separate days missed");
        assert_eq!(task.last_missed_on, Some((now - Duration::days(1)).date().format("%Y-%m-%d").to_string()));
    }

    #[test]
    fn a_missed_meeting_is_left_alone() {
        // Scope guard: only tasks roll. A meeting you didn't attend is history, not work to re-plan.
        let mut conn = db::test_conn();
        let s = Settings::default();
        let now = Local::now().naive_local();
        let start = scheduler::fmt_dt(now - Duration::days(1));
        let end = scheduler::fmt_dt(now - Duration::days(1) + Duration::minutes(30));
        db::insert_event(&conn, "Standup I slept through", &start, &end, "fixed").unwrap();

        reschedule_inner(&mut conn, &s).unwrap();

        let ev = db::list_events(&conn).unwrap().into_iter().find(|e| e.title.starts_with("Standup")).unwrap();
        assert_eq!(ev.start, start, "the missed meeting stays where it was");
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

    // ---------------- dragging a task on the calendar ----------------
    //
    // A task the scheduler split around a meeting is several blocks sharing one title. Dragging any
    // of them means "put this task here": the chunks pool and re-lay from the drop point, merging
    // where the space allows and splitting only around what is genuinely in the way.

    /// Blocks of one task, in time order, as (start, end) minute-of-day pairs on `day`.
    fn task_spans(conn: &Connection, task_id: i64) -> Vec<(NaiveDateTime, NaiveDateTime)> {
        let mut v: Vec<(NaiveDateTime, NaiveDateTime)> = db::list_blocks(conn)
            .unwrap()
            .into_iter()
            .filter(|b| b.task_id == task_id)
            .map(|b| (scheduler::parse_dt(&b.start).unwrap(), scheduler::parse_dt(&b.end).unwrap()))
            .collect();
        v.sort_by_key(|(s, _)| *s);
        v
    }

    fn minutes(spans: &[(NaiveDateTime, NaiveDateTime)]) -> i64 {
        spans.iter().map(|(s, e)| (*e - *s).num_minutes()).sum()
    }

    /// A weekday inside the work week, far enough ahead that "now" never overtakes the fixture.
    fn future_workday(now: NaiveDateTime) -> chrono::NaiveDate {
        let mut d = now.date() + Duration::days(2);
        while !Settings::default().work_days.contains(&(d.weekday().number_from_monday() as u8)) {
            d += Duration::days(1);
        }
        d
    }

    #[test]
    fn dragging_a_split_task_into_open_space_merges_its_chunks_into_one_block() {
        // The reported bug: a 2h task split around an 11:00 meeting showed as two identically-named
        // blocks, and dragging one moved only that half. Dropped somewhere with room for the whole
        // thing, it must come back as ONE block.
        let mut conn = db::test_conn();
        let s = Settings::default();
        let now = Local::now().naive_local();
        let day = future_workday(now);

        let t = add_splittable_task(&conn, "Split essay", 120, 30);
        // 09:00-10:00 and 11:00-12:00, straddling a meeting at 10:00.
        plant_block(&conn, t, day.and_hms_opt(9, 0, 0).unwrap(), 60, false);
        plant_block(&conn, t, day.and_hms_opt(11, 0, 0).unwrap(), 60, false);
        db::insert_event(
            &conn,
            "Standup",
            &scheduler::fmt_dt(day.and_hms_opt(10, 0, 0).unwrap()),
            &scheduler::fmt_dt(day.and_hms_opt(11, 0, 0).unwrap()),
            "fixed",
        )
        .unwrap();
        assert_eq!(task_spans(&conn, t).len(), 2, "precondition: the task starts out split");

        // Drop it at 13:00, where the afternoon is wide open.
        let target = day.and_hms_opt(13, 0, 0).unwrap();
        move_task_to(&mut conn, &s, t, target).unwrap();

        let spans = task_spans(&conn, t);
        assert_eq!(spans.len(), 1, "the two halves merged back into one block, got {spans:?}");
        assert_eq!(spans[0].0, target, "and it starts exactly where it was dropped");
        assert_eq!(minutes(&spans), 120, "with all of its minutes intact");
    }

    #[test]
    fn dragging_a_whole_task_onto_a_meeting_splits_it_around_the_meeting() {
        // The other half of the behavior: drop a contiguous task where something already sits and it
        // parts around it instead of overlapping or snapping away.
        let mut conn = db::test_conn();
        let s = Settings::default();
        let now = Local::now().naive_local();
        let day = future_workday(now);

        let t = add_splittable_task(&conn, "Report", 120, 30);
        plant_block(&conn, t, day.and_hms_opt(9, 0, 0).unwrap(), 120, false);
        db::insert_event(
            &conn,
            "Client call",
            &scheduler::fmt_dt(day.and_hms_opt(14, 0, 0).unwrap()),
            &scheduler::fmt_dt(day.and_hms_opt(15, 0, 0).unwrap()),
            "fixed",
        )
        .unwrap();

        // Drop at 13:00 — one hour of room, then the call, then open again.
        move_task_to(&mut conn, &s, t, day.and_hms_opt(13, 0, 0).unwrap()).unwrap();

        let spans = task_spans(&conn, t);
        assert_eq!(spans.len(), 2, "split around the call, got {spans:?}");
        assert_eq!(spans[0], (day.and_hms_opt(13, 0, 0).unwrap(), day.and_hms_opt(14, 0, 0).unwrap()));
        assert_eq!(spans[1], (day.and_hms_opt(15, 0, 0).unwrap(), day.and_hms_opt(16, 0, 0).unwrap()));
        assert_eq!(minutes(&spans), 120, "no minutes lost in the split");
    }

    #[test]
    fn a_drop_inside_a_busy_interval_slides_forward_instead_of_overlapping() {
        let mut conn = db::test_conn();
        let s = Settings::default();
        let day = future_workday(Local::now().naive_local());

        let t = add_task(&conn, "Deep work", 60);
        plant_block(&conn, t, day.and_hms_opt(9, 0, 0).unwrap(), 60, false);
        db::insert_event(
            &conn,
            "Interview",
            &scheduler::fmt_dt(day.and_hms_opt(13, 0, 0).unwrap()),
            &scheduler::fmt_dt(day.and_hms_opt(14, 0, 0).unwrap()),
            "fixed",
        )
        .unwrap();

        // Drop right in the middle of the interview.
        move_task_to(&mut conn, &s, t, day.and_hms_opt(13, 30, 0).unwrap()).unwrap();

        let spans = task_spans(&conn, t);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].0, day.and_hms_opt(14, 0, 0).unwrap(), "slid to the end of what was in the way");
    }

    #[test]
    fn a_dragged_task_may_land_outside_working_hours() {
        // A drag is an explicit instruction. The auto-scheduler stays inside the work day; a hand
        // drag is allowed to put work in the evening, and must not be quietly pulled back to 09:00.
        let mut conn = db::test_conn();
        let s = Settings::default();
        let day = future_workday(Local::now().naive_local());

        let t = add_task(&conn, "Evening reading", 60);
        plant_block(&conn, t, day.and_hms_opt(10, 0, 0).unwrap(), 60, false);

        let evening = day.and_hms_opt(21, 0, 0).unwrap();
        move_task_to(&mut conn, &s, t, evening).unwrap();

        let spans = task_spans(&conn, t);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].0, evening, "the drop time is honoured outside the work window");
    }

    #[test]
    fn a_dragged_task_is_pinned_so_the_next_reschedule_leaves_it_alone() {
        // Dragging has always meant "pin here". Losing that would make the block snap back to the
        // auto-schedule on the very next user action, which fires constantly.
        let mut conn = db::test_conn();
        let s = Settings::default();
        let day = future_workday(Local::now().naive_local());

        let t = add_task(&conn, "Pinned by drag", 60);
        plant_block(&conn, t, day.and_hms_opt(9, 0, 0).unwrap(), 60, false);
        let target = day.and_hms_opt(15, 30, 0).unwrap();
        move_task_to(&mut conn, &s, t, target).unwrap();

        assert!(
            db::list_blocks(&conn).unwrap().iter().filter(|b| b.task_id == t).all(|b| b.locked),
            "every re-laid chunk is pinned"
        );

        // ...and it survives a full reschedule unchanged.
        reschedule_inner(&mut conn, &s).unwrap();
        assert_eq!(task_spans(&conn, t), vec![(target, target + Duration::minutes(60))]);
    }

    #[test]
    fn dragging_one_task_does_not_disturb_another() {
        let mut conn = db::test_conn();
        let s = Settings::default();
        let day = future_workday(Local::now().naive_local());

        let a = add_task(&conn, "Task A", 60);
        let b = add_task(&conn, "Task B", 60);
        plant_block(&conn, a, day.and_hms_opt(9, 0, 0).unwrap(), 60, true);
        plant_block(&conn, b, day.and_hms_opt(11, 0, 0).unwrap(), 60, true);
        let b_before = task_spans(&conn, b);

        move_task_to(&mut conn, &s, a, day.and_hms_opt(14, 0, 0).unwrap()).unwrap();

        assert_eq!(task_spans(&conn, b), b_before, "the other task stayed put");
        assert_eq!(task_spans(&conn, a).len(), 1);
    }

    #[test]
    fn a_dragged_task_will_not_land_on_another_tasks_block() {
        let mut conn = db::test_conn();
        let s = Settings::default();
        let day = future_workday(Local::now().naive_local());

        let a = add_task(&conn, "Mover", 60);
        let b = add_task(&conn, "Squatter", 60);
        plant_block(&conn, a, day.and_hms_opt(9, 0, 0).unwrap(), 60, false);
        plant_block(&conn, b, day.and_hms_opt(14, 0, 0).unwrap(), 60, true);

        move_task_to(&mut conn, &s, a, day.and_hms_opt(14, 0, 0).unwrap()).unwrap();

        let a_spans = task_spans(&conn, a);
        let b_spans = task_spans(&conn, b);
        for (as_, ae) in &a_spans {
            for (bs, be) in &b_spans {
                assert!(as_ >= be || ae <= bs, "dragged task overlaps another task's block: {a_spans:?} vs {b_spans:?}");
            }
        }
    }

    #[test]
    fn the_min_chunk_is_respected_when_a_drag_has_to_split() {
        // Splitting must not manufacture fragments smaller than the task says it can be worked in —
        // a 15-minute sliver of a 90-minute deep-work task is worse than pushing it later.
        let mut conn = db::test_conn();
        let s = Settings::default();
        let day = future_workday(Local::now().naive_local());

        // min_chunk 60, total 120.
        let t = add_splittable_task(&conn, "Deep work", 120, 60);
        plant_block(&conn, t, day.and_hms_opt(9, 0, 0).unwrap(), 120, false);
        // Leave only a 30-minute gap at the drop point, then a wall, then open space.
        db::insert_event(
            &conn,
            "Wall",
            &scheduler::fmt_dt(day.and_hms_opt(13, 30, 0).unwrap()),
            &scheduler::fmt_dt(day.and_hms_opt(15, 0, 0).unwrap()),
            "fixed",
        )
        .unwrap();

        move_task_to(&mut conn, &s, t, day.and_hms_opt(13, 0, 0).unwrap()).unwrap();

        let spans = task_spans(&conn, t);
        assert!(
            spans.iter().all(|(s0, e0)| (*e0 - *s0).num_minutes() >= 60),
            "a sub-min-chunk sliver was created: {spans:?}"
        );
        assert_eq!(minutes(&spans), 120);
    }

    #[test]
    fn dragging_a_task_that_has_no_blocks_yet_schedules_its_estimate() {
        let mut conn = db::test_conn();
        let s = Settings::default();
        let day = future_workday(Local::now().naive_local());

        let t = add_task(&conn, "Brand new", 45);
        // Give the auto-scheduler nothing to do first — drop it straight onto a time.
        let target = day.and_hms_opt(16, 0, 0).unwrap();
        move_task_to(&mut conn, &s, t, target).unwrap();

        let spans = task_spans(&conn, t);
        assert_eq!(minutes(&spans), 45, "the estimate is what gets placed, got {spans:?}");
        assert_eq!(spans[0].0, target);
    }

    #[test]
    fn dragging_a_finished_task_does_not_rewrite_its_history() {
        // A done task's blocks record when the work actually happened, and they are still drawn on
        // the calendar — so a stray drag must not re-lay them somewhere else.
        let mut conn = db::test_conn();
        let s = Settings::default();
        let day = future_workday(Local::now().naive_local());

        let t = add_task(&conn, "Already done", 60);
        // Pinned, matching `a_finished_task_keeps_its_history_and_is_never_swept` — see
        // `a_done_tasks_unpinned_block_does_not_survive_a_reschedule` for why that matters.
        plant_block(&conn, t, day.and_hms_opt(9, 0, 0).unwrap(), 60, true);
        db::set_task_status(&conn, t, "done").unwrap();
        let before = task_spans(&conn, t);

        move_task_to(&mut conn, &s, t, day.and_hms_opt(15, 0, 0).unwrap()).unwrap();

        assert_eq!(task_spans(&conn, t), before, "history stayed where it was");
    }

    /// Ticking a task off takes it off the calendar. **This is the intended behavior**, confirmed as
    /// a product decision: the calendar shows what is PLANNED, and the done bin in the task list is
    /// the record of what was finished. Finishing a task calls `reschedule`,
    /// `replace_unlocked_blocks` deletes every `locked = 0` row, and a done task is not in
    /// `active_ids` so it is not re-emitted as sticky either — the block goes.
    ///
    /// Kept as a test because it is load-bearing and non-obvious: the deletion happens two layers
    /// away from the checkbox, so anything that made done tasks sticky (or pinned their blocks on
    /// completion) would silently leave finished work cluttering the week.
    #[test]
    fn ticking_a_task_off_removes_it_from_the_calendar() {
        let mut conn = db::test_conn();
        let s = Settings::default();
        let day = future_workday(Local::now().naive_local());

        let t = add_task(&conn, "Finished this", 60);
        plant_block(&conn, t, day.and_hms_opt(9, 0, 0).unwrap(), 60, false);
        db::set_task_status(&conn, t, "done").unwrap();

        reschedule_inner(&mut conn, &s).unwrap();

        assert!(task_spans(&conn, t).is_empty(), "a finished task leaves the calendar");

        // A PINNED block is the one exception: an explicit "do it at THIS time" outlives completion,
        // so a block you placed by hand is still there as a record of when you did it.
        let p = add_task(&conn, "Also finished", 60);
        plant_block(&conn, p, day.and_hms_opt(11, 0, 0).unwrap(), 60, true);
        db::set_task_status(&conn, p, "done").unwrap();
        reschedule_inner(&mut conn, &s).unwrap();
        assert_eq!(task_spans(&conn, p).len(), 1, "a pinned block of a done task is kept as history");
    }

    #[test]
    fn moving_an_unknown_task_is_a_harmless_no_op() {
        let mut conn = db::test_conn();
        let s = Settings::default();
        let day = future_workday(Local::now().naive_local());
        let t = add_task(&conn, "Real", 60);
        plant_block(&conn, t, day.and_hms_opt(9, 0, 0).unwrap(), 60, true);
        let before = task_spans(&conn, t);

        move_task_to(&mut conn, &s, 999_999, day.and_hms_opt(14, 0, 0).unwrap()).unwrap();

        assert_eq!(task_spans(&conn, t), before, "an unknown id changes nothing");
    }

    #[test]
    fn a_drag_with_nowhere_to_go_leaves_the_task_where_it_was() {
        // Dropped into a solid wall of busy time that runs to the horizon: better to keep the
        // existing plan than to delete the task's blocks and place nothing.
        let mut conn = db::test_conn();
        let mut s = Settings::default();
        s.horizon_days = 1;
        let day = Local::now().naive_local().date();

        let t = add_task(&conn, "Nowhere", 60);
        plant_block(&conn, t, day.and_hms_opt(1, 0, 0).unwrap(), 60, true);
        // Block out the rest of the day from the drop point onward.
        db::insert_event(
            &conn,
            "All afternoon",
            &scheduler::fmt_dt(day.and_hms_opt(12, 0, 0).unwrap()),
            &scheduler::fmt_dt((day + Duration::days(1)).and_hms_opt(0, 0, 0).unwrap()),
            "fixed",
        )
        .unwrap();

        move_task_to(&mut conn, &s, t, day.and_hms_opt(12, 0, 0).unwrap()).unwrap();

        assert!(!task_spans(&conn, t).is_empty(), "the task still has a plan rather than vanishing");
    }
}
