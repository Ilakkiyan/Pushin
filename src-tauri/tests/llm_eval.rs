//! LLM pressure-test harness — measures how well the on-device model + parsing pipeline turn
//! real-world prompts into the right plan. This is a *measurement tool*, not a pass/fail gate:
//! tiny models are probabilistic, so it prints a scorecard rather than failing the build.
//!
//! It is `#[ignore]`d (and self-skips if no server is reachable), so `cargo test` stays green
//! on machines without a model. Run it against a live llama-server like so:
//!
//! ```text
//! # 1. Launch Pushin so a llama-server is up on :8080 (or point at any OpenAI-compatible server)
//! # 2. From src-tauri/:
//! cargo test --test llm_eval -- --ignored --nocapture
//!
//! # Override target/model if needed:
//! PUSHIN_LLM_URL=http://127.0.0.1:8080 PUSHIN_LLM_MODEL=qwen2.5-7b-instruct-q4_k_m \
//!   cargo test --test llm_eval -- --ignored --nocapture
//! ```
//!
//! Each case runs the *full* real path — `parser::plan` (LLM call + deterministic recovery) then
//! `parser::store_plan` (dedupe, edit-reconcile, date resolution) against a throwaway SQLite DB —
//! and scores the resulting `PlanOutcome` / calendar against what the prompt should produce. The
//! per-category accuracy is the feedback loop for tuning the prompt and the deterministic tracks.

use std::time::Duration;

use chrono::{Datelike, Duration as ChronoDuration, Local, NaiveDate, NaiveDateTime, Timelike};
use pushin_lib::db;
use pushin_lib::model::{Event, Settings};
use pushin_lib::parser::{self, ChatTurn, PlanOutcome};
use rusqlite::Connection;

// ---------------- expectations ----------------

#[derive(Default)]
struct Expect {
    events: Option<usize>,     // exact count of newly created events
    min_events: Option<usize>, // at least N created events
    tasks: Option<usize>,      // exact count of created tasks
    min_tasks: Option<usize>,
    habits: Option<usize>,
    min_habits: Option<usize>,
    min_updated: Option<usize>,
    min_removed: Option<usize>,
    /// Each needle must appear in some created/updated event title.
    title_has: &'static [&'static str],
    /// Each needle must appear in some created habit name.
    habit_has: &'static [&'static str],
    /// Each needle must appear in some REMOVED event title (selective deletes).
    removed_has: &'static [&'static str],
    /// (needle, minutes) — the matching event's duration ≈ minutes.
    event_minutes: &'static [(&'static str, i64)],
    /// (needle, hour, minute) — the matching event starts at exactly h:m (AM/PM/noon correctness).
    event_start_hm: &'static [(&'static str, u32, u32)],
    /// (needle, day_offset) — the matching event's start date == today + offset (relative dates).
    event_day_offset: &'static [(&'static str, i64)],
    /// Titles that must STILL exist after (e.g. a sibling event a selective remove must spare).
    survives: &'static [&'static str],
    /// Each value must match some created task's estimate (±5 min).
    task_estimates: &'static [i64],
    /// Each value (1 low .. 4 urgent) must appear among created tasks' priorities.
    priorities: &'static [i64],
    /// At least one created task has a dependency.
    has_task_dep: bool,
    /// Bespoke checks needing the resulting calendar/tasks (durations, spans, deadlines…).
    custom: Option<fn(&PlanOutcome, &Connection) -> Vec<(String, bool)>>,
}

struct Case {
    name: &'static str,
    category: &'static str,
    history: &'static [(&'static str, &'static str)],
    seed: &'static [(&'static str, i64, (u32, u32), i64, (u32, u32))], // (title, start_day_off,(h,m), end_day_off,(h,m))
    prompt: &'static str,
    expect: Expect,
}

// ---------------- helpers ----------------

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

/// Minutes of the first event whose title contains `needle`.
fn ev_minutes(conn: &Connection, needle: &str) -> Option<i64> {
    let n = needle.to_lowercase();
    db::list_events(conn).ok()?.into_iter().find(|e| e.title.to_lowercase().contains(&n)).and_then(|e| {
        let (s, en) = (parse(&e.start)?, parse(&e.end)?);
        Some((en - s).num_minutes())
    })
}

fn near(actual: Option<i64>, want: i64) -> bool {
    actual.map(|a| (a - want).abs() <= 1).unwrap_or(false)
}

