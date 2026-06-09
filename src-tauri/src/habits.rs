//! Habit metrics — streaks and consistency derived from a habit's set of completed days.
//! Pure date arithmetic (no DB), so it's unit-testable without a model or a database.

use crate::model::{Event, Habit, HabitDay, HabitStats};
use crate::scheduler::{parse_dt, Interval};
use chrono::{Duration, NaiveDate, NaiveDateTime};
use std::collections::HashSet;

/// Is this habit already on the calendar for `day`? Used so "Add to today" can only place a
/// habit once per day instead of stacking duplicates.
pub fn habit_already_on_day(events: &[Event], name: &str, day: NaiveDate) -> bool {
    events.iter().any(|e| {
        e.kind == "habit"
            && e.title.eq_ignore_ascii_case(name)
            && parse_dt(&e.start).map(|d| d.date()) == Some(day)
    })
}

/// How many days of history the heatmap shows (17 weeks).
const HISTORY_DAYS: i64 = 17 * 7;
/// Window for the consistency percentage.
const RATE_WINDOW_DAYS: i64 = 30;

/// Build the metrics for one habit from the days it was completed.
pub fn compute_stats(habit: &Habit, done: &HashSet<NaiveDate>, today: NaiveDate) -> HabitStats {
    HabitStats {
        id: habit.id,
        name: habit.name.clone(),
        color: habit.color.clone(),
        cadence: habit.cadence.clone(),
        duration_minutes: habit.duration_minutes,
        created_at: habit.created_at.clone(),
        done_today: done.contains(&today),
        current_streak: current_streak(done, today),
        longest_streak: longest_streak(done),
        completion_rate: completion_rate(habit, done, today),
        total_done: done.len() as i64,
        scheduled_days: 0, // filled in by commands::habit_stats (needs the events table)
        history: history(done, today),
    }
}

/// Find when to drop a habit of `duration_min` onto a day, given that day's `busy`
/// intervals (events + task blocks) and the awake `[window_start, window_end)` window.
///
/// Strategy: **best-fit** — pick the *smallest* free gap the habit still fits in (ties broken
/// toward the earliest). This tucks the habit into the awkward little gaps between existing
/// commitments and leaves the big open stretches intact for deep-work tasks, instead of
/// dumping it at the end of the day. Within the chosen gap it hugs an adjacent commitment:
/// start-aligned right after the thing before it, or end-aligned right before the thing after
/// it when the gap only touches a commitment on its later side. A wide-open day has no
/// commitment to hug, so the habit lands at the start of the window (morning). If the day is
/// completely packed it falls back to the end of the window (a last-resort overlap).
/// Returns `None` only if the window itself is empty (e.g. it's already past end-of-day).
pub fn find_habit_slot(
    busy: &[Interval],
    window_start: NaiveDateTime,
    window_end: NaiveDateTime,
    duration_min: i64,
) -> Option<(NaiveDateTime, NaiveDateTime)> {
    if window_end <= window_start {
        return None;
    }
    let dur = Duration::minutes(duration_min.max(1));

    // Free gaps within the window (sweep the busy intervals clipped to it).
    let mut b: Vec<Interval> = busy.iter().filter(|iv| iv.end > window_start && iv.start < window_end).copied().collect();
    b.sort_by_key(|iv| iv.start);
    let mut gaps = Vec::new();
    let mut cursor = window_start;
    for iv in &b {
        let s = iv.start.max(window_start);
        if s > cursor {
            gaps.push(Interval { start: cursor, end: s });
        }
        cursor = cursor.max(iv.end.min(window_end));
        if cursor >= window_end {
            break;
        }
    }
    if cursor < window_end {
        gaps.push(Interval { start: cursor, end: window_end });
    }

    // Best-fit: smallest gap that still fits, earliest as the tie-breaker (gaps are already
    // in chronological order, so a stable min-by keeps the earlier one on ties).
    let best = gaps
        .iter()
        .filter(|g| g.minutes() >= duration_min)
        .min_by_key(|g| g.minutes());
    if let Some(g) = best {
        // Hug a real commitment: abut whatever the gap touches, preferring its earlier edge.
        // A gap flush against the window edge on its early side (and a commitment on its late
        // side) end-aligns; otherwise start-align (including the empty-day → morning case).
        let touches_earlier = g.start > window_start;
        let touches_later = g.end < window_end;
        return Some(if !touches_earlier && touches_later {
            (g.end - dur, g.end)
        } else {
            (g.start, g.start + dur)
        });
    }
    // Packed day: last resort, pin it to the end of the window.
    Some((window_end - dur, window_end))
}

