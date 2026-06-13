//! The auto-scheduler — Pushin's core IP.
//!
//! Deterministic, explainable, fast. Given tasks (durations, deadlines, deps,
//! priorities), fixed events, and locked blocks, it greedily places work into the
//! user's free time using a dependency-aware EDF + priority ordering, then reports
//! conflicts (dependency cycles, over-capacity, deadline misses).
//!
//! `free_slots` is `pub` and reused by the booking module.

use crate::model::{Block, Conflict, ScheduleResult, Settings, Task, DT_FMT};
use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, Timelike};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Interval {
    pub start: NaiveDateTime,
    pub end: NaiveDateTime,
}

impl Interval {
    pub fn minutes(&self) -> i64 {
        (self.end - self.start).num_minutes()
    }
}

pub fn parse_dt(s: &str) -> Option<NaiveDateTime> {
    let t = s.trim();
    // Wall-clock datetime, with or without seconds.
    if let Ok(dt) = NaiveDateTime::parse_from_str(t, DT_FMT) {
        return Some(dt);
    }
    if let Ok(dt) = NaiveDateTime::parse_from_str(t, "%Y-%m-%dT%H:%M") {
        return Some(dt);
    }
    // Models often append a timezone (e.g. "2026-05-31T19:00:00Z" or "+05:00").
    // Treat the written wall-clock as local — strip the zone and reparse.
    let stripped = t.trim_end_matches('Z');
    if stripped != t {
        if let Ok(dt) = NaiveDateTime::parse_from_str(stripped, DT_FMT) {
            return Some(dt);
        }
        if let Ok(dt) = NaiveDateTime::parse_from_str(stripped, "%Y-%m-%dT%H:%M") {
            return Some(dt);
        }
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(t) {
        return Some(dt.naive_local());
    }
    // Date only → treat as end of that day.
    if let Ok(d) = NaiveDate::parse_from_str(t, "%Y-%m-%d") {
        return Some(d.and_hms_opt(23, 59, 0).unwrap());
    }
    None
}

pub fn fmt_dt(dt: NaiveDateTime) -> String {
    dt.format(DT_FMT).to_string()
}

fn parse_hm(s: &str) -> NaiveTime {
    NaiveTime::parse_from_str(s, "%H:%M").unwrap_or_else(|_| NaiveTime::from_hms_opt(9, 0, 0).unwrap())
}

/// Like `parse_hm` but `None` for empty/invalid input (so unset sleep/commitment times are skipped).
fn parse_hm_opt(s: &str) -> Option<NaiveTime> {
    NaiveTime::parse_from_str(s.trim(), "%H:%M").ok()
}

/// Push a recurring daily window starting on `date`. If `end <= start` it spans midnight
/// (e.g. bedtime 23:00 → wake 07:00), so the interval runs into the next day; the per-day
/// sweep in `free_slots` clamps it correctly to each day.
fn push_window(out: &mut Vec<Interval>, date: NaiveDate, start: NaiveTime, end: NaiveTime) {
    let s = date.and_time(start);
    let e = if end <= start {
        (date + Duration::days(1)).and_time(end)
    } else {
        date.and_time(end)
    };
    if e > s {
        out.push(Interval { start: s, end: e });
    }
}

/// Recurring personal commitments — the user's sleep window plus blocked-time/routines —
/// expanded into concrete busy intervals across the horizon. This is what makes the scheduler
/// (and, via `free_slots`, the booking page) plan *around* the user's life.
pub fn personal_busy(now: NaiveDateTime, s: &Settings) -> Vec<Interval> {
    let today = now.date();
    let mut out = Vec::new();
    // Start a day early so an overnight window that began "yesterday" still carves this morning.
    for d in -1..s.horizon_days.max(0) {
        let date = today + Duration::days(d);
        if s.sleep_enabled {
            if let (Some(ss), Some(se)) = (parse_hm_opt(&s.sleep_start), parse_hm_opt(&s.sleep_end)) {
                push_window(&mut out, date, ss, se);
            }
        }
        let wd = date.weekday().number_from_monday() as u8; // 1=Mon..7=Sun
        for c in &s.commitments {
            if !c.days.is_empty() && !c.days.contains(&wd) {
                continue;
            }
            if let (Some(cs), Some(ce)) = (parse_hm_opt(&c.start), parse_hm_opt(&c.end)) {
                push_window(&mut out, date, cs, ce);
            }
        }
    }
    out
}

/// Round a datetime up to the next `step` minute boundary (tidy block starts).
fn round_up(dt: NaiveDateTime, step: i64) -> NaiveDateTime {
    let zeroed = dt.with_second(0).unwrap().with_nanosecond(0).unwrap();
    let rem = zeroed.minute() as i64 % step;
    if rem == 0 && dt.second() == 0 && dt.nanosecond() == 0 {
        zeroed
    } else {
        zeroed - Duration::minutes(rem) + Duration::minutes(step)
    }
}

/// Free intervals within working hours over the horizon, with `busy` (fixed events
/// + locked blocks) carved out and `buffer_minutes` padding around each busy item.
pub fn free_slots(now: NaiveDateTime, s: &Settings, busy: &[Interval]) -> Vec<Interval> {
    let ws = parse_hm(&s.work_start);
    let we_t = parse_hm(&s.work_end);
    let today = now.date();
    let buf = Duration::minutes(s.buffer_minutes.max(0));

    let mut busyx: Vec<Interval> = busy
        .iter()
        .map(|b| Interval { start: b.start - buf, end: b.end + buf })
        .collect();
    // Reserve the user's sleep window and recurring commitments. These are personal time, not
    // tasks, so they get no buffer padding around them.
    busyx.extend(personal_busy(now, s));
    busyx.sort_by_key(|i| i.start);

    let mut out = Vec::new();
    for d in 0..s.horizon_days.max(0) {
        let date = today + Duration::days(d);
        let wd = date.weekday().number_from_monday() as u8; // 1=Mon..7=Sun
        if !s.work_days.contains(&wd) {
            continue;
        }
        let mut day_start = date.and_time(ws);
        let day_end = date.and_time(we_t);
        if day_end <= day_start {
            continue;
        }
        if d == 0 {
            let n = round_up(now, 5);
            if n > day_start {
                day_start = n;
            }
        }
        if day_start >= day_end {
            continue;
        }
        // Sweep busy intervals, emitting the gaps as free time.
        let mut cursor = day_start;
        for b in &busyx {
            if b.end <= cursor {
                continue;
            }
            if b.start >= day_end {
                break;
            }
            let bs = b.start.max(day_start);
            if bs > cursor {
                out.push(Interval { start: cursor, end: bs.min(day_end) });
            }
            cursor = cursor.max(b.end.min(day_end));
            if cursor >= day_end {
                break;
            }
        }
        if cursor < day_end {
            out.push(Interval { start: cursor, end: day_end });
        }
    }
    out.into_iter().filter(|i| i.end > i.start).collect()
}

/// Place up to `remaining` minutes of work into `free` (mutated), no earlier than `earliest` and
/// (when set) no later than `latest` — so a task is never scheduled past its deadline. One block
/// per contiguous interval; `min_chunk` avoids tiny fragments. Returns (placed blocks, last end,
/// minutes that couldn't be placed). Time after `latest` stays in the free pool for other tasks.
fn place(
    free: &mut Vec<Interval>,
    earliest: NaiveDateTime,
    mut remaining: i64,
    min_chunk: i64,
    latest: Option<NaiveDateTime>,
) -> (Vec<Interval>, Option<NaiveDateTime>, i64) {
    let mut placed = Vec::new();
    let mut last_end = None;
    let mut i = 0;
    while remaining > 0 && i < free.len() {
        let iv = free[i];
        if iv.end <= earliest {
            i += 1;
            continue;
        }
        let piece_start = iv.start.max(earliest);
        // Don't let this task's block cross its deadline (but keep the rest of the slot free).
        let piece_end = latest.map_or(iv.end, |l| iv.end.min(l));
        let piece_len = (piece_end - piece_start).num_minutes();
        if piece_len <= 0 {
            i += 1;
            continue;
        }
        let need = remaining.min(piece_len);
        // Avoid leaving a sub-min_chunk fragment, but always allow the final remainder.
        if need < min_chunk.min(remaining) {
            i += 1;
            continue;
        }
        let chunk_end = piece_start + Duration::minutes(need);
        placed.push(Interval { start: piece_start, end: chunk_end });
        remaining -= need;
        last_end = Some(chunk_end);

        let mut repl = Vec::new();
        if piece_start > iv.start {
            repl.push(Interval { start: iv.start, end: piece_start });
        }
        if chunk_end < iv.end {
            repl.push(Interval { start: chunk_end, end: iv.end });
        }
        let n = repl.len();
        free.splice(i..i + 1, repl);
        i += n;
    }
    (placed, last_end, remaining)
}

fn cmp_task(a: &Task, b: &Task) -> Ordering {
    let da = a.deadline.as_deref().and_then(parse_dt);
    let db = b.deadline.as_deref().and_then(parse_dt);
    let by_deadline = match (da, db) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    };
    by_deadline
        .then(b.priority.cmp(&a.priority)) // higher priority first
        .then(a.estimated_minutes.cmp(&b.estimated_minutes))
        .then(a.id.cmp(&b.id))
}

