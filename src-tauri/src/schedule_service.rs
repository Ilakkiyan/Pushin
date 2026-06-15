use crate::model::{ScheduleResult, Settings};
use crate::scheduler::{self, Interval};
use crate::{db, scheduler::SchedulePref};
use anyhow::Result;
use chrono::Local;
use rusqlite::Connection;

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

    let task_ids: Vec<i64> = tasks.iter().map(|t| t.id).collect();
    let prefs: std::collections::HashMap<i64, SchedulePref> = db::resolve_task_prefs(conn, &task_ids).unwrap_or_default();
    let now = Local::now().naive_local();
    let result = scheduler::schedule_with_prefs(now, settings, &tasks, &fixed, &locked, &prefs);
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
