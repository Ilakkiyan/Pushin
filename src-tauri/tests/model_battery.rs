//! Model regression battery **with a UI projection**. Unlike `llm_eval.rs` (which only asserts on DB
//! rows), this prints, per case, a readable text "screen" of what the user would actually SEE after the
//! command: the chat reply, AND the resulting calendar (events + scheduled task-blocks at their times),
//! plus the task and habit lists. That makes subtly-wrong outputs (wrong time, duplicate block, a
//! clarification when it should have acted, a botched multi-day span) visible to a human/LLM reviewer
//! even when there's no crisp assertion.
//!
//! It's the **pre-push model gate**: run it before shipping any model / parser / prompt change.
//!
//! Each case runs the *full* real path — `parser::plan` (live model) → `parser::store_plan` (dedupe,
//! edit-reconcile, date resolution) → `schedule_service::reschedule_inner` (the same scheduler pass the
//! app runs after a plan, so task-blocks land on the calendar) — against a throwaway SQLite DB.
//!
//! `#[ignore]`d and self-skips when no server is reachable, so `cargo test` stays green. Run it with a
//! live model:
//!
//! ```text
//! # App open → a llama-server is on :8080. Then, from src-tauri/:
//! cargo test --test model_battery -- --ignored --nocapture
//!
//! # Or point at Ollama (OpenAI-compatible) — note: base URL WITHOUT the /v1 suffix:
//! PUSHIN_LLM_URL=http://localhost:11434 PUSHIN_LLM_MODEL=qwen2.5:3b \
//!   cargo test --test model_battery -- --ignored --nocapture
//! ```
//!
//! Output goes to stdout AND a full report at `target/model-battery/report.md`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::time::Duration;

use chrono::{Datelike, Duration as ChronoDuration, Local, NaiveDate, NaiveDateTime, NaiveTime, Timelike};
use pushin_lib::db;
use pushin_lib::model::{Conflict, Event, Settings};
use pushin_lib::parser::{self, ChatTurn, PlanOutcome};
use pushin_lib::schedule_service;
use rusqlite::Connection;

// ============================ expectations (optional crisp checks) ============================
// Many "weird" cases have no single right answer — those carry only a `note` (what good looks like)
// and are judged from the printed screen. Where truth IS crisp, these assert it.
#[derive(Default)]
struct Expect {
    events: Option<usize>,
    min_events: Option<usize>,
    tasks: Option<usize>,
    min_tasks: Option<usize>,
    habits: Option<usize>,
    min_habits: Option<usize>,
    min_updated: Option<usize>,
    min_removed: Option<usize>,
    title_has: &'static [&'static str],
    habit_has: &'static [&'static str],
    removed_has: &'static [&'static str],
    survives: &'static [&'static str],
    /// (needle, hour, minute) — the matching event starts at exactly h:m. Needle "" = first event.
    event_start_hm: &'static [(&'static str, u32, u32)],
    /// (needle, minutes) — the matching event's duration ≈ minutes (±1).
    event_minutes: &'static [(&'static str, i64)],
    /// (needle, day_offset) — the matching event's start date == today + offset.
    event_day_offset: &'static [(&'static str, i64)],
    /// Each value (1 low .. 4 urgent) must appear among created tasks' priorities.
    priorities: &'static [i64],
    has_task_dep: bool,
    /// Restraint: must create no event / task / habit.
    created_nothing: bool,
    /// A scheduling conflict (unschedulable / deadline-miss / cycle) must surface.
    has_conflict: bool,
}

struct Case {
    name: &'static str,
    category: &'static str,
    /// What a *good* result looks like — guidance for eye-judging the printed screen.
    note: &'static str,
    history: &'static [(&'static str, &'static str)],
    seed: &'static [(&'static str, i64, (u32, u32), i64, (u32, u32))], // (title, sdo,(h,m), edo,(h,m))
    prompt: &'static str,
    expect: Expect,
}

