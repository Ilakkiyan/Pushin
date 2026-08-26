//! Daily Briefing — the morning "here's your day" assembly (ROADMAP Phase 3, Planning Rituals).
//! Pure, deterministic gathering of today's agenda from the same SQLite source of truth: today's
//! fixed events, tasks due today or overdue, and how much focus time is already blocked. The LLM is
//! NOT involved here — this is reliable structured data the UI renders directly.

use crate::model::{Block, Event, Task};
use crate::scheduler::parse_dt;
use chrono::{Datelike, NaiveDate};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Briefing {
    pub date: String,
    pub weekday: String,
    /// Fixed events occurring today, sorted by start.
    pub events: Vec<Event>,
    /// Active tasks due today or overdue by less than [`STALE_AFTER_DAYS`] (overdue first),
    /// sorted by deadline. This is the actionable list.
    pub due_tasks: Vec<Task>,
    /// Active tasks overdue by [`STALE_AFTER_DAYS`] or more, oldest first. Split out rather than
    /// hidden: they used to sit at the top of `due_tasks` forever (deadline <= today had no floor),
    /// so a task missed in June still led the briefing in August and buried what's actually due.
    /// The UI collapses these into a "Stale" group with an archive action.
    pub stale_tasks: Vec<Task>,
    /// Minutes of scheduled task blocks today (how packed the day already is).
    pub focus_minutes: i64,
}

/// How far past its deadline a task must be before it stops counting as "due" and becomes stale.
/// 30 days is a month of the user ignoring it — at that point it is a decision to make, not a
/// reminder to repeat.
pub const STALE_AFTER_DAYS: i64 = 30;

/// Best-effort date out of a deadline string: a bare `YYYY-MM-DD`, else the date part of a full
/// timestamp (`parse_dt` tolerates the ISO variants we store).
fn deadline_date(s: &str) -> Option<NaiveDate> {
    let s = s.trim();
    NaiveDate::parse_from_str(s, "%Y-%m-%d").ok().or_else(|| parse_dt(s).map(|dt| dt.date()))
}

fn weekday_name(d: NaiveDate) -> &'static str {
    match d.weekday() {
        chrono::Weekday::Mon => "Monday",
        chrono::Weekday::Tue => "Tuesday",
        chrono::Weekday::Wed => "Wednesday",
        chrono::Weekday::Thu => "Thursday",
        chrono::Weekday::Fri => "Friday",
        chrono::Weekday::Sat => "Saturday",
        chrono::Weekday::Sun => "Sunday",
    }
}

