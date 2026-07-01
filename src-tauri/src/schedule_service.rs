use crate::model::{Block, ScheduleResult, Settings};
use crate::scheduler::{self, Interval};
use crate::{db, scheduler::SchedulePref};
use anyhow::Result;
use chrono::Local;
use rusqlite::Connection;
use std::collections::HashSet;

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

    let now = Local::now().naive_local();

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
}