// ================================ helpers ================================
fn chk(label: impl Into<String>, cond: bool) -> (String, bool) {
    (label.into(), cond)
}
fn iso(day_off: i64, h: u32, m: u32) -> String {
    let d = (Local::now().naive_local().date() + ChronoDuration::days(day_off)).and_hms_opt(h, m, 0).unwrap();
    d.format("%Y-%m-%dT%H:%M:%S").to_string()
}
fn parse(s: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").ok()
}
fn near(actual: Option<i64>, want: i64) -> bool {
    actual.map(|a| (a - want).abs() <= 1).unwrap_or(false)
}
fn ev_span(conn: &Connection, needle: &str) -> Option<(NaiveDateTime, NaiveDateTime)> {
    let n = needle.to_lowercase();
    let e = db::list_events(conn).ok()?.into_iter().find(|e| e.title.to_lowercase().contains(&n))?;
    Some((parse(&e.start)?, parse(&e.end)?))
}
fn ev_minutes(conn: &Connection, needle: &str) -> Option<i64> {
    ev_span(conn, needle).map(|(s, e)| (e - s).num_minutes())
}
fn ev_start_hm(conn: &Connection, needle: &str) -> Option<(u32, u32)> {
    ev_span(conn, needle).map(|(s, _)| (s.hour(), s.minute()))
}
fn ev_start_date(conn: &Connection, needle: &str) -> Option<NaiveDate> {
    ev_span(conn, needle).map(|(s, _)| s.date())
}
fn ev_exists(conn: &Connection, needle: &str) -> bool {
    let n = needle.to_lowercase();
    db::list_events(conn).map(|v| v.iter().any(|e| e.title.to_lowercase().contains(&n))).unwrap_or(false)
}
fn title_in(o: &PlanOutcome, needle: &str) -> bool {
    let n = needle.to_lowercase();
    o.created_event_titles.iter().chain(o.updated_event_titles.iter()).any(|t| t.to_lowercase().contains(&n))
}

fn evaluate(e: &Expect, o: &PlanOutcome, conflicts: &[Conflict], conn: &Connection) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    if let Some(n) = e.events { out.push(chk(format!("created {n} event(s)"), o.created_event_titles.len() == n)); }
    if let Some(n) = e.min_events { out.push(chk(format!("≥{n} event(s)"), o.created_event_titles.len() >= n)); }
    if let Some(n) = e.tasks { out.push(chk(format!("created {n} task(s)"), o.created_task_ids.len() == n)); }
    if let Some(n) = e.min_tasks { out.push(chk(format!("≥{n} task(s)"), o.created_task_ids.len() >= n)); }
    if let Some(n) = e.habits { out.push(chk(format!("created {n} habit(s)"), o.created_habit_names.len() == n)); }
    if let Some(n) = e.min_habits { out.push(chk(format!("≥{n} habit(s)"), o.created_habit_names.len() >= n)); }
    if let Some(n) = e.min_updated { out.push(chk(format!("≥{n} updated"), o.updated_event_titles.len() >= n)); }
    if let Some(n) = e.min_removed { out.push(chk(format!("≥{n} removed"), o.removed_event_titles.len() >= n)); }
    for needle in e.title_has { out.push(chk(format!("title ~ \"{needle}\""), title_in(o, needle))); }
    for needle in e.habit_has {
        let nl = needle.to_lowercase();
        out.push(chk(format!("habit ~ \"{needle}\""), o.created_habit_names.iter().any(|h| h.to_lowercase().contains(&nl))));
    }
    for needle in e.removed_has {
        let nl = needle.to_lowercase();
        out.push(chk(format!("removed ~ \"{needle}\""), o.removed_event_titles.iter().any(|t| t.to_lowercase().contains(&nl))));
    }
    for needle in e.survives { out.push(chk(format!("\"{needle}\" still exists"), ev_exists(conn, needle))); }
    for (needle, h, m) in e.event_start_hm {
        out.push(chk(format!("{} starts {h:02}:{m:02}", show(needle)), ev_start_hm(conn, needle) == Some((*h, *m))));
    }
    for (needle, mins) in e.event_minutes {
        out.push(chk(format!("{} ≈ {mins}m", show(needle)), near(ev_minutes(conn, needle), *mins)));
    }
    for (needle, off) in e.event_day_offset {
        let want = Local::now().naive_local().date() + ChronoDuration::days(*off);
        out.push(chk(format!("{} on day {off:+}", show(needle)), ev_start_date(conn, needle) == Some(want)));
    }
    for p in e.priorities {
        let ok = db::list_tasks(conn).map(|ts| ts.iter().any(|t| t.priority == *p)).unwrap_or(false);
        out.push(chk(format!("a task priority={p}"), ok));
    }
    if e.has_task_dep {
        let ok = db::list_tasks(conn).map(|ts| ts.iter().any(|t| !t.depends_on.is_empty())).unwrap_or(false);
        out.push(chk("a task has a dependency", ok));
    }
    if e.created_nothing {
        out.push(chk("created nothing", o.created_event_titles.is_empty() && o.created_task_ids.is_empty() && o.created_habit_names.is_empty()));
    }
    if e.has_conflict {
        out.push(chk("a scheduling conflict surfaced", !conflicts.is_empty()));
    }
    out
}
fn show(needle: &str) -> &str {
    if needle.is_empty() { "event" } else { needle }
}

// ================================ UI renderer ================================

