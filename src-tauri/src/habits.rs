//! Habit metrics — streaks and consistency derived from a habit's set of completed days.
//! Pure date arithmetic (no DB), so it's unit-testable without a model or a database.

use crate::model::{Event, Habit, HabitDay, HabitStats};
use crate::scheduler::{parse_dt, Interval};
use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime};
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

/// Build the metrics for one habit from the days it was completed. Streaks and consistency are
/// measured over the habit's **due** days (per its cadence), so a Mon/Wed habit isn't "broken" by
/// an un-completed Tuesday.
pub fn compute_stats(habit: &Habit, done: &HashSet<NaiveDate>, today: NaiveDate) -> HabitStats {
    HabitStats {
        id: habit.id,
        name: habit.name.clone(),
        color: habit.color.clone(),
        cadence: habit.cadence.clone(),
        days: habit.days.clone(),
        interval_days: habit.interval_days.max(1),
        duration_minutes: habit.duration_minutes,
        created_at: habit.created_at.clone(),
        done_today: done.contains(&today),
        current_streak: current_streak(habit, done, today),
        longest_streak: longest_streak(habit, done),
        completion_rate: completion_rate(habit, done, today),
        total_done: done.len() as i64,
        scheduled_days: 0, // filled in by commands::habit_stats (needs the events table)
        history: history(habit, done, today),
    }
}

/// The habit's creation calendar date (anchor for interval cadences).
fn created_date(habit: &Habit) -> Option<NaiveDate> {
    habit.created_at.get(..10).and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
}

/// Is the habit expected on `date`, per its cadence? daily = always; weekly = the weekday is in
/// `days`; interval = `interval_days` apart from creation. Unknown cadence falls back to daily.
pub fn is_due(habit: &Habit, date: NaiveDate) -> bool {
    match habit.cadence.as_str() {
        "weekly" => {
            let wd = date.weekday().number_from_monday() as u8; // 1=Mon..7=Sun
            !habit.days.is_empty() && habit.days.contains(&wd)
        }
        "interval" => {
            let n = habit.interval_days.max(1);
            if n <= 1 {
                return true;
            }
            created_date(habit).map_or(true, |c| date >= c && (date - c).num_days() % n == 0)
        }
        _ => true,
    }
}

/// The nearest due day strictly before `date` (bounded scan), or None.
fn prev_due(habit: &Habit, date: NaiveDate) -> Option<NaiveDate> {
    let mut d = date - Duration::days(1);
    for _ in 0..400 {
        if is_due(habit, d) {
            return Some(d);
        }
        d -= Duration::days(1);
    }
    None
}

/// The nearest due day strictly after `date` (bounded scan), or None.
fn next_due(habit: &Habit, date: NaiveDate) -> Option<NaiveDate> {
    let mut d = date + Duration::days(1);
    for _ in 0..400 {
        if is_due(habit, d) {
            return Some(d);
        }
        d += Duration::days(1);
    }
    None
}