#[allow(clippy::too_many_arguments)]
fn schedule_one(
    t: &Task,
    free: &mut Vec<Interval>,
    now: NaiveDateTime,
    locked_min: &HashMap<i64, i64>,
    scheduled_end: &mut HashMap<i64, NaiveDateTime>,
    ignore_deps: bool,
    blocks: &mut Vec<Block>,
    conflicts: &mut Vec<Conflict>,
) {
    let mut earliest = round_up(now, 5);
    if let Some(es) = &t.earliest_start {
        if let Some(e) = parse_dt(es) {
            if e > earliest {
                earliest = e;
            }
        }
    }
    if !ignore_deps {
        for d in &t.depends_on {
            if let Some(e) = scheduled_end.get(d) {
                if *e > earliest {
                    earliest = *e;
                }
            }
        }
    }

    let est = t.estimated_minutes.max(0);
    let already = *locked_min.get(&t.id).unwrap_or(&0);
    let remaining = (est - already).max(0);
    let min_chunk = t.min_chunk_minutes.max(1);
    // Never schedule a task past its deadline — cap placement at it.
    let deadline = t.deadline.as_deref().and_then(parse_dt);

    let (placed, last_end, left) = place(free, earliest, remaining, min_chunk, deadline);
    for p in &placed {
        blocks.push(Block {
            id: 0,
            task_id: t.id,
            start: fmt_dt(p.start),
            end: fmt_dt(p.end),
            locked: false,
            provider: None,
            external_id: None,
            sync_state: None,
        });
    }
    if let Some(e) = last_end {
        let cur = scheduled_end.entry(t.id).or_insert(e);
        if e > *cur {
            *cur = e;
        }
    }
    // Anything that couldn't be placed: if the task has a deadline, the reason is it won't fit
    // before that deadline (we capped there) → DeadlineMiss; otherwise it's over-capacity.
    if left > 0 {
        match deadline {
            Some(dld) => conflicts.push(Conflict::DeadlineMiss {
                task_id: t.id,
                title: t.title.clone(),
                scheduled_end: fmt_dt(last_end.unwrap_or(dld)),
                deadline: fmt_dt(dld),
            }),
            None => conflicts.push(Conflict::Unschedulable {
                task_id: t.id,
                title: t.title.clone(),
                remaining_minutes: left,
            }),
        }
    }
}

