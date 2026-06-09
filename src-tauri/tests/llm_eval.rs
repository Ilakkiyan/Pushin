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

use chrono::{Duration as ChronoDuration, Local, NaiveDateTime};
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
    min_updated: Option<usize>,
    min_removed: Option<usize>,
    /// Each needle must appear in some created/updated event title.
    title_has: &'static [&'static str],
    /// Each needle must appear in some created habit name.
    habit_has: &'static [&'static str],
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

        let (checks, outcome): (Vec<(String, bool)>, Option<PlanOutcome>) =
            match parser::plan(&client, &settings, &current, &history, case.prompt).await {
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