/// (start, end) of the first event whose title contains `needle`.
fn ev_span(conn: &Connection, needle: &str) -> Option<(NaiveDateTime, NaiveDateTime)> {
    let n = needle.to_lowercase();
    let e = db::list_events(conn).ok()?.into_iter().find(|e| e.title.to_lowercase().contains(&n))?;
    Some((parse(&e.start)?, parse(&e.end)?))
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

fn evaluate(e: &Expect, o: &PlanOutcome, conn: &Connection) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    if let Some(n) = e.events {
        out.push(chk(format!("created {n} event(s)"), o.created_event_titles.len() == n));
    }
    if let Some(n) = e.min_events {
        out.push(chk(format!("≥{n} event(s)"), o.created_event_titles.len() >= n));
    }
    if let Some(n) = e.tasks {
        out.push(chk(format!("created {n} task(s)"), o.created_task_ids.len() == n));
    }
    if let Some(n) = e.min_tasks {
        out.push(chk(format!("≥{n} task(s)"), o.created_task_ids.len() >= n));
    }
    if let Some(n) = e.habits {
        out.push(chk(format!("created {n} habit(s)"), o.created_habit_names.len() == n));
    }
    if let Some(n) = e.min_updated {
        out.push(chk(format!("≥{n} updated"), o.updated_event_titles.len() >= n));
    }
    if let Some(n) = e.min_removed {
        out.push(chk(format!("≥{n} removed"), o.removed_event_titles.len() >= n));
    }
    for needle in e.title_has {
        out.push(chk(format!("title ~ \"{needle}\""), title_in(o, needle)));
    }
    for needle in e.habit_has {
        let n = needle.to_lowercase();
        out.push(chk(format!("habit ~ \"{needle}\""), o.created_habit_names.iter().any(|h| h.to_lowercase().contains(&n))));
    }
    if let Some(n) = e.min_habits {
        out.push(chk(format!("≥{n} habit(s)"), o.created_habit_names.len() >= n));
    }
    for needle in e.removed_has {
        let n = needle.to_lowercase();
        out.push(chk(format!("removed ~ \"{needle}\""), o.removed_event_titles.iter().any(|t| t.to_lowercase().contains(&n))));
    }
    for (needle, mins) in e.event_minutes {
        out.push(chk(format!("{needle} ≈ {mins}m"), near(ev_minutes(conn, needle), *mins)));
    }
    for (needle, h, m) in e.event_start_hm {
        out.push(chk(format!("{needle} starts {h:02}:{m:02}"), ev_start_hm(conn, needle) == Some((*h, *m))));
    }
    for (needle, off) in e.event_day_offset {
        let want = Local::now().naive_local().date() + ChronoDuration::days(*off);
        out.push(chk(format!("{needle} on day {off:+}"), ev_start_date(conn, needle) == Some(want)));
    }
    for needle in e.survives {
        out.push(chk(format!("\"{needle}\" still exists"), ev_exists(conn, needle)));
    }
    for est in e.task_estimates {
        let ok = db::list_tasks(conn).map(|ts| ts.iter().any(|t| (t.estimated_minutes - est).abs() <= 5)).unwrap_or(false);
        out.push(chk(format!("a task ≈ {est}m"), ok));
    }
    for p in e.priorities {
        let ok = db::list_tasks(conn).map(|ts| ts.iter().any(|t| t.priority == *p)).unwrap_or(false);
        out.push(chk(format!("a task priority={p}"), ok));
    }
    if e.has_task_dep {
        let ok = db::list_tasks(conn).map(|ts| ts.iter().any(|t| !t.depends_on.is_empty())).unwrap_or(false);
        out.push(chk("a task has a dependency", ok));
    }
    if let Some(f) = e.custom {
        out.extend(f(o, conn));
    }
    out
}

// ---- custom checks ----