/// Schedule all non-done tasks. `fixed` = immovable events; `locked` = pinned blocks
/// (task_id, interval) that are kept and counted toward their task's progress.
/// Returns NEW blocks (locked ones are preserved by the caller).
pub fn schedule(
    now: NaiveDateTime,
    s: &Settings,
    tasks: &[Task],
    fixed: &[Interval],
    locked: &[(i64, Interval)],
) -> ScheduleResult {
    let active: Vec<&Task> = tasks.iter().filter(|t| t.status != "done").collect();
    let id_set: HashSet<i64> = active.iter().map(|t| t.id).collect();
    let task_by_id: HashMap<i64, &Task> = active.iter().map(|t| (t.id, *t)).collect();

    // Busy = fixed events + locked blocks.
    let mut busy: Vec<Interval> = fixed.to_vec();
    for (_, iv) in locked {
        busy.push(*iv);
    }
    let mut free = free_slots(now, s, &busy);

    // Locked minutes / ends per task.
    let mut locked_min: HashMap<i64, i64> = HashMap::new();
    let mut scheduled_end: HashMap<i64, NaiveDateTime> = HashMap::new();
    for (tid, iv) in locked {
        *locked_min.entry(*tid).or_insert(0) += iv.minutes();
        let e = scheduled_end.entry(*tid).or_insert(iv.end);
        if iv.end > *e {
            *e = iv.end;
        }
    }

    // Dependency graph (edges only between active tasks).
    let mut indeg: HashMap<i64, usize> = active.iter().map(|t| (t.id, 0usize)).collect();
    let mut dependents: HashMap<i64, Vec<i64>> = HashMap::new();
    for t in &active {
        for d in &t.depends_on {
            if id_set.contains(d) {
                *indeg.get_mut(&t.id).unwrap() += 1;
                dependents.entry(*d).or_default().push(t.id);
            }
        }
    }

    let mut conflicts = Vec::new();
    let mut blocks = Vec::new();
    let mut done: HashSet<i64> = HashSet::new();
    let mut ready: Vec<i64> = indeg
        .iter()
        .filter(|(_, &v)| v == 0)
        .map(|(k, _)| *k)
        .collect();

    while !ready.is_empty() {
        ready.sort_by(|a, b| cmp_task(task_by_id[a], task_by_id[b]));
        let tid = ready.remove(0);
        let t = task_by_id[&tid];
        schedule_one(t, &mut free, now, &locked_min, &mut scheduled_end, false, &mut blocks, &mut conflicts);
        done.insert(tid);
        if let Some(deps) = dependents.get(&tid).cloned() {
            for dep in deps {
                let e = indeg.get_mut(&dep).unwrap();
                *e -= 1;
                if *e == 0 {
                    ready.push(dep);
                }
            }
        }
    }

    // Anything left unprocessed is in a dependency cycle: report and place ignoring deps.
    let mut stuck: Vec<i64> = active
        .iter()
        .map(|t| t.id)
        .filter(|id| !done.contains(id))
        .collect();
    if !stuck.is_empty() {
        conflicts.push(Conflict::DependencyCycle { task_ids: stuck.clone() });
        stuck.sort_by(|a, b| cmp_task(task_by_id[a], task_by_id[b]));
        for tid in stuck {
            let t = task_by_id[&tid];
            schedule_one(t, &mut free, now, &locked_min, &mut scheduled_end, true, &mut blocks, &mut conflicts);
        }
    }

    ScheduleResult { blocks, conflicts }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Commitment;

    fn dt(y: i32, m: u32, d: u32, h: u32, min: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(h, min, 0)
            .unwrap()
    }

    fn settings(horizon: i64) -> Settings {
        Settings {
            work_days: vec![1, 2, 3, 4, 5, 6, 7], // all days, so tests are weekday-agnostic
            work_start: "09:00".into(),
            work_end: "17:00".into(),
            horizon_days: horizon,
            buffer_minutes: 0,
            ..Settings::default()
        }
    }

    fn task(id: i64, title: &str, est: i64, deadline: Option<&str>, deps: Vec<i64>) -> Task {
        Task {
            id,
            project_id: None,
            title: title.into(),
            notes: String::new(),
            estimated_minutes: est,
            deadline: deadline.map(|s| s.to_string()),
            earliest_start: None,
            priority: 2,
            min_chunk_minutes: 30,
            max_chunk_minutes: 240,
            status: "todo".into(),
            created_at: String::new(),
            depends_on: deps,
        }
    }

    #[test]
    fn parse_dt_handles_timezone_suffixes() {
        // The LLM sometimes appends Z / an offset / drops seconds — all should resolve
        // to the same wall-clock time rather than being dropped.
        let want = dt(2026, 5, 31, 19, 0);
        assert_eq!(parse_dt("2026-05-31T19:00:00"), Some(want));
        assert_eq!(parse_dt("2026-05-31T19:00:00Z"), Some(want));
        assert_eq!(parse_dt("2026-05-31T19:00"), Some(want));
        assert_eq!(parse_dt("2026-05-31T19:00:00+05:00"), Some(want));
        assert!(parse_dt("sometime next week").is_none());
    }

    #[test]
    fn free_slots_empty_day() {
        let s = settings(1);
        let now = dt(2026, 6, 8, 8, 0);
        let f = free_slots(now, &s, &[]);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].minutes(), 480); // 09:00-17:00
        assert_eq!(f[0].start, dt(2026, 6, 8, 9, 0));
    }

    #[test]
    fn free_slots_carves_fixed_event() {
        let s = settings(1);
        let now = dt(2026, 6, 8, 8, 0);
        let busy = [Interval { start: dt(2026, 6, 8, 12, 0), end: dt(2026, 6, 8, 13, 0) }];
        let f = free_slots(now, &s, &busy);
        assert_eq!(f.len(), 2);
        assert_eq!(f[0], Interval { start: dt(2026, 6, 8, 9, 0), end: dt(2026, 6, 8, 12, 0) });
        assert_eq!(f[1], Interval { start: dt(2026, 6, 8, 13, 0), end: dt(2026, 6, 8, 17, 0) });
    }

    #[test]
    fn free_slots_respects_now() {
        let s = settings(1);
        let now = dt(2026, 6, 8, 10, 30);
        let f = free_slots(now, &s, &[]);
        assert_eq!(f[0].start, dt(2026, 6, 8, 10, 30));
        assert_eq!(f[0].minutes(), 390);
    }

    #[test]
    fn schedules_single_task() {
        let s = settings(1);
        let now = dt(2026, 6, 8, 8, 0);
        let tasks = vec![task(1, "A", 120, None, vec![])];
        let r = schedule(now, &s, &tasks, &[], &[]);
        assert_eq!(r.blocks.len(), 1);
        assert_eq!(r.blocks[0].start, "2026-06-08T09:00:00");
        assert_eq!(r.blocks[0].end, "2026-06-08T11:00:00");
        assert!(r.conflicts.is_empty());
    }

    #[test]
    fn respects_dependencies() {
        let s = settings(1);
        let now = dt(2026, 6, 8, 8, 0);
        // B depends on A; even though B sorts no-earlier, A must come first.
        let tasks = vec![task(1, "A", 60, None, vec![]), task(2, "B", 60, None, vec![1])];
        let r = schedule(now, &s, &tasks, &[], &[]);
        let a_end = r.blocks.iter().find(|b| b.task_id == 1).unwrap().end.clone();
        let b_start = r.blocks.iter().find(|b| b.task_id == 2).unwrap().start.clone();
        assert!(b_start >= a_end, "B should start after A ends ({b_start} >= {a_end})");
    }

    #[test]
    fn earliest_deadline_first() {
        let s = settings(1);
        let now = dt(2026, 6, 8, 8, 0);
        // task 2 has the earlier deadline, so it should be placed first (09:00).
        let tasks = vec![
            task(1, "Later", 60, Some("2026-06-08T17:00:00"), vec![]),
            task(2, "Sooner", 60, Some("2026-06-08T11:00:00"), vec![]),
        ];
        let r = schedule(now, &s, &tasks, &[], &[]);
        let sooner = r.blocks.iter().find(|b| b.task_id == 2).unwrap();
        assert_eq!(sooner.start, "2026-06-08T09:00:00");
    }

    #[test]
    fn flags_over_capacity() {
        let s = settings(1); // only one 8h day
        let now = dt(2026, 6, 8, 8, 0);
        let tasks = vec![task(1, "Huge", 600, None, vec![])]; // 10h > 8h
        let r = schedule(now, &s, &tasks, &[], &[]);
        assert_eq!(r.blocks.iter().map(|b| {
            let st = parse_dt(&b.start).unwrap();
            let en = parse_dt(&b.end).unwrap();
            (en - st).num_minutes()
        }).sum::<i64>(), 480);
        assert!(r.conflicts.iter().any(|c| matches!(c, Conflict::Unschedulable { remaining_minutes, .. } if *remaining_minutes == 120)));
    }

    #[test]
    fn never_schedules_past_deadline() {
        // A task due Tuesday must not get blocks on Wednesday+, even if there's room later.
        let s = settings(7); // a week of capacity
        let now = dt(2026, 6, 8, 8, 0); // Monday
        let deadline = "2026-06-09T23:59:00"; // Tuesday end-of-day
        let tasks = vec![task(1, "Study", 240, Some(deadline), vec![])];
        let r = schedule(now, &s, &tasks, &[], &[]);
        let cap = parse_dt(deadline).unwrap();
        assert!(!r.blocks.is_empty());
        for b in &r.blocks {
            assert!(parse_dt(&b.end).unwrap() <= cap, "block {b:?} runs past the deadline");
        }
    }

    #[test]
    fn deadline_miss_when_it_cannot_fit_in_time() {
        // 8h of work but only ~3h before today-noon → can't fit → DeadlineMiss, and nothing placed
        // after the deadline.
        let s = settings(7);
        let now = dt(2026, 6, 8, 8, 0);
        let deadline = "2026-06-08T12:00:00";
        let tasks = vec![task(1, "Cram", 480, Some(deadline), vec![])];
        let r = schedule(now, &s, &tasks, &[], &[]);
        assert!(r.conflicts.iter().any(|c| matches!(c, Conflict::DeadlineMiss { .. })));
        let cap = parse_dt(deadline).unwrap();
        for b in &r.blocks {
            assert!(parse_dt(&b.end).unwrap() <= cap);
        }
    }

    #[test]
    fn flags_deadline_miss() {
        let s = settings(3);
        let now = dt(2026, 6, 8, 8, 0);
        // 8h of work but deadline is today noon -> can't finish in time.
        let tasks = vec![task(1, "Tight", 480, Some("2026-06-08T12:00:00"), vec![])];
        let r = schedule(now, &s, &tasks, &[], &[]);
        assert!(r.conflicts.iter().any(|c| matches!(c, Conflict::DeadlineMiss { .. })));
    }

    #[test]
    fn detects_dependency_cycle() {
        let s = settings(1);
        let now = dt(2026, 6, 8, 8, 0);
        let tasks = vec![task(1, "A", 60, None, vec![2]), task(2, "B", 60, None, vec![1])];
        let r = schedule(now, &s, &tasks, &[], &[]);
        assert!(r.conflicts.iter().any(|c| matches!(c, Conflict::DependencyCycle { .. })));
        // Cyclic tasks are still placed (deps ignored) so the user isn't left empty-handed.
        assert_eq!(r.blocks.len(), 2);
    }

    #[test]
    fn no_blocks_overlap_each_other_or_fixed() {
        let s = settings(2);
        let now = dt(2026, 6, 8, 8, 0);
        let fixed = [Interval { start: dt(2026, 6, 8, 12, 0), end: dt(2026, 6, 8, 13, 0) }];
        let tasks = vec![
            task(1, "A", 180, None, vec![]),
            task(2, "B", 200, None, vec![]),
            task(3, "C", 150, None, vec![]),
        ];
        let r = schedule(now, &s, &tasks, &fixed, &[]);
        let mut ivs: Vec<Interval> = r
            .blocks
            .iter()
            .map(|b| Interval { start: parse_dt(&b.start).unwrap(), end: parse_dt(&b.end).unwrap() })
            .collect();
        ivs.push(fixed[0]);
        ivs.sort_by_key(|i| i.start);
        for w in ivs.windows(2) {
            assert!(w[0].end <= w[1].start, "overlap: {:?} vs {:?}", w[0], w[1]);
        }
    }

    #[test]
    fn sleep_window_is_kept_free() {
        let mut s = settings(1);
        s.work_start = "06:00".into();
        s.work_end = "23:00".into();
        s.sleep_enabled = true;
        s.sleep_start = "22:00".into();
        s.sleep_end = "07:00".into();
        // Sleep 22:00→07:00 should carve the early morning and late evening from the work window,
        // leaving only 07:00–22:00 free.
        let now = dt(2026, 6, 8, 5, 0);
        let f = free_slots(now, &s, &[]);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].start, dt(2026, 6, 8, 7, 0));
        assert_eq!(f[0].end, dt(2026, 6, 8, 22, 0));
    }

    #[test]
    fn commitment_blocks_its_window() {
        let mut s = settings(1);
        s.sleep_enabled = false;
        s.commitments = vec![Commitment {
            id: "1".into(),
            name: "Lunch".into(),
            start: "12:00".into(),
            end: "13:00".into(),
            days: vec![],
            kind: "blocked".into(),
        }];
        let now = dt(2026, 6, 8, 8, 0);
        // A task big enough to span lunch — none of it may land in the 12:00–13:00 window.
        let tasks = vec![task(1, "A", 480, None, vec![])];
        let r = schedule(now, &s, &tasks, &[], &[]);
        assert!(!r.blocks.is_empty());
        for b in &r.blocks {
            let bs = parse_dt(&b.start).unwrap();
            let be = parse_dt(&b.end).unwrap();
            assert!(
                be <= dt(2026, 6, 8, 12, 0) || bs >= dt(2026, 6, 8, 13, 0),
                "block overlaps the lunch commitment: {b:?}"
            );
        }
    }

    #[test]
    fn commitment_only_blocks_its_weekdays() {
        let mut s = settings(1);
        s.sleep_enabled = false;
        // 2026-06-08 is a Monday; this commitment is Tuesdays-only, so Monday is untouched.
        s.commitments = vec![Commitment {
            id: "1".into(),
            name: "Standup".into(),
            start: "12:00".into(),
            end: "13:00".into(),
            days: vec![2],
            kind: "blocked".into(),
        }];
        let now = dt(2026, 6, 8, 8, 0);
        let f = free_slots(now, &s, &[]);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0], Interval { start: dt(2026, 6, 8, 9, 0), end: dt(2026, 6, 8, 17, 0) });
    }

    #[test]
    fn locked_block_counts_toward_progress() {
        let s = settings(1);
        let now = dt(2026, 6, 8, 8, 0);
        let tasks = vec![task(1, "A", 120, None, vec![])];
        // 60min already locked 09:00-10:00 -> only 60 more should be scheduled.
        let locked = [(1i64, Interval { start: dt(2026, 6, 8, 9, 0), end: dt(2026, 6, 8, 10, 0) })];
        let r = schedule(now, &s, &tasks, &[], &locked);
        let total: i64 = r.blocks.iter().map(|b| (parse_dt(&b.end).unwrap() - parse_dt(&b.start).unwrap()).num_minutes()).sum();
        assert_eq!(total, 60);
        // and the new block can't overlap the locked one
        for b in &r.blocks {
            assert!(parse_dt(&b.start).unwrap() >= dt(2026, 6, 8, 10, 0));
        }
    }
}