/// The chat reply the user sees — a faithful port of `ChatPane.tsx` `send()` (lines ~64–92).
/// KEEP IN SYNC with that component (TS↔Rust can't share the logic).
fn render_chat(o: &PlanOutcome) -> String {
    let n = o.created_task_ids.len();
    let ev = o.created_event_titles.len();
    let hab = o.created_habit_names.len();
    let upd = o.updated_event_titles.len();
    let rem = o.removed_event_titles.len();
    let s = |k: usize| if k == 1 { "" } else { "s" };

    let mut bits: Vec<String> = Vec::new();
    if n > 0 {
        let to = if o.project_names.is_empty() { String::new() } else { format!(" to {}", o.project_names.join(", ")) };
        bits.push(format!("{n} task{}{}", s(n), to));
    }
    if ev > 0 { bits.push(format!("{ev} event{} ({})", s(ev), o.created_event_titles.join(", "))); }
    if hab > 0 { bits.push(format!("{hab} habit{} ({})", s(hab), o.created_habit_names.join(", "))); }

    let mut actions: Vec<String> = Vec::new();
    if !bits.is_empty() { actions.push(format!("Added {}", bits.join(" and "))); }
    if upd > 0 {
        let mut seen = HashSet::new();
        let uniq: Vec<String> = o.updated_event_titles.iter().filter(|t| seen.insert((*t).clone())).cloned().collect();
        actions.push(format!("updated {upd} event{} ({})", s(upd), uniq.join(", ")));
    }
    if rem > 0 { actions.push(format!("removed {rem} event{}", s(rem))); }

    let mut parts: Vec<String> = Vec::new();
    if !actions.is_empty() {
        parts.push(format!("{}, and re-planned your calendar.", actions.join(", ")));
    } else if o.clarifications.is_empty() {
        parts.push("I didn't catch anything to change — try giving a bit more detail.".to_string());
    }
    if !o.clarifications.is_empty() {
        parts.push(format!("A few things to confirm:\n{}", o.clarifications.iter().map(|c| format!("• {c}")).collect::<Vec<_>>().join("\n")));
    }
    if !o.recalled_notes.is_empty() {
        parts.push(format!("📌 Recalled from your notes:\n{}", o.recalled_notes.iter().map(|c| format!("• {c}")).collect::<Vec<_>>().join("\n")));
    }
    parts.join("\n\n")
}