/// Consecutive completed days ending today, or ending yesterday if today isn't done yet
/// (so an unbroken run still shows while today is "pending"). 0 if the last completion is
/// older than yesterday.
fn current_streak(done: &HashSet<NaiveDate>, today: NaiveDate) -> i64 {
    let mut cursor = if done.contains(&today) {
        today
    } else if done.contains(&(today - Duration::days(1))) {
        today - Duration::days(1)
    } else {
        return 0;
    };
    let mut count = 0;
    while done.contains(&cursor) {
        count += 1;
        cursor -= Duration::days(1);
    }
    count
}

/// The longest unbroken run anywhere in the habit's history.
fn longest_streak(done: &HashSet<NaiveDate>) -> i64 {
    let mut longest = 0;
    for &d in done {
        // Only count from the start of a run (the day before is missing).
        if done.contains(&(d - Duration::days(1))) {
            continue;
        }
        let mut len = 0;
        let mut cursor = d;
        while done.contains(&cursor) {
            len += 1;
            cursor += Duration::days(1);
        }
        longest = longest.max(len);
    }
    longest
}

/// Fraction of the last 30 days (or since creation, if younger) that were completed.
fn completion_rate(habit: &Habit, done: &HashSet<NaiveDate>, today: NaiveDate) -> f64 {
    let created = habit
        .created_at
        .get(..10)
        .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
        .unwrap_or(today);
    let since_created = (today - created).num_days() + 1;
    let window = since_created.clamp(1, RATE_WINDOW_DAYS);
    let hits = (0..window).filter(|i| done.contains(&(today - Duration::days(*i)))).count() as f64;
    hits / window as f64
}