/// Assemble the briefing for `today` from the full event/task/block sets. Pure.
pub fn assemble(today: NaiveDate, events: &[Event], tasks: &[Task], blocks: &[Block]) -> Briefing {
    let mut day_events: Vec<Event> = events
        .iter()
        .filter(|e| parse_dt(&e.start).map(|dt| dt.date() == today).unwrap_or(false))
        .cloned()
        .collect();
    day_events.sort_by(|a, b| a.start.cmp(&b.start));

    // Everything past its deadline and still real work, split at the stale threshold. `is_active`
    // excludes both done and archived, so archiving a stale task removes it from here for good.
    let stale_before = today - chrono::Duration::days(STALE_AFTER_DAYS);
    let overdue: Vec<(NaiveDate, &Task)> = tasks
        .iter()
        .filter(|t| t.is_active())
        .filter_map(|t| Some((t.deadline.as_deref().and_then(deadline_date)?, t)))
        .filter(|(d, _)| *d <= today)
        .collect();
    let mut due_tasks: Vec<Task> = overdue.iter().filter(|(d, _)| *d > stale_before).map(|(_, t)| (*t).clone()).collect();
    let mut stale_tasks: Vec<Task> = overdue.iter().filter(|(d, _)| *d <= stale_before).map(|(_, t)| (*t).clone()).collect();
    // Overdue first (earliest deadline first); undated shouldn't appear (filtered above).
    due_tasks.sort_by(|a, b| a.deadline.cmp(&b.deadline));
    stale_tasks.sort_by(|a, b| a.deadline.cmp(&b.deadline));

    let focus_minutes: i64 = blocks
        .iter()
        .filter_map(|b| {
            let (s, e) = (parse_dt(&b.start)?, parse_dt(&b.end)?);
            (s.date() == today).then(|| (e - s).num_minutes().max(0))
        })
        .sum();

    Briefing {
        date: today.format("%Y-%m-%d").to_string(),
        weekday: weekday_name(today).to_string(),
        events: day_events,
        due_tasks,
        stale_tasks,
        focus_minutes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Block, Event, Task};

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn event(title: &str, start: &str) -> Event {
        Event {
            id: 0,
            title: title.into(),
            start: start.into(),
            end: format!("{start}").replace("T0", "T1"), // crude later time; not used by assertions
            kind: "fixed".into(),
            source: "manual".into(),
            created_at: String::new(),
            provider: None,
            external_id: None,
            account_id: None,
            etag: None,
        }
    }

    fn task(title: &str, deadline: Option<&str>, status: &str) -> Task {
        Task {
            id: 0,
            project_id: None,
            title: title.into(),
            notes: String::new(),
            estimated_minutes: 60,
            deadline: deadline.map(String::from),
            earliest_start: None,
            priority: 2,
            min_chunk_minutes: 30,
            max_chunk_minutes: 120,
            status: status.into(),
            created_at: String::new(),
            missed_count: 0,
            last_missed_on: None,
            depends_on: vec![],
        }
    }

    fn block(task_id: i64, start: &str, end: &str) -> Block {
        Block { id: 0, task_id, start: start.into(), end: end.into(), locked: false, provider: None, external_id: None, sync_state: None }
    }

    #[test]
    fn picks_todays_events_sorted() {
        let today = date(2026, 6, 15);
        let events = vec![
            event("Standup", "2026-06-15T09:00:00"),
            event("Tomorrow thing", "2026-06-16T09:00:00"),
            event("Lunch", "2026-06-15T12:00:00"),
        ];
        let b = assemble(today, &events, &[], &[]);
        assert_eq!(b.events.iter().map(|e| e.title.as_str()).collect::<Vec<_>>(), vec!["Standup", "Lunch"]);
        assert_eq!(b.weekday, "Monday");
    }

    #[test]
    fn a_month_old_deadline_becomes_stale_not_due() {
        // The bug: `deadline <= today` had no floor, so a task missed in June still led the August
        // briefing as "due" and buried what was actually due.
        let today = date(2026, 8, 25);
        let tasks = vec![
            task("Ancient bike repair", Some("2026-06-14"), "todo"), // 72 days late
            task("Tax form", Some("2026-07-25"), "todo"),            // exactly 31 days late
            task("Essay", Some("2026-08-22"), "todo"),               // 3 days late — still due
            task("Today", Some("2026-08-25T17:00:00"), "scheduled"),
        ];
        let b = assemble(today, &[], &tasks, &[]);
        assert_eq!(
            b.due_tasks.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(),
            vec!["Essay", "Today"],
            "only recently-overdue and today's work stays in the actionable list"
        );
        assert_eq!(
            b.stale_tasks.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(),
            vec!["Ancient bike repair", "Tax form"],
            "oldest first"
        );
    }

    #[test]
    fn the_stale_boundary_is_inclusive_at_the_threshold() {
        // Exactly STALE_AFTER_DAYS late is stale; one day less is still due. Pinning this stops the
        // boundary drifting silently if the constant is ever retuned.
        let today = date(2026, 8, 25);
        let at = (today - chrono::Duration::days(STALE_AFTER_DAYS)).format("%Y-%m-%d").to_string();
        let just_inside = (today - chrono::Duration::days(STALE_AFTER_DAYS - 1)).format("%Y-%m-%d").to_string();
        let tasks = vec![task("At threshold", Some(&at), "todo"), task("Just inside", Some(&just_inside), "todo")];
        let b = assemble(today, &[], &tasks, &[]);
        assert_eq!(b.due_tasks.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(), vec!["Just inside"]);
        assert_eq!(b.stale_tasks.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(), vec!["At threshold"]);
    }

    #[test]
    fn archiving_a_stale_task_removes_it_from_the_briefing() {
        let today = date(2026, 8, 25);
        let tasks = vec![
            task("Let go of this", Some("2026-06-14"), "archived"),
            task("Still owed", Some("2026-06-15"), "todo"),
        ];
        let b = assemble(today, &[], &tasks, &[]);
        assert!(b.due_tasks.is_empty());
        assert_eq!(b.stale_tasks.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(), vec!["Still owed"]);
    }

    #[test]
    fn due_tasks_include_overdue_and_today_not_done() {
        let today = date(2026, 6, 15);
        let tasks = vec![
            task("Overdue essay", Some("2026-06-13"), "todo"),
            task("Due today", Some("2026-06-15T17:00:00"), "scheduled"),
            task("Future", Some("2026-06-20"), "todo"),
            task("Done one", Some("2026-06-14"), "done"),
            task("No deadline", None, "todo"),
        ];
        let b = assemble(today, &[], &tasks, &[]);
        let titles: Vec<_> = b.due_tasks.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, vec!["Overdue essay", "Due today"], "overdue first, future/done/undated excluded");
    }

    #[test]
    fn focus_minutes_sum_todays_blocks_only() {
        let today = date(2026, 6, 15);
        let blocks = vec![
            block(1, "2026-06-15T10:00:00", "2026-06-15T11:30:00"), // 90
            block(2, "2026-06-15T14:00:00", "2026-06-15T14:30:00"), // 30
            block(3, "2026-06-16T10:00:00", "2026-06-16T11:00:00"), // not today
        ];
        assert_eq!(assemble(today, &[], &[], &blocks).focus_minutes, 120);
    }
}