fn lunch_and_party(_o: &PlanOutcome, c: &Connection) -> Vec<(String, bool)> {
    vec![
        chk("lunch ≈ 2h", near(ev_minutes(c, "lunch"), 120)),
        chk("party ≈ 4h", near(ev_minutes(c, "party"), 240)),
    ]
}
fn sleepover_overnight(_o: &PlanOutcome, c: &Connection) -> Vec<(String, bool)> {
    vec![chk("sleepover ≈ 12h overnight", near(ev_minutes(c, "sleep"), 720))]
}
fn meeting_two_hours(_o: &PlanOutcome, c: &Connection) -> Vec<(String, bool)> {
    vec![chk("meeting now 2h", near(ev_minutes(c, "meeting"), 120))]
}
fn standup_half_hour(_o: &PlanOutcome, c: &Connection) -> Vec<(String, bool)> {
    vec![chk("standup ≈ 30m", near(ev_minutes(c, "standup"), 30))]
}
fn pmless_range_two_hours(_o: &PlanOutcome, c: &Connection) -> Vec<(String, bool)> {
    vec![chk("12–2 ≈ 2h", near(ev_minutes(c, ""), 120))]
}
fn trip_is_multiday(_o: &PlanOutcome, c: &Connection) -> Vec<(String, bool)> {
    let m = ev_minutes(c, "");
    vec![chk("trip is a multi-day all-day block (≥13d)", m.map(|x| x >= 13 * 24 * 60).unwrap_or(false))]
}
fn span_two_days(_o: &PlanOutcome, c: &Connection) -> Vec<(String, bool)> {
    let m = ev_minutes(c, "");
    vec![chk("spans ~2 all-day days", m.map(|x| x >= 2 * 24 * 60).unwrap_or(false))]
}
fn single_task_estimate_180(_o: &PlanOutcome, c: &Connection) -> Vec<(String, bool)> {
    let est = db::list_tasks(c).ok().and_then(|t| t.first().map(|t| t.estimated_minutes));
    vec![chk("task estimate ≈ 3h", near(est, 180))]
}
fn all_tasks_have_deadline(_o: &PlanOutcome, c: &Connection) -> Vec<(String, bool)> {
    let tasks = db::list_tasks(c).unwrap_or_default();
    let ok = !tasks.is_empty() && tasks.iter().all(|t| t.deadline.is_some());
    vec![chk("every task got a deadline", ok)]
}
fn created_nothing(o: &PlanOutcome, _c: &Connection) -> Vec<(String, bool)> {
    vec![chk("didn't fabricate an event/task", o.created_event_titles.is_empty() && o.created_task_ids.is_empty())]
}
fn study_not_overdecomposed(o: &PlanOutcome, _c: &Connection) -> Vec<(String, bool)> {
    // The 3B/7B loves to explode a plain "study for 2h" into a project with invented subtasks
    // ("Pick platform"). A single study item is fine; fabricating extras is the failure.
    vec![chk("didn't fabricate extra tasks (≤1)", o.created_task_ids.len() <= 1)]
}
fn passport_on_the_25th(_o: &PlanOutcome, c: &Connection) -> Vec<(String, bool)> {
    // Day-of-month with no month ("on the 25th") — Rust resolves M/D, not "the 25th", so this is a
    // known date-resolution stretch. The check documents whether the model lands the right day.
    vec![chk("lands on the 25th", ev_start_date(c, "passport").map(|d| d.day()) == Some(25))]
}
fn robotics_three_days(_o: &PlanOutcome, c: &Connection) -> Vec<(String, bool)> {
    // "3 days from 8am to 5pm" must become three separate 8–5 days (Thu/Fri/Sat), NOT one all-day
    // block that swallows the time window (the bug: find_span_days made it span_days → all-day).
    let evs: Vec<Event> = db::list_events(c)
        .unwrap_or_default()
        .into_iter()
        .filter(|e| e.title.to_lowercase().contains("robot"))
        .collect();
    let windows_ok = !evs.is_empty()
        && evs.iter().all(|e| match (parse(&e.start), parse(&e.end)) {
            (Some(s), Some(en)) => s.hour() == 8 && (en - s).num_minutes() == 540, // 8am–5pm
            _ => false,
        });
    let mut days: Vec<NaiveDate> = evs.iter().filter_map(|e| parse(&e.start).map(|d| d.date())).collect();
    days.sort();
    days.dedup();
    let consecutive = days.len() == 3 && (days[2] - days[0]).num_days() == 2;
    vec![
        chk("three robotics days", evs.len() == 3),
        chk("each is an 8am–5pm window (not all-day)", windows_ok),
        chk("three consecutive days from Thursday", consecutive),
    ]
}
fn us_history_two_hours(o: &PlanOutcome, c: &Connection) -> Vec<(String, bool)> {
    // The 2-hour duration is the point (the create-time duration-drop the user hit). "Every other
    // day" recurrence has no clean home (habits are daily-only), so accept the block as either a
    // recurring habit or a single event — as long as it lands as a ~2-hour US-history slot.
    let habit_2h = db::list_habits(c)
        .map(|hs| hs.iter().any(|h| h.name.to_lowercase().contains("histor") && (h.duration_minutes - 120).abs() <= 5))
        .unwrap_or(false);
    let event_2h = near(ev_minutes(c, "histor"), 120);
    vec![
        chk("US history got scheduled (habit or event)", !o.created_habit_names.is_empty() || !o.created_event_titles.is_empty()),
        chk("as a 2-hour block", habit_2h || event_2h),
    ]
}
fn one_task_not_event(o: &PlanOutcome, _c: &Connection) -> Vec<(String, bool)> {
    // "Spend Saturday afternoon cleaning the garage" is work (a task), not a fixed event, and is one
    // activity (not a decomposed project).
    vec![
        chk("is a task, not an event", o.created_task_ids.len() == 1 && o.created_event_titles.is_empty()),
    ]
}

// ---------------- the battery ----------------