/// Contiguous days, oldest → today, each flagged done/not — drives the heatmap.
fn history(done: &HashSet<NaiveDate>, today: NaiveDate) -> Vec<HabitDay> {
    (0..HISTORY_DAYS)
        .rev()
        .map(|i| {
            let d = today - Duration::days(i);
            HabitDay { day: d.format("%Y-%m-%d").to_string(), done: done.contains(&d) }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn habit(created: &str) -> Habit {
        Habit {
            id: 1,
            name: "Read".into(),
            color: "#22c55e".into(),
            cadence: "daily".into(),
            duration_minutes: 30,
            archived: false,
            created_at: format!("{created}T08:00:00"),
        }
    }

    fn days(today: NaiveDate, offsets: &[i64]) -> HashSet<NaiveDate> {
        offsets.iter().map(|o| today - Duration::days(*o)).collect()
    }

    #[test]
    fn streak_counts_run_ending_today() {
        let today = NaiveDate::from_ymd_opt(2026, 6, 10).unwrap();
        let done = days(today, &[0, 1, 2, 4]); // today, -1, -2, (gap), -4
        assert_eq!(current_streak(&done, today), 3);
    }

    #[test]
    fn streak_survives_a_pending_today() {
        let today = NaiveDate::from_ymd_opt(2026, 6, 10).unwrap();
        let done = days(today, &[1, 2, 3]); // not today, but yesterday + back
        assert_eq!(current_streak(&done, today), 3);
    }

    #[test]
    fn streak_breaks_after_a_missed_day() {
        let today = NaiveDate::from_ymd_opt(2026, 6, 10).unwrap();
        let done = days(today, &[2, 3, 4]); // last completion was 2 days ago
        assert_eq!(current_streak(&done, today), 0);
    }

    #[test]
    fn longest_streak_scans_whole_history() {
        let today = NaiveDate::from_ymd_opt(2026, 6, 10).unwrap();
        let done = days(today, &[0, 1, 5, 6, 7, 8, 20]); // runs of 2, 4, 1
        assert_eq!(longest_streak(&done), 4);
    }

    #[test]
    fn completion_rate_uses_30_day_window() {
        let today = NaiveDate::from_ymd_opt(2026, 6, 10).unwrap();
        let h = habit("2026-01-01"); // older than the window
        let done = days(today, &(0..15).collect::<Vec<_>>()); // 15 of the last 30 days
        assert!((completion_rate(&h, &done, today) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn completion_rate_prorates_for_a_young_habit() {
        let today = NaiveDate::from_ymd_opt(2026, 6, 10).unwrap();
        let h = habit("2026-06-08"); // created 3 days ago → window of 3
        let done = days(today, &[0, 1]); // 2 of those 3 days
        assert!((completion_rate(&h, &done, today) - 2.0 / 3.0).abs() < 1e-9);
    }

    fn dt(h: u32, m: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 6, 10).unwrap().and_hms_opt(h, m, 0).unwrap()
    }

    #[test]
    fn habit_slot_lands_in_the_morning_on_an_empty_day() {
        // Empty 07:00–22:00 window: no commitment to hug, so a 30-min habit starts the day
        // (07:00) rather than getting pinned to the end of the window.
        let slot = find_habit_slot(&[], dt(7, 0), dt(22, 0), 30).unwrap();
        assert_eq!(slot, (dt(7, 0), dt(7, 30)));
    }

    #[test]
    fn habit_slot_tucks_into_the_smallest_fitting_gap() {
        // Morning meeting 09:00–12:00 and afternoon 12:45–17:00 leave gaps of 120/45/300 min.
        // A 30-min habit takes the tight lunch gap (best-fit) and abuts the 12:00 meeting end,
        // keeping the long morning and evening stretches open for tasks.
        let busy = [
            Interval { start: dt(9, 0), end: dt(12, 0) },
            Interval { start: dt(12, 45), end: dt(17, 0) },
        ];
        let slot = find_habit_slot(&busy, dt(7, 0), dt(22, 0), 30).unwrap();
        assert_eq!(slot, (dt(12, 0), dt(12, 30)));
    }

    #[test]
    fn habit_slot_hugs_a_later_commitment_when_the_gap_starts_at_the_window_edge() {
        // Evening blocked 20:00–22:00; the only gap (07:00–20:00) is flush with the window's
        // start, so a 60-min habit hugs the 20:00 commitment instead of floating at 07:00.
        let busy = [Interval { start: dt(20, 0), end: dt(22, 0) }];
        let slot = find_habit_slot(&busy, dt(7, 0), dt(22, 0), 60).unwrap();
        assert_eq!(slot, (dt(19, 0), dt(20, 0)));
    }

    #[test]
    fn habit_slot_skips_a_gap_too_small_to_fit() {
        // A small late gap (21:30–22:00, 30m) can't hold a 60-min habit, so it falls back to
        // the only other (earlier) gap, hugging the 20:30 commitment.
        let busy = [
            Interval { start: dt(20, 30), end: dt(21, 30) },
        ];
        let slot = find_habit_slot(&busy, dt(7, 0), dt(22, 0), 60).unwrap();
        assert_eq!(slot, (dt(19, 30), dt(20, 30)));
    }

    #[test]
    fn habit_slot_none_when_window_already_passed() {
        assert!(find_habit_slot(&[], dt(22, 0), dt(22, 0), 30).is_none());
    }

    #[test]
    fn detects_habit_already_on_a_day() {
        let mk = |title: &str, kind: &str, start: &str| Event {
            id: 1,
            title: title.into(),
            start: start.into(),
            end: "2026-06-10T21:00:00".into(),
            kind: kind.into(),
            source: "manual".into(),
            created_at: String::new(),
            provider: None,
            external_id: None,
            account_id: None,
            etag: None,
        };
        let day = NaiveDate::from_ymd_opt(2026, 6, 10).unwrap();
        let events = vec![
            mk("Read", "habit", "2026-06-10T20:00:00"),
            mk("Read", "fixed", "2026-06-10T08:00:00"), // a normal event, not a habit instance
        ];
        assert!(habit_already_on_day(&events, "Read", day)); // same habit, same day
        assert!(habit_already_on_day(&events, "read", day)); // case-insensitive
        assert!(!habit_already_on_day(&events, "Stretch", day)); // different habit
        assert!(!habit_already_on_day(&events, "Read", day.succ_opt().unwrap())); // different day
    }

    #[test]
    fn stats_roll_up_correctly() {
        let today = NaiveDate::from_ymd_opt(2026, 6, 10).unwrap();
        let h = habit("2026-05-01");
        let done = days(today, &[0, 1, 2]);
        let s = compute_stats(&h, &done, today);
        assert!(s.done_today);
        assert_eq!(s.current_streak, 3);
        assert_eq!(s.total_done, 3);
        assert_eq!(s.history.len() as i64, HISTORY_DAYS);
        assert_eq!(s.history.last().unwrap().day, "2026-06-10"); // newest is today
        assert!(s.history.last().unwrap().done);
    }
}