/// The latest due day on or before `date` (bounded scan), or None.
fn due_on_or_before(habit: &Habit, date: NaiveDate) -> Option<NaiveDate> {
    let mut d = date;
    for _ in 0..400 {
        if is_due(habit, d) {
            return Some(d);
        }
        d -= Duration::days(1);
    }
    None
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

/// Consecutive completed **due** days ending at the latest due day on/before today — or, if today
/// is due but still pending, ending at the previous due day (so an unbroken run shows while today
/// is "pending"). 0 if the most recent due day was missed.
fn current_streak(habit: &Habit, done: &HashSet<NaiveDate>, today: NaiveDate) -> i64 {
    let start = if is_due(habit, today) {
        if done.contains(&today) {
            Some(today)
        } else {
            prev_due(habit, today) // today due but pending → count up to the previous due day
        }
    } else {
        due_on_or_before(habit, today)
    };
    let mut cursor = match start {
        Some(d) => d,
        None => return 0,
    };
    let mut count = 0;
    loop {
        if !done.contains(&cursor) {
            break;
        }
        count += 1;
        match prev_due(habit, cursor) {
            Some(p) => cursor = p,
            None => break,
        }
    }
    count
}

/// The longest unbroken run of completed **due** days anywhere in the habit's history.
fn longest_streak(habit: &Habit, done: &HashSet<NaiveDate>) -> i64 {
    let mut longest = 0;
    for &d in done {
        if !is_due(habit, d) {
            continue; // only completions on due days count toward a cadence streak
        }
        // Only start from the beginning of a run (the previous due day is missing/not done).
        if let Some(p) = prev_due(habit, d) {
            if done.contains(&p) {
                continue;
            }
        }
        let mut len = 0;
        let mut cursor = Some(d);
        while let Some(c) = cursor {
            if !done.contains(&c) {
                break;
            }
            len += 1;
            cursor = next_due(habit, c);
        }
        longest = longest.max(len);
    }
    longest
}

/// Fraction of the habit's **due** days in the last 30 days (or since creation, if younger) that
/// were completed. A habit with no due days in the window reports 0.
fn completion_rate(habit: &Habit, done: &HashSet<NaiveDate>, today: NaiveDate) -> f64 {
    let created = created_date(habit).unwrap_or(today);
    let since_created = (today - created).num_days() + 1;
    let window = since_created.clamp(1, RATE_WINDOW_DAYS);
    let (mut due, mut hits) = (0i64, 0i64);
    for i in 0..window {
        let d = today - Duration::days(i);
        if is_due(habit, d) {
            due += 1;
            if done.contains(&d) {
                hits += 1;
            }
        }
    }
    if due == 0 {
        0.0
    } else {
        hits as f64 / due as f64
    }
}

/// Contiguous days, oldest → today, each flagged done + due — drives the heatmap (non-due days
/// render dimmed so the cadence reads visually).
fn history(habit: &Habit, done: &HashSet<NaiveDate>, today: NaiveDate) -> Vec<HabitDay> {
    (0..HISTORY_DAYS)
        .rev()
        .map(|i| {
            let d = today - Duration::days(i);
            HabitDay { day: d.format("%Y-%m-%d").to_string(), done: done.contains(&d), due: is_due(habit, d) }
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
            days: vec![],
            interval_days: 1,
            duration_minutes: 30,
            archived: false,
            created_at: format!("{created}T08:00:00"),
            preferred_minute: None,
        }
    }
    fn weekly(created: &str, days: &[u8]) -> Habit {
        Habit { cadence: "weekly".into(), days: days.to_vec(), ..habit(created) }
    }
    fn interval(created: &str, n: i64) -> Habit {
        Habit { cadence: "interval".into(), interval_days: n, ..habit(created) }
    }
    const DAILY: fn() -> Habit = || Habit {
        id: 1,
        name: "Read".into(),
        color: "#22c55e".into(),
        cadence: "daily".into(),
        days: vec![],
        interval_days: 1,
        duration_minutes: 30,
        archived: false,
        created_at: "2025-01-01T08:00:00".into(),
        preferred_minute: None,
    };

    fn days(today: NaiveDate, offsets: &[i64]) -> HashSet<NaiveDate> {
        offsets.iter().map(|o| today - Duration::days(*o)).collect()
    }

    #[test]
    fn streak_counts_run_ending_today() {
        let today = NaiveDate::from_ymd_opt(2026, 6, 10).unwrap();
        let done = days(today, &[0, 1, 2, 4]); // today, -1, -2, (gap), -4
        assert_eq!(current_streak(&DAILY(), &done, today), 3);
    }

    #[test]
    fn streak_survives_a_pending_today() {
        let today = NaiveDate::from_ymd_opt(2026, 6, 10).unwrap();
        let done = days(today, &[1, 2, 3]); // not today, but yesterday + back
        assert_eq!(current_streak(&DAILY(), &done, today), 3);
    }

    #[test]
    fn streak_breaks_after_a_missed_day() {
        let today = NaiveDate::from_ymd_opt(2026, 6, 10).unwrap();
        let done = days(today, &[2, 3, 4]); // last completion was 2 days ago
        assert_eq!(current_streak(&DAILY(), &done, today), 0);
    }

    #[test]
    fn longest_streak_scans_whole_history() {
        let today = NaiveDate::from_ymd_opt(2026, 6, 10).unwrap();
        let done = days(today, &[0, 1, 5, 6, 7, 8, 20]); // runs of 2, 4, 1
        assert_eq!(longest_streak(&DAILY(), &done), 4);
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

    #[test]
    fn is_due_honors_cadence() {
        // 2026-06-08 is a Monday.
        let mon = NaiveDate::from_ymd_opt(2026, 6, 8).unwrap();
        let tue = NaiveDate::from_ymd_opt(2026, 6, 9).unwrap();
        let wed = NaiveDate::from_ymd_opt(2026, 6, 10).unwrap();
        let mw = weekly("2026-01-01", &[1, 3]); // Mon & Wed
        assert!(is_due(&mw, mon) && !is_due(&mw, tue) && is_due(&mw, wed));

        let eod = interval("2026-06-08", 2); // every other day from Mon
        assert!(is_due(&eod, mon) && !is_due(&eod, tue) && is_due(&eod, wed));
        assert!(!is_due(&eod, NaiveDate::from_ymd_opt(2026, 6, 7).unwrap())); // before creation

        assert!(is_due(&habit("2026-01-01"), tue)); // daily is always due
    }

    #[test]
    fn weekly_streak_ignores_off_days() {
        let wed = NaiveDate::from_ymd_opt(2026, 6, 10).unwrap(); // a Wednesday
        let mon = NaiveDate::from_ymd_opt(2026, 6, 8).unwrap();
        let prev_wed = NaiveDate::from_ymd_opt(2026, 6, 3).unwrap();
        // Created on the first completed due day so the rate window covers exactly its due days.
        let mw = weekly("2026-06-03", &[1, 3]);
        // Completed Wed + the Mon before + the Wed before → a 3-long streak across off-days.
        let done: HashSet<NaiveDate> = [wed, mon, prev_wed].into_iter().collect();
        let s = compute_stats(&mw, &done, wed);
        assert_eq!(s.current_streak, 3); // a missed Tuesday must NOT break it
        // Consistency is over DUE days only — all 3 due days since creation were done → 100%.
        assert!((s.completion_rate - 1.0).abs() < 1e-9, "rate was {}", s.completion_rate);
        // The heatmap marks Tuesday not-due.
        assert!(s.history.iter().any(|d| d.day == "2026-06-09" && !d.due));
    }

    #[test]
    fn interval_streak_counts_every_other_day() {
        let today = NaiveDate::from_ymd_opt(2026, 6, 10).unwrap();
        let h = interval("2026-06-02", 2); // due 6/2,6/4,6/6,6/8,6/10,...
        let done = days(today, &[0, 2, 4]); // 6/10, 6/8, 6/6 — three due days in a row
        assert_eq!(current_streak(&h, &done, today), 3);
        // A completion on an off day (6/9) doesn't extend the streak.
        let mut done2 = done.clone();
        done2.insert(today - Duration::days(1));
        assert_eq!(current_streak(&h, &done2, today), 3);
    }
}