fn cases() -> Vec<Case> {
    use Expect as E;
    vec![
        // ---- single task ----
        Case {
            name: "single task with stated length",
            category: "single-task",
            history: &[],
            seed: &[],
            prompt: "I need to study for my chemistry final, about 3 hours.",
            expect: E { tasks: Some(1), custom: Some(single_task_estimate_180), ..Default::default() },
        },
        // ---- multi-task projects ----
        Case {
            name: "side project, 4 tasks, relative deadline",
            category: "multi-task",
            history: &[],
            seed: &[],
            prompt: "Launch a side project in 3 weeks: design a logo, build a landing page, write 3 blog posts, and set up analytics.",
            expect: E { min_tasks: Some(4), custom: Some(all_tasks_have_deadline), ..Default::default() },
        },
        Case {
            name: "exam prep, day-word deadline",
            category: "multi-task",
            history: &[],
            seed: &[],
            prompt: "Prep for my exam Friday: review 4 chapters, do 2 practice tests, and make a cheat sheet.",
            expect: E { min_tasks: Some(3), custom: Some(all_tasks_have_deadline), ..Default::default() },
        },
        // ---- single events ----
        Case {
            name: "dentist appointment",
            category: "single-event",
            history: &[],
            seed: &[],
            prompt: "Dentist appointment this Friday at 2pm.",
            expect: E { events: Some(1), title_has: &["dentist"], ..Default::default() },
        },
        Case {
            name: "standup with explicit half-hour range",
            category: "single-event",
            history: &[],
            seed: &[],
            prompt: "Team standup tomorrow 9 to 9:30.",
            expect: E { events: Some(1), custom: Some(standup_half_hour), ..Default::default() },
        },
        // ---- multi events ----
        Case {
            name: "two events, both with ranges",
            category: "multi-event",
            history: &[],
            seed: &[],
            prompt: "Lunch with mom Friday 12-2 and a graduation party from 6-10.",
            expect: E { events: Some(2), custom: Some(lunch_and_party), ..Default::default() },
        },
        // ---- PM-less / overnight ranges ----
        Case {
            name: "pm-less range",
            category: "ranges",
            history: &[],
            seed: &[],
            prompt: "Block 12-2 for a working session today.",
            expect: E { events: Some(1), custom: Some(pmless_range_two_hours), ..Default::default() },
        },
        Case {
            name: "overnight range",
            category: "ranges",
            history: &[],
            seed: &[],
            prompt: "Sleepover Saturday 8pm to 8am.",
            expect: E { events: Some(1), custom: Some(sleepover_overnight), ..Default::default() },
        },
        // ---- edits (seeded) ----
        Case {
            name: "reschedule by title",
            category: "edit",
            history: &[],
            seed: &[("Dentist", 2, (14, 0), 2, (15, 0))],
            prompt: "Move the dentist to 4pm.",
            expect: E { min_updated: Some(1), events: Some(0), title_has: &["dentist"], ..Default::default() },
        },
        Case {
            name: "change duration (the classic loop)",
            category: "edit",
            history: &[],
            seed: &[("Meeting with my friend", 0, (13, 0), 0, (14, 0))],
            prompt: "Change the meeting today to be 2 hours instead of 1.",
            expect: E { min_updated: Some(1), custom: Some(meeting_two_hours), ..Default::default() },
        },
        // ---- remove (seeded) ----
        Case {
            name: "cancel by title",
            category: "remove",
            history: &[],
            seed: &[("Sleepover", 3, (20, 0), 4, (8, 0))],
            prompt: "Cancel the sleepover.",
            expect: E { min_removed: Some(1), events: Some(0), ..Default::default() },
        },
        // ---- habits / recurring ----
        Case {
            name: "daily routine with range → habit",
            category: "habit",
            history: &[],
            seed: &[],
            prompt: "I want to practice violin every day from 4 to 5pm.",
            expect: E { habits: Some(1), events: Some(0), habit_has: &["violin"], ..Default::default() },
        },
        Case {
            name: "daily routine with duration → habit",
            category: "habit",
            history: &[],
            seed: &[],
            prompt: "Exercise daily for 30 minutes.",
            expect: E { habits: Some(1), events: Some(0), habit_has: &["exercise"], ..Default::default() },
        },
        Case {
            // Screenshot 1: "exercise every day at 10pm" stacked duplicate events. A daily routine
            // with a fixed time must still become one habit, not a (stack of) one-off event(s).
            name: "daily routine with a fixed time → habit, no duplicate event",
            category: "habit",
            history: &[],
            seed: &[],
            prompt: "I'm going to exercise for an hour every day at 10pm.",
            expect: E { habits: Some(1), events: Some(0), habit_has: &["exercise"], ..Default::default() },
        },
        Case {
            // "every day except sunday" is a partial-week recurrence; habits are daily-only (no
            // per-day exclusions, no fixed time), so the exclusion + 8pm are known gaps. The point
            // is it lands as ONE (daily) workout habit, not a stack of one-off events.
            name: "daily-except-one-day routine → habit (exclusion is a known gap)",
            category: "habit",
            history: &[],
            seed: &[],
            prompt: "I want to workout every day this week except sunday at 8pm.",
            expect: E { min_habits: Some(1), events: Some(0), habit_has: &["workout"], ..Default::default() },
        },
        Case {
            // "every other day" recurrence isn't expressible (daily-only habits), but the stated
            // two-hour duration MUST survive — the create-time duration-drop the user reported.
            name: "every-other-day study block keeps its 2-hour duration",
            category: "habit",
            history: &[],
            seed: &[],
            prompt: "I will study every other day this week for two hours at 9 am for US history.",
            expect: E { custom: Some(us_history_two_hours), ..Default::default() },
        },
        // ---- dates / spans ----
        Case {
            name: "explicit date + multi-week span",
            category: "dates",
            history: &[],
            seed: &[],
            prompt: "From 6/12 I'll be staying in Vietnam for two weeks.",
            expect: E { events: Some(1), custom: Some(trip_is_multiday), ..Default::default() },
        },
        Case {
            name: "named weekday range → multi-day",
            category: "dates",
            history: &[],
            seed: &[],
            prompt: "I have orientation Wednesday and Thursday this week.",
            expect: E { min_events: Some(1), custom: Some(span_two_days), ..Default::default() },
        },
        Case {
            // A multi-day event WITH a daily time window: "3 days from 8am to 5pm" must become three
            // separate 8–5 days (Thu/Fri/Sat), not one all-day block that swallows the times.
            name: "multi-day event with a daily time window",
            category: "dates",
            history: &[],
            seed: &[],
            prompt: "From this Thursday, I have a robotics competition that lasts for 3 days from 8 am to 5 pm.",
            expect: E { custom: Some(robotics_three_days), ..Default::default() },
        },
        // ---- mixed event + task ----
        Case {
            name: "event and a task with deadline in one message",
            category: "mixed",
            history: &[],
            seed: &[],
            prompt: "Dentist Friday at 2pm, and I need to finish the slides, about 3 hours, due Monday.",
            expect: E { min_events: Some(1), min_tasks: Some(1), title_has: &["dentist"], custom: Some(all_tasks_have_deadline), ..Default::default() },
        },
        // ---- conversational follow-up ----
        Case {
            name: "follow-up resolves via history",
            category: "context",
            history: &[("user", "Can you schedule a meeting with Sarah?"), ("assistant", "Sure — what day and time?")],
            seed: &[],
            prompt: "This Friday at 7pm.",
            expect: E { events: Some(1), ..Default::default() },
        },
        Case {
            // Screenshot 3: a fresh, self-contained request must NOT inherit a stale subject from
            // earlier turns (the new "surgery" came back titled "Study"). History gating drops the
            // prior Study turns, so the event is titled from the actual request.
            name: "fresh request ignores stale history (no contamination)",
            category: "context",
            history: &[
                ("user", "Help me study for my chemistry final this week."),
                ("assistant", "Added a Study project with tasks, and re-planned your calendar."),
            ],
            seed: &[],
            prompt: "On 6/12 I have a surgery at 10am.",
            expect: E { min_events: Some(1), title_has: &["surg"], ..Default::default() },
        },
        // ---- restraint on vague input ----
        Case {
            name: "vague input → no hallucination",
            category: "restraint",
            history: &[],
            seed: &[],
            prompt: "I have some stuff going on next week.",
            expect: E { custom: Some(created_nothing), ..Default::default() },
        },
        Case {
            // Screenshot 2: "study for 2h starting at 1pm" got exploded into a project with an
            // invented "Pick platform" task. One study item is fine; fabricating extras is not.
            name: "simple study block isn't over-decomposed",
            category: "restraint",
            history: &[],
            seed: &[],
            prompt: "Tomorrow I'm going to study for 2 hours starting at 1pm.",
            expect: E { custom: Some(study_not_overdecomposed), ..Default::default() },
        },

        // ============================================================================
        //  AMBITIOUS / LONG-TAIL — harder phrasings to find where the model breaks.
        // ============================================================================

        // ---- harder single events: time normalization (noon/AM/short durations) ----
        Case {
            name: "noon + explicit duration",
            category: "hard-event",
            history: &[],
            seed: &[],
            prompt: "Team sync at noon tomorrow for 45 minutes.",
            expect: E { events: Some(1), event_start_hm: &[("sync", 12, 0)], event_minutes: &[("sync", 45)], ..Default::default() },
        },
        Case {
            name: "explicit morning beats assume-PM",
            category: "hard-event",
            history: &[],
            seed: &[],
            prompt: "Gym tomorrow at 6 in the morning.",
            expect: E { events: Some(1), event_start_hm: &[("gym", 6, 0)], ..Default::default() },
        },
        Case {
            name: "short 15-minute call",
            category: "hard-event",
            history: &[],
            seed: &[],
            prompt: "Quick 15 minute call with the bank at 3pm today.",
            expect: E { events: Some(1), event_start_hm: &[("call", 15, 0)], event_minutes: &[("call", 15)], ..Default::default() },
        },
        Case {
            name: "location noise + half-hour range",
            category: "hard-event",
            history: &[],
            seed: &[],
            prompt: "Lunch at Chipotle with Sam tomorrow 12:30 to 1:15.",
            expect: E { events: Some(1), event_start_hm: &[("lunch", 12, 30)], event_minutes: &[("lunch", 45)], ..Default::default() },
        },
        Case {
            name: "overnight cross-midnight range",
            category: "hard-event",
            history: &[],
            seed: &[],
            prompt: "Movie night tonight 11pm to 1am.",
            expect: E { events: Some(1), event_minutes: &[("movie", 120)], ..Default::default() },
        },
        Case {
            name: "three back-to-back events in one line",
            category: "hard-event",
            history: &[],
            seed: &[],
            prompt: "Tomorrow: standup 9-9:15, design review 11-12, and a 1:1 at 3-3:30.",
            // (The 1:1 often gets titled "One-on-one meeting", so we assert the two stable needles
            // plus the 3-event structure rather than needle-matching the third.)
            expect: E { events: Some(3), event_minutes: &[("standup", 15), ("design", 60)], ..Default::default() },
        },

        // ---- harder dates ----
        Case {
            name: "named day next week",
            category: "hard-date",
            history: &[],
            seed: &[],
            prompt: "Doctor's appointment next Tuesday at 9am.",
            expect: E { events: Some(1), event_start_hm: &[("doctor", 9, 0)], ..Default::default() },
        },
        Case {
            // Day-of-month with no month — a known Rust date-resolution gap; documents the model's pick.
            name: "day-of-month (the 25th)",
            category: "hard-date",
            history: &[],
            seed: &[],
            prompt: "Renew my passport on the 25th at 10am.",
            expect: E { events: Some(1), custom: Some(passport_on_the_25th), ..Default::default() },
        },

        // ---- harder tasks: dependencies, varied estimates, priorities, task-vs-event ----
        Case {
            name: "sequential dependencies",
            category: "hard-task",
            history: &[],
            seed: &[],
            prompt: "To launch the app I need to fix the login bug, then write tests, then deploy to prod.",
            expect: E { min_tasks: Some(3), has_task_dep: true, ..Default::default() },
        },
        Case {
            name: "two tasks, distinct estimates",
            category: "hard-task",
            history: &[],
            seed: &[],
            prompt: "Write the quarterly report, about 90 minutes, and email it to the team, 10 minutes.",
            expect: E { min_tasks: Some(2), task_estimates: &[90, 10], ..Default::default() },
        },
        Case {
            name: "mixed priorities",
            category: "hard-task",
            history: &[],
            seed: &[],
            prompt: "Urgent: file the tax forms by tomorrow. Also, low priority — organize the old photos sometime.",
            expect: E { min_tasks: Some(2), priorities: &[4, 1], ..Default::default() },
        },
        Case {
            name: "single chore is a task, not an event",
            category: "hard-task",
            history: &[],
            seed: &[],
            prompt: "Spend Saturday afternoon cleaning out the garage.",
            expect: E { custom: Some(one_task_not_event), ..Default::default() },
        },

        // ---- harder habits ----
        Case {
            name: "morning meditation habit",
            category: "hard-habit",
            history: &[],
            seed: &[],
            prompt: "Meditate for 10 minutes every morning.",
            expect: E { min_habits: Some(1), events: Some(0), habit_has: &["medit"], ..Default::default() },
        },
        Case {
            name: "habit with no stated duration",
            category: "hard-habit",
            history: &[],
            seed: &[],
            prompt: "Read every night before bed.",
            expect: E { min_habits: Some(1), events: Some(0), habit_has: &["read"], ..Default::default() },
        },
        Case {
            // Known limitation: habits are daily-only, so weekly recurrence ("every Monday and
            // Wednesday") has no clean home. Documents what the model does with it.
            name: "weekly recurrence (known gap)",
            category: "hard-habit",
            history: &[],
            seed: &[],
            prompt: "Go to the gym every Monday and Wednesday.",
            expect: E { min_habits: Some(1), events: Some(0), habit_has: &["gym"], ..Default::default() },
        },

        // ---- harder edits (relative time/day math against a seeded calendar) ----
        Case {
            name: "rename an event",
            category: "hard-edit",
            history: &[],
            seed: &[("Gym session", 1, (7, 0), 1, (8, 0))],
            prompt: "Rename my gym session to Morning workout.",
            expect: E { min_updated: Some(1), events: Some(0), title_has: &["workout"], ..Default::default() },
        },
        Case {
            name: "push back an hour (relative time math)",
            category: "hard-edit",
            history: &[],
            seed: &[("Dentist", 1, (14, 0), 1, (15, 0))],
            prompt: "Push my dentist appointment back an hour.",
            expect: E { min_updated: Some(1), event_start_hm: &[("dentist", 15, 0)], ..Default::default() },
        },
        Case {
            name: "shorten to one hour",
            category: "hard-edit",
            history: &[],
            seed: &[("Workshop", 1, (10, 0), 1, (12, 0))],
            prompt: "Make the workshop only one hour.",
            expect: E { min_updated: Some(1), event_minutes: &[("workshop", 60)], ..Default::default() },
        },
        Case {
            name: "move to tomorrow (day shift)",
            category: "hard-edit",
            history: &[],
            seed: &[("Standup", 0, (9, 0), 0, (9, 30))],
            prompt: "Move standup to tomorrow.",
            expect: E { min_updated: Some(1), events: Some(0), event_day_offset: &[("standup", 1)], ..Default::default() },
        },

        // ---- harder removes (selective / plural) ----
        Case {
            name: "selective remove spares the sibling",
            category: "hard-remove",
            history: &[],
            seed: &[("Lunch with mom", 1, (12, 0), 1, (13, 0)), ("Lunch with Dan", 1, (15, 0), 1, (16, 0))],
            prompt: "Cancel lunch with Dan.",
            expect: E { min_removed: Some(1), removed_has: &["Dan"], survives: &["with mom"], ..Default::default() },
        },
        Case {
            name: "remove all of a kind",
            category: "hard-remove",
            history: &[],
            seed: &[("Sleepover at Jake's", 2, (20, 0), 3, (8, 0)), ("Sleepover at Mia's", 4, (20, 0), 5, (8, 0))],
            prompt: "Cancel all my sleepovers.",
            expect: E { min_removed: Some(2), ..Default::default() },
        },

        // ---- multi-intent: combinations in one message ----
        Case {
            name: "event + habit together",
            category: "multi-intent",
            history: &[],
            seed: &[],
            prompt: "Book a haircut Saturday at 11am, and remind me to stretch every morning.",
            expect: E { min_events: Some(1), min_habits: Some(1), title_has: &["haircut"], habit_has: &["stretch"], ..Default::default() },
        },
        Case {
            name: "remove + create together",
            category: "multi-intent",
            history: &[],
            seed: &[("Old sync", 1, (10, 0), 1, (10, 30))],
            prompt: "Cancel the old sync and schedule a team lunch Friday at noon.",
            expect: E { min_removed: Some(1), min_events: Some(1), title_has: &["lunch"], event_start_hm: &[("lunch", 12, 0)], ..Default::default() },
        },
        Case {
            name: "task + event + habit triple",
            category: "multi-intent",
            history: &[],
            seed: &[],
            prompt: "Finish the essay, about 2 hours, by Friday; dinner with my parents Sunday at 6pm; and journal every night.",
            expect: E { min_tasks: Some(1), min_events: Some(1), min_habits: Some(1), title_has: &["dinner"], habit_has: &["journal"], ..Default::default() },
        },
        Case {
            name: "edit + create together",
            category: "multi-intent",
            history: &[],
            seed: &[("Lunch", 0, (12, 0), 0, (13, 0))],
            prompt: "Move lunch to 1pm and add a coffee break at 3pm.",
            expect: E { min_updated: Some(1), min_events: Some(1), title_has: &["coffee"], event_start_hm: &[("coffee", 15, 0)], ..Default::default() },
        },

        // ---- restraint: queries, greetings, past-tense, venting (must create nothing) ----
        Case {
            name: "calendar query is not a command",
            category: "restraint",
            history: &[],
            seed: &[],
            prompt: "What's on my calendar tomorrow?",
            expect: E { custom: Some(created_nothing), ..Default::default() },
        },
        Case {
            name: "greeting creates nothing",
            category: "restraint",
            history: &[],
            seed: &[],
            prompt: "Hey! How's it going today?",
            expect: E { custom: Some(created_nothing), ..Default::default() },
        },
        Case {
            name: "past-tense is not a new task",
            category: "restraint",
            history: &[],
            seed: &[],
            prompt: "I already finished the laundry earlier.",
            expect: E { custom: Some(created_nothing), ..Default::default() },
        },
        Case {
            name: "venting creates nothing",
            category: "restraint",
            history: &[],
            seed: &[],
            prompt: "Honestly I'm feeling pretty overwhelmed this week.",
            expect: E { custom: Some(created_nothing), ..Default::default() },
        },

        // ---- context: a follow-up edit referring to the prior turn ----
        Case {
            name: "follow-up edit via history",
            category: "context",
            history: &[("user", "Add a dentist appointment Friday at 2pm."), ("assistant", "Added Dentist on Friday at 2pm.")],
            seed: &[("Dentist", 2, (14, 0), 2, (15, 0))],
            prompt: "Actually, make it 3pm instead.",
            expect: E { min_updated: Some(1), event_start_hm: &[("dentist", 15, 0)], ..Default::default() },
        },

        // ============================================================================
        //  WAVE 2 — push harder: relative dates, odd time formats, messy input, multi-edits.
        // ============================================================================

        // ---- relative dates beyond weekday (likely a deterministic-track target) ----
        Case {
            name: "the day after tomorrow",
            category: "rel-date",
            history: &[],
            seed: &[],
            prompt: "Lunch the day after tomorrow at 1pm.",
            expect: E { events: Some(1), event_day_offset: &[("lunch", 2)], event_start_hm: &[("lunch", 13, 0)], ..Default::default() },
        },
        Case {
            name: "in two weeks",
            category: "rel-date",
            history: &[],
            seed: &[],
            prompt: "Schedule a project review in two weeks at 2pm.",
            expect: E { events: Some(1), event_day_offset: &[("review", 14)], ..Default::default() },
        },

        // ---- odd / military time formats ----
        Case {
            name: "military time",
            category: "odd-time",
            history: &[],
            seed: &[],
            prompt: "Flight tomorrow at 1400.",
            expect: E { events: Some(1), event_start_hm: &[("flight", 14, 0)], ..Default::default() },
        },
        Case {
            name: "leading-zero 24h range",
            category: "odd-time",
            history: &[],
            seed: &[],
            prompt: "Block 0900-1030 for deep work tomorrow.",
            expect: E { events: Some(1), event_start_hm: &[("deep", 9, 0)], event_minutes: &[("deep", 90)], ..Default::default() },
        },
        Case {
            name: "duration-only break",
            category: "odd-time",
            history: &[],
            seed: &[],
            prompt: "Take a 30 minute break at 2pm today.",
            expect: E { events: Some(1), event_start_hm: &[("break", 14, 0)], event_minutes: &[("break", 30)], ..Default::default() },
        },

        // ---- messy / sms-speak ----
        Case {
            name: "sms-speak appointment",
            category: "messy",
            history: &[],
            seed: &[],
            prompt: "dr appt tmrw 3p",
            expect: E { events: Some(1), event_day_offset: &[("", 1)], event_start_hm: &[("", 15, 0)], ..Default::default() },
        },
        Case {
            name: "abbreviated meeting",
            category: "messy",
            history: &[],
            seed: &[],
            prompt: "mtg w/ sarah mon 10a",
            expect: E { events: Some(1), event_start_hm: &[("", 10, 0)], ..Default::default() },
        },

        // ---- "weekly" wording that is actually ONE event, not a habit ----
        Case {
            name: "weekly sync is a single event, not a habit",
            category: "routing",
            history: &[],
            seed: &[],
            prompt: "Set up our weekly sync this Thursday at 2pm.",
            expect: E { events: Some(1), habits: Some(0), event_start_hm: &[("sync", 14, 0)], ..Default::default() },
        },

        // ---- multiple edits in one message ----
        Case {
            name: "two edits at once",
            category: "multi-edit",
            history: &[],
            seed: &[("Dentist", 1, (14, 0), 1, (15, 0)), ("Workshop", 1, (10, 0), 1, (11, 0))],
            prompt: "Move the dentist to 3pm and make the workshop 2 hours.",
            expect: E { min_updated: Some(2), event_start_hm: &[("dentist", 15, 0)], event_minutes: &[("workshop", 120)], ..Default::default() },
        },

        // ---- mid-sentence self-correction → ONE event at the corrected time ----
        Case {
            name: "self-correction keeps the final value",
            category: "correction",
            history: &[],
            seed: &[],
            prompt: "Schedule a call at 3pm today, actually make it 4pm.",
            expect: E { events: Some(1), event_start_hm: &[("call", 16, 0)], ..Default::default() },
        },

        // ---- per-task distinct deadlines ----
        Case {
            name: "two tasks, two different deadlines",
            category: "hard-task",
            history: &[],
            seed: &[],
            prompt: "Finish chapter 1 by Monday and chapter 2 by Wednesday.",
            expect: E { min_tasks: Some(2), custom: Some(all_tasks_have_deadline), ..Default::default() },
        },

        // ---- a long, mixed brain-dump: event + task + habit + remove in one message ----
        Case {
            name: "long mixed brain-dump",
            category: "multi-intent",
            history: &[],
            seed: &[("Old standup", 1, (9, 0), 1, (9, 30))],
            prompt: "Ok my week: dentist Monday at 9am, finish the budget report (about 3 hours) by Wednesday, go for a run every morning, and cancel the old standup.",
            // "dent" stem: the model titles this "Dentist" or "Dental" run-to-run; both are correct.
            expect: E { min_events: Some(1), min_tasks: Some(1), min_habits: Some(1), min_removed: Some(1), title_has: &["dent"], habit_has: &["run"], ..Default::default() },
        },
    ]
}