/// The calendar the user would see: timed events + scheduled task-blocks grouped by day and sorted by
/// time, with multi-day/all-day events split out. Overlaps are flagged (scheduling conflicts show).
fn render_calendar(conn: &Connection) -> String {
    let events = db::list_events(conn).unwrap_or_default();
    let blocks = db::list_blocks(conn).unwrap_or_default();
    let tasks = db::list_tasks(conn).unwrap_or_default();
    let task_title: HashMap<i64, String> = tasks.iter().map(|t| (t.id, t.title.clone())).collect();

    struct Item { start: NaiveDateTime, end: NaiveDateTime, label: String, kind: &'static str }
    let midnight = NaiveTime::from_hms_opt(0, 0, 0).unwrap();
    let mut timed: Vec<Item> = Vec::new();
    let mut allday: Vec<(NaiveDateTime, NaiveDateTime, String)> = Vec::new();

    for e in &events {
        if let (Some(s), Some(en)) = (parse(&e.start), parse(&e.end)) {
            let dur = (en - s).num_minutes();
            // All-day / multi-day: ≥24h, or midnight→midnight (how the day view treats them).
            if dur >= 24 * 60 || (s.time() == midnight && en.time() == midnight && dur > 0) {
                allday.push((s, en, e.title.clone()));
            } else {
                timed.push(Item { start: s, end: en, label: e.title.clone(), kind: "event" });
            }
        }
    }
    for b in &blocks {
        if let (Some(s), Some(en)) = (parse(&b.start), parse(&b.end)) {
            let title = task_title.get(&b.task_id).cloned().unwrap_or_else(|| format!("task #{}", b.task_id));
            timed.push(Item { start: s, end: en, label: title, kind: "task block" });
        }
    }
    if timed.is_empty() && allday.is_empty() {
        return "(calendar empty)".to_string();
    }

    timed.sort_by(|a, b| a.start.cmp(&b.start));
    let mut out = String::new();
    let mut cur_date: Option<NaiveDate> = None;
    let mut day_last_end: Option<NaiveDateTime> = None;
    for it in &timed {
        let d = it.start.date();
        if cur_date != Some(d) {
            cur_date = Some(d);
            day_last_end = None;
            let _ = write!(out, "\n{}", d.format("%a %b %d"));
        }
        let overlap = day_last_end.map(|le| it.start < le).unwrap_or(false);
        let _ = write!(
            out,
            "\n  {}-{}  {}  [{}]{}",
            it.start.format("%H:%M"),
            it.end.format("%H:%M"),
            it.label,
            it.kind,
            if overlap { "  ⚠ overlaps previous" } else { "" },
        );
        day_last_end = Some(day_last_end.map(|le| le.max(it.end)).unwrap_or(it.end));
    }
    if !allday.is_empty() {
        let _ = write!(out, "\nall-day / multi-day:");
        for (s, en, title) in &allday {
            // end is exclusive midnight → show the inclusive last day when it lands on midnight.
            let last = if en.time() == midnight { (*en - ChronoDuration::days(1)).date() } else { en.date() };
            let _ = write!(out, "\n  {} -> {}  {}  [event]", s.format("%b %d"), last.format("%b %d"), title);
        }
    }
    out.trim_start().to_string()
}

fn render_tasks(conn: &Connection) -> String {
    let tasks = db::list_tasks(conn).unwrap_or_default();
    if tasks.is_empty() { return "(none)".to_string(); }
    let title_by_id: HashMap<i64, String> = tasks.iter().map(|t| (t.id, t.title.clone())).collect();
    let mut out = String::new();
    for t in &tasks {
        let due = t.deadline.as_deref().map(|d| d.chars().take(10).collect::<String>()).unwrap_or_else(|| "—".into());
        let deps = if t.depends_on.is_empty() {
            String::new()
        } else {
            let names: Vec<String> = t.depends_on.iter().map(|id| title_by_id.get(id).cloned().unwrap_or_else(|| format!("#{id}"))).collect();
            format!("  (after: {})", names.join(", "))
        };
        let _ = write!(out, "\n• {}  {}m  due {}  P{}  [{}]{}", t.title, t.estimated_minutes, due, t.priority, t.status, deps);
    }
    out.trim_start().to_string()
}

fn render_habits(conn: &Connection) -> String {
    let habits = db::list_habits(conn).unwrap_or_default();
    if habits.is_empty() { return "(none)".to_string(); }
    let mut out = String::new();
    for h in &habits {
        let cadence = match h.cadence.as_str() {
            "weekly" => format!("weekly {:?}", h.days),
            "interval" => format!("every {} days", h.interval_days),
            other => other.to_string(),
        };
        let _ = write!(out, "\n• {}  {}m  {}", h.name, h.duration_minutes, cadence);
    }
    out.trim_start().to_string()
}

fn render_conflicts(conflicts: &[Conflict]) -> Option<String> {
    if conflicts.is_empty() { return None; }
    let mut out = String::new();
    for c in conflicts {
        let line = match c {
            Conflict::Unschedulable { title, remaining_minutes, .. } => format!("⚠ \"{title}\" doesn't fit ({remaining_minutes}m over)"),
            Conflict::DeadlineMiss { title, .. } => format!("⚠ \"{title}\" can't finish before its deadline"),
            Conflict::DependencyCycle { task_ids } => format!("⚠ dependency cycle among {} tasks", task_ids.len()),
        };
        let _ = write!(out, "\n{line}");
    }
    Some(out.trim_start().to_string())
}

fn indent(s: &str, pad: &str) -> String {
    s.lines().map(|l| format!("{pad}{l}")).collect::<Vec<_>>().join("\n")
}

// ================================ the battery ================================
fn cases() -> Vec<Case> {
    use Expect as E;
    vec![
        // ---------- shorthand / sms-speak / typos ----------
        Case { name: "dr appt shorthand", category: "shorthand", note: "one event tomorrow 15:00 ('3p' = 3pm).",
            history: &[], seed: &[], prompt: "dr appt tmrw 3p",
            expect: E { events: Some(1), event_start_hm: &[("", 15, 0)], event_day_offset: &[("", 1)], ..Default::default() } },
        Case { name: "abbreviated meeting w/ day", category: "shorthand", note: "one event Monday 10:00.",
            history: &[], seed: &[], prompt: "mtg w/ sara mon 10a",
            expect: E { events: Some(1), event_start_hm: &[("", 10, 0)], ..Default::default() } },
        Case { name: "range shorthand no colon", category: "shorthand", note: "Fri 12:00–13:30 (90m).",
            history: &[], seed: &[], prompt: "lunch w bri fri 12-130",
            expect: E { events: Some(1), event_minutes: &[("lunch", 90)], event_start_hm: &[("lunch", 12, 0)], ..Default::default() } },
        Case { name: "bare at-time", category: "shorthand", note: "event today 16:00 ('@ 4' → 4pm default).",
            history: &[], seed: &[], prompt: "call the client @ 4",
            expect: E { events: Some(1), event_start_hm: &[("call", 16, 0)], ..Default::default() } },

        // ---------- ambiguous ----------
        Case { name: "bare hour, am/pm unclear", category: "ambiguous", note: "'5' is ambiguous; a sane default (17:00) OR a clarification is acceptable — should NOT silently pick 5am.",
            history: &[], seed: &[], prompt: "set up a meeting at 5",
            expect: E::default() },
        Case { name: "vague future time", category: "ambiguous", note: "no concrete time; acceptable to clarify or place as an all-day/loose item next week — NOT invent a precise time.",
            history: &[], seed: &[], prompt: "lunch with the team sometime next week",
            expect: E::default() },
        Case { name: "no date at all", category: "ambiguous", note: "'soon' has no date — should ask, not fabricate a day.",
            history: &[], seed: &[], prompt: "need to see the dentist soon",
            expect: E::default() },

        // ---------- self-correction ----------
        Case { name: "correct the time mid-sentence", category: "correction", note: "ONE event at the corrected 16:00, not two.",
            history: &[], seed: &[], prompt: "schedule a call at 3pm today, actually make it 4pm",
            expect: E { events: Some(1), event_start_hm: &[("call", 16, 0)], ..Default::default() } },
        Case { name: "correct am/pm", category: "correction", note: "ONE event at 07:00.",
            history: &[], seed: &[], prompt: "add yoga at 6, no wait — 7am",
            expect: E { events: Some(1), event_start_hm: &[("yoga", 7, 0)], ..Default::default() } },

        // ---------- brain-dump (mixed intents) ----------
        Case { name: "weekly brain-dump with a cancel", category: "brain-dump",
            note: "dentist event Mon, budget task w/ Wed deadline, a run habit, the old standup removed, haircut event Sat.",
            history: &[], seed: &[("Old standup", 1, (9, 0), 1, (9, 30))],
            prompt: "ok this week: dentist monday 9am, finish the budget report (~3h) by wednesday, go for a run every morning, cancel the old standup, and book a haircut saturday 11am",
            expect: E { min_events: Some(1), min_tasks: Some(1), min_habits: Some(1), min_removed: Some(1), title_has: &["dent"], habit_has: &["run"], ..Default::default() } },
        Case { name: "packed day with a prep task", category: "brain-dump",
            note: "several timed events tomorrow + a 'prep slides' task (~2h). Watch for double-booking / a stray duplicate.",
            history: &[], seed: &[],
            prompt: "tomorrow is packed: standup 9-9:15, design review 11-12, lunch at 12:30, a 1:1 at 3, and I need to prep slides (about 2 hours) before the review",
            expect: E { min_events: Some(3), min_tasks: Some(1), ..Default::default() } },

        // ---------- odd / relative dates ----------
        Case { name: "in a fortnight", category: "odd-date", note: "event 14 days out at 14:00.",
            history: &[], seed: &[], prompt: "schedule a project review in a fortnight at 2pm",
            expect: E { events: Some(1), event_day_offset: &[("review", 14)], ..Default::default() } },
        Case { name: "end of the month", category: "odd-date", note: "should land on the last day of this month at 10:00 (or clarify) — not today.",
            history: &[], seed: &[], prompt: "renew my passport at the end of the month at 10am",
            expect: E::default() },
        Case { name: "compound relative day", category: "odd-date", note: "'the day after next tuesday' — judge the resolved date on the screen.",
            history: &[], seed: &[], prompt: "lunch the day after next tuesday at 1pm",
            expect: E::default() },
        Case { name: "this weekend", category: "odd-date", note: "should land Sat or Sun — judge from the screen.",
            history: &[], seed: &[], prompt: "team offsite this weekend",
            expect: E::default() },
        Case { name: "day-of-month only", category: "odd-date", note: "'the 3rd' with no month — judge which date it picks (known Rust gap).",
            history: &[], seed: &[], prompt: "flight on the 3rd at 7am",
            expect: E::default() },

        // ---------- overnight / cross-midnight ----------
        Case { name: "cross-midnight movie", category: "overnight", note: "11pm→1am = 120m, not -22h.",
            history: &[], seed: &[], prompt: "movie night tonight 11pm to 1am",
            expect: E { events: Some(1), event_minutes: &[("movie", 120)], ..Default::default() } },
        Case { name: "overnight sleepover", category: "overnight", note: "8pm→8am = 12h (720m).",
            history: &[], seed: &[], prompt: "sleepover saturday 8pm to 8am",
            expect: E { events: Some(1), event_minutes: &[("sleep", 720)], ..Default::default() } },
        Case { name: "red-eye, only a start", category: "overnight", note: "late-night start; judge the time/date.",
            history: &[], seed: &[], prompt: "red-eye flight friday at 11:30pm",
            expect: E { min_events: Some(1), ..Default::default() } },

        // ---------- fuzzy durations ----------
        Case { name: "whole afternoon chore", category: "fuzzy-duration", note: "Saturday-afternoon cleaning: a task (~3-5h) OR an afternoon block — should NOT explode into many subtasks.",
            history: &[], seed: &[], prompt: "block off the whole afternoon saturday for cleaning the garage",
            expect: E::default() },
        Case { name: "quick sync", category: "fuzzy-duration", note: "'quick' ≈ 15–30m event at 14:00.",
            history: &[], seed: &[], prompt: "quick sync with sam at 2pm today",
            expect: E { events: Some(1), event_start_hm: &[("sync", 14, 0)], ..Default::default() } },
        Case { name: "a couple hours of work", category: "fuzzy-duration", note: "a task estimated ~120m (a couple hours).",
            history: &[], seed: &[], prompt: "work on the essay for a couple hours tomorrow",
            expect: E { min_tasks: Some(1), ..Default::default() } },

        // ---------- recurrence edges ----------
        Case { name: "every weekday", category: "recurrence", note: "a recurring habit, ideally Mon–Fri (weekly) — judge whether it excludes the weekend.",
            history: &[], seed: &[], prompt: "go to the gym every weekday at 7am",
            expect: E { min_habits: Some(1), habit_has: &["gym"], ..Default::default() } },
        Case { name: "MWF", category: "recurrence", note: "weekly habit on Mon/Wed/Fri — judge the days on the screen.",
            history: &[], seed: &[], prompt: "standup every monday, wednesday and friday at 9",
            expect: E { min_habits: Some(1), ..Default::default() } },
        Case { name: "twice a day", category: "recurrence", note: "twice-daily isn't expressible (daily-only) — a single daily habit is acceptable; judge it doesn't stack events.",
            history: &[], seed: &[], prompt: "water the plants twice a day",
            expect: E::default() },
        Case { name: "every other day, 2h", category: "recurrence", note: "interval habit (every 2 days), 120m — the duration MUST survive.",
            history: &[], seed: &[], prompt: "study every other day for two hours at 9am",
            expect: E::default() },
        Case { name: "every morning", category: "recurrence", note: "a daily meditation habit.",
            history: &[], seed: &[], prompt: "meditate for 10 minutes every morning",
            expect: E { min_habits: Some(1), habit_has: &["medit"], ..Default::default() } },

        // ---------- restraint ----------
        Case { name: "calendar query", category: "restraint", note: "a question — create nothing.",
            history: &[], seed: &[], prompt: "what's on my calendar tomorrow?",
            expect: E { created_nothing: true, ..Default::default() } },
        Case { name: "venting", category: "restraint", note: "venting — create nothing.",
            history: &[], seed: &[], prompt: "honestly i'm so behind on everything this week",
            expect: E { created_nothing: true, ..Default::default() } },
        Case { name: "past tense", category: "restraint", note: "already done — create nothing.",
            history: &[], seed: &[], prompt: "i already finished the quarterly report earlier",
            expect: E { created_nothing: true, ..Default::default() } },
        Case { name: "explicit negation", category: "restraint", note: "explicitly told not to add — create nothing.",
            history: &[], seed: &[], prompt: "don't schedule anything, i'm just thinking out loud",
            expect: E { created_nothing: true, ..Default::default() } },
        Case { name: "uncertainty", category: "restraint", note: "'maybe / not sure' — ideally nothing or a clarification, not a committed event.",
            history: &[], seed: &[], prompt: "maybe i'll go for a run later, not sure yet",
            expect: E::default() },

        // ---------- multi-turn context ----------
        Case { name: "pronoun edit via history", category: "context", note: "'move it' refers to the dentist → update to 15:00, no new event.",
            history: &[("user", "add a dentist appointment friday at 2pm"), ("assistant", "Added Dentist on Friday at 2pm.")],
            seed: &[("Dentist", 2, (14, 0), 2, (15, 0))], prompt: "actually move it to 3pm",
            expect: E { min_updated: Some(1), events: Some(0), event_start_hm: &[("dentist", 15, 0)], ..Default::default() } },
        Case { name: "'make that longer'", category: "context", note: "'that' = the workshop → duration becomes 180m.",
            history: &[("user", "schedule a workshop monday 10 to 12"), ("assistant", "Added Workshop Monday 10–12.")],
            seed: &[("Workshop", 1, (10, 0), 1, (12, 0))], prompt: "make that 3 hours instead",
            expect: E { min_updated: Some(1), event_minutes: &[("workshop", 180)], ..Default::default() } },
        Case { name: "fresh request ignores stale subject", category: "context", note: "new 'surgery' event must NOT inherit the earlier 'study' subject.",
            history: &[("user", "help me study for my chemistry final this week"), ("assistant", "Added a Study project and re-planned.")],
            seed: &[], prompt: "on 6/12 i have a surgery at 10am",
            expect: E { min_events: Some(1), title_has: &["surg"], ..Default::default() } },
        Case { name: "follow-up supplies the time", category: "context", note: "history says meeting w/ Sarah; this turn gives day+time → one event.",
            history: &[("user", "can you schedule a meeting with sarah?"), ("assistant", "Sure — what day and time?")],
            seed: &[], prompt: "this friday at 7pm",
            expect: E { events: Some(1), ..Default::default() } },

        // ---------- selective edits / removes ----------
        Case { name: "cancel all of a kind", category: "selective", note: "both sleepovers removed.",
            history: &[], seed: &[("Sleepover at Jake's", 2, (20, 0), 3, (8, 0)), ("Sleepover at Mia's", 4, (20, 0), 5, (8, 0))],
            prompt: "cancel all my sleepovers",
            expect: E { min_removed: Some(2), ..Default::default() } },
        Case { name: "selective cancel spares sibling", category: "selective", note: "only 'with Dan' removed; 'with mom' survives.",
            history: &[], seed: &[("Lunch with mom", 1, (12, 0), 1, (13, 0)), ("Lunch with Dan", 1, (15, 0), 1, (16, 0))],
            prompt: "cancel lunch with dan",
            expect: E { min_removed: Some(1), removed_has: &["Dan"], survives: &["with mom"], ..Default::default() } },
        Case { name: "cancel-except", category: "selective", note: "'everything except the standup' — judge that standup survives and the rest go.",
            history: &[], seed: &[("Standup", 1, (9, 0), 1, (9, 30)), ("Design review", 1, (11, 0), 1, (12, 0)), ("Lunch", 1, (12, 30), 1, (13, 30))],
            prompt: "clear my schedule tomorrow except the standup",
            expect: E { survives: &["Standup"], ..Default::default() } },
        Case { name: "edit a nonexistent event", category: "selective", note: "no dentist exists — should clarify / no-op, NOT fabricate a new event.",
            history: &[], seed: &[], prompt: "move the dentist appointment to 4pm",
            expect: E::default() },

        // ---------- conflicts / unschedulable ----------
        Case { name: "too much work for a full day", category: "conflict", note: "the calendar is fully blocked 9–17 today; 6h of work today can't fit → expect a conflict OR it spilling to later days (visible on the screen).",
            history: &[], seed: &[("Conference", 0, (9, 0), 0, (17, 0))],
            prompt: "i need to get 6 hours of deep work done today",
            expect: E::default() },
        Case { name: "deadline that can't be met", category: "conflict", note: "busy all day; a 4-hour task due today → deadline-miss / unschedulable should surface.",
            history: &[], seed: &[("All-day offsite", 0, (9, 0), 0, (18, 0))],
            prompt: "finish the 4-hour report by end of day today",
            expect: E::default() },

        // ---------- adversarial ----------
        Case { name: "over-decomposition bait", category: "adversarial", note: "a single 2h study block — exactly ONE task, no invented subtasks/project.",
            history: &[], seed: &[], prompt: "tomorrow i'm going to study for 2 hours starting at 1pm",
            expect: E::default() },
        Case { name: "wildly vague", category: "adversarial", note: "'plan my entire life' — should clarify / create nothing, not hallucinate a pile.",
            history: &[], seed: &[], prompt: "plan my entire life for me",
            expect: E::default() },
        Case { name: "emoji-laden party", category: "adversarial", note: "one 3-hour event Saturday 20:00–23:00; emoji shouldn't corrupt the title or time.",
            history: &[], seed: &[], prompt: "🎉 birthday party 🎂 saturday 8pm-11pm at my place 🍕",
            expect: E { events: Some(1), event_minutes: &[("party", 180)], ..Default::default() } },
        Case { name: "non-ASCII / Spanish", category: "adversarial", note: "one event tomorrow 15:00; accented text shouldn't break extraction.",
            history: &[], seed: &[], prompt: "reunión con José mañana a las 3pm",
            expect: E { min_events: Some(1), ..Default::default() } },
        Case { name: "rambling vague title", category: "adversarial", note: "Thursday 14:00 event; judge the title it settles on.",
            history: &[], seed: &[], prompt: "remind me about the thing with the guy about the stuff on thursday at 2",
            expect: E { min_events: Some(1), ..Default::default() } },
    ]
}

// ================================ runner ================================
#[tokio::test]
#[ignore = "needs a live llama-server; run with --ignored --nocapture (Pushin open, or PUSHIN_LLM_URL set)"]
async fn model_battery() {
    let base = std::env::var("PUSHIN_LLM_URL").unwrap_or_else(|_| "http://127.0.0.1:8080".into());
    let model = std::env::var("PUSHIN_LLM_MODEL").unwrap_or_else(|_| Settings::default().model_id);
    let client = reqwest::Client::builder().timeout(Duration::from_secs(180)).build().unwrap();

    if client.get(format!("{base}/v1/models")).timeout(Duration::from_secs(3)).send().await.is_err() {
        eprintln!("\n⚠️  No llama-server reachable at {base}. Open Pushin (or set PUSHIN_LLM_URL) and re-run. Skipping.\n");
        return;
    }

    let header = format!("# Pushin model battery — {model} @ {base}\n_(run {})_\n", Local::now().format("%Y-%m-%d %H:%M"));
    let mut report = header.clone();
    print!("{header}");

    let mut by_cat: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    let (mut pass_total, mut total_total) = (0usize, 0usize);

    for (i, case) in cases().into_iter().enumerate() {
        let path = std::env::temp_dir().join(format!("pushin_battery_{}_{}.db", std::process::id(), i));
        let _ = std::fs::remove_file(&path);
        let mut conn = db::open(&path).expect("open temp db");

        let mut settings = Settings::default();
        settings.llm_base_url = base.clone();
        settings.model_id = model.clone();
        settings.sleep_enabled = false;

        for (title, sdo, (sh, sm), edo, (eh, em)) in case.seed {
            db::insert_event(&conn, title, &iso(*sdo, *sh, *sm), &iso(*edo, *eh, *em), "fixed").unwrap();
        }
        let current: Vec<Event> = db::list_events(&conn).unwrap_or_default();
        let history: Vec<ChatTurn> = case.history.iter().map(|(r, c)| ChatTurn { role: (*r).into(), content: (*c).into() }).collect();

        let (checks, outcome, conflicts): (Vec<(String, bool)>, Option<PlanOutcome>, Vec<Conflict>) =
            match parser::plan(&client, &settings, &current, &history, case.prompt, &[]).await {
                Ok(plan) => match parser::store_plan(&conn, &settings, &plan) {
                    Ok(o) => {
                        let conflicts = schedule_service::reschedule_inner(&mut conn, &settings).map(|s| s.conflicts).unwrap_or_default();
                        (evaluate(&case.expect, &o, &conflicts, &conn), Some(o), conflicts)
                    }
                    Err(e) => (vec![chk(format!("store_plan errored: {e}"), false)], None, vec![]),
                },
                Err(e) => (vec![chk(format!("plan errored: {e}"), false)], None, vec![]),
            };

        let passed = checks.iter().filter(|(_, ok)| *ok).count();
        let total = checks.len();

        // ---- build the per-case "screen" ----
        let mut screen = String::new();
        let _ = writeln!(screen, "\n══════════════════════════════════════════════════════════════════");
        let mark = if total == 0 { "·" } else if passed == total { "✓" } else { "✗" };
        let _ = writeln!(screen, "{mark} [{}] {}   {}/{}", case.category, case.name, passed, total);
        let _ = writeln!(screen, "prompt: {:?}", case.prompt);
        if !case.history.is_empty() {
            let h = case.history.iter().map(|(r, c)| format!("{r}: {c}")).collect::<Vec<_>>().join("  |  ");
            let _ = writeln!(screen, "history: {h}");
        }
        if !case.seed.is_empty() {
            let s = case.seed.iter().map(|(t, ..)| *t).collect::<Vec<_>>().join(", ");
            let _ = writeln!(screen, "seeded calendar: {s}");
        }
        let _ = writeln!(screen, "──────────────────────────────────────────────────────────────────");
        match &outcome {
            Some(o) => {
                let _ = writeln!(screen, "CHAT\n{}", indent(&render_chat(o), "  "));
                let _ = writeln!(screen, "\nCALENDAR\n{}", indent(&render_calendar(&conn), "  "));
                let _ = writeln!(screen, "\nTASKS\n{}", indent(&render_tasks(&conn), "  "));
                let _ = writeln!(screen, "\nHABITS\n{}", indent(&render_habits(&conn), "  "));
                if let Some(c) = render_conflicts(&conflicts) {
                    let _ = writeln!(screen, "\nCONFLICTS\n{}", indent(&c, "  "));
                }
            }
            None => { let _ = writeln!(screen, "(pipeline error — see checks)"); }
        }
        if total > 0 {
            let _ = writeln!(screen, "\nCHECKS  {passed}/{total}");
            for (label, ok) in &checks {
                let _ = writeln!(screen, "  {} {label}", if *ok { "✓" } else { "✗" });
            }
        }
        if !case.note.is_empty() {
            let _ = writeln!(screen, "NOTE (good = ): {}", case.note);
        }

        print!("{screen}");
        report.push_str(&screen);

        let e = by_cat.entry(case.category).or_default();
        e.0 += passed;
        e.1 += total;
        pass_total += passed;
        total_total += total;

        drop(conn);
        let _ = std::fs::remove_file(&path);
    }

    // ---- scorecard ----
    let mut footer = String::from("\n\n══════════════════════ scorecard (by category) ══════════════════════\n");
    for (cat, (p, t)) in &by_cat {
        let pct = if *t == 0 { 0.0 } else { 100.0 * *p as f64 / *t as f64 };
        let _ = writeln!(footer, "  {cat:<14} {p}/{t}  ({pct:.0}%)");
    }
    let pct = if total_total == 0 { 0.0 } else { 100.0 * pass_total as f64 / total_total as f64 };
    let _ = writeln!(footer, "\n  TOTAL (crisp checks): {pass_total}/{total_total}  ({pct:.0}%)");
    let _ = writeln!(footer, "  (cases with no checks are eye-judged from the screens above)");
    print!("{footer}");
    report.push_str(&footer);

    // ---- persist the full report so it can be read after the run ----
    let report_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("model-battery").join("report.md");
    if let Some(parent) = report_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if std::fs::write(&report_path, &report).is_ok() {
        println!("\n📄 Full report: {}", report_path.display());
    }
}