// ---------------- runner ----------------

#[tokio::test]
#[ignore = "needs a live llama-server; run with --ignored --nocapture while Pushin is open"]
async fn llm_eval() {
    let base = std::env::var("PUSHIN_LLM_URL").unwrap_or_else(|_| "http://127.0.0.1:8080".into());
    let model = std::env::var("PUSHIN_LLM_MODEL").unwrap_or_else(|_| Settings::default().model_id);

    let client = reqwest::Client::builder().timeout(Duration::from_secs(180)).build().unwrap();

    // Self-skip if nothing is listening, so the harness is safe to run blindly.
    if client.get(format!("{base}/v1/models")).timeout(Duration::from_secs(3)).send().await.is_err() {
        eprintln!("\n⚠️  No llama-server reachable at {base}. Open Pushin (or set PUSHIN_LLM_URL) and re-run. Skipping.\n");
        return;
    }

    println!("\n=== Pushin LLM eval — model: {model} @ {base} ===\n");

    let mut by_cat: std::collections::BTreeMap<&str, (usize, usize)> = Default::default();
    let (mut pass_total, mut total_total) = (0usize, 0usize);

    for (i, case) in cases().into_iter().enumerate() {
        // Fresh throwaway DB per case so seeds and dedupe never bleed across cases.
        let path = std::env::temp_dir().join(format!("pushin_eval_{}_{}.db", std::process::id(), i));
        let _ = std::fs::remove_file(&path);
        let conn = db::open(&path).expect("open temp db");

        let mut settings = Settings::default();
        settings.llm_base_url = base.clone();
        settings.model_id = model.clone();
        settings.sleep_enabled = false; // clean baseline (no routine context in the prompt)

        for (title, sdo, (sh, sm), edo, (eh, em)) in case.seed {
            db::insert_event(&conn, title, &iso(*sdo, *sh, *sm), &iso(*edo, *eh, *em), "fixed").unwrap();
        }
        let current: Vec<Event> = db::list_events(&conn).unwrap_or_default();
        let history: Vec<ChatTurn> = case.history.iter().map(|(r, c)| ChatTurn { role: (*r).into(), content: (*c).into() }).collect();

        // PUSHIN_EVAL_UNION=1 → evaluate the single-call union path (what a fine-tuned student runs).
        // PUSHIN_EVAL_ROUTER=1 → force the router pipeline (classify + per-intent extract w/ dynamic
        // few-shot), which `plan()` now keeps only as an error fallback — used to A/B router vs union.
        // Default (no env) → whatever `plan()` ships (currently union-first). All end in store_plan + recovery.
        let union_mode = std::env::var("PUSHIN_EVAL_UNION").is_ok();
        let router_mode = std::env::var("PUSHIN_EVAL_ROUTER").is_ok();
        let plan_result = if union_mode {
            parser::union_label(&client, &settings, &current, &history, case.prompt).await.map(|(_, _, p)| p)
        } else if router_mode {
            parser::route_eval(&client, &settings, &current, &history, case.prompt).await
        } else {
            parser::plan(&client, &settings, &current, &history, case.prompt, &[]).await
        };
        let (checks, outcome): (Vec<(String, bool)>, Option<PlanOutcome>) =
            match plan_result {
                Ok(plan) => match parser::store_plan(&conn, &settings, &plan) {
                    Ok(o) => (evaluate(&case.expect, &o, &conn), Some(o)),
                    Err(e) => (vec![chk(format!("store_plan errored: {e}"), false)], None),
                },
                Err(e) => (vec![chk(format!("plan errored: {e}"), false)], None),
            };

        let passed = checks.iter().filter(|(_, ok)| *ok).count();
        let total = checks.len().max(1);
        let mark = if passed == total { "✓" } else { "✗" };
        println!("{mark} [{}] {} — {passed}/{total}", case.category, case.name);
        for (label, ok) in &checks {
            println!("      {} {}", if *ok { "✓" } else { "✗" }, label);
        }
        println!("      prompt: {:?}", case.prompt);
        // What the model actually produced — the diagnostic for any ✗ above.
        if let Some(o) = &outcome {
            println!(
                "      → events:{:?} updated:{:?} removed:{:?} tasks:{} habits:{:?} clar:{:?}",
                o.created_event_titles, o.updated_event_titles, o.removed_event_titles, o.created_task_ids.len(), o.created_habit_names, o.clarifications
            );
        }

        let e = by_cat.entry(case.category).or_default();
        e.0 += passed;
        e.1 += total;
        pass_total += passed;
        total_total += total;

        drop(conn);
        let _ = std::fs::remove_file(&path);
    }

    println!("\n--- by category ---");
    for (cat, (p, t)) in &by_cat {
        println!("  {cat:<12} {p}/{t}  ({:.0}%)", 100.0 * *p as f64 / *t as f64);
    }
    let pct = 100.0 * pass_total as f64 / total_total.max(1) as f64;
    println!("\n=== TOTAL: {pass_total}/{total_total} checks  ({pct:.0}%) ===\n");
}
