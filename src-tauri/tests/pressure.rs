//! Freeform PRESSURE TEST — throw weird, messy, rambling, conversational prompts at the *real* parse
//! pipeline (`parser::plan` + guards + `store_plan`) and DUMP what comes out. No pass/fail: a human
//! reads the output to see where the model + deterministic guards fold, so the failures become the next
//! guard/clarification targets. `#[ignore]`d + self-skips with no server.
//!
//!   cargo test --test pressure -- --ignored --nocapture   (with Pushin/a llama-server on :8080)

use std::time::Duration;

use chrono::{Duration as ChronoDuration, Local};
use pushin_lib::db;
use pushin_lib::model::Settings;
use pushin_lib::parser::{self, ChatTurn};

fn iso(day_off: i64, h: u32, m: u32) -> String {
    let d = (Local::now().naive_local().date() + ChronoDuration::days(day_off)).and_hms_opt(h, m, 0).unwrap();
    d.format("%Y-%m-%dT%H:%M:%S").to_string()
}

struct Case {
    tag: &'static str,
    history: &'static [(&'static str, &'static str)],
    seed: &'static [(&'static str, i64, (u32, u32), i64, (u32, u32))],
    prompt: &'static str,
}

fn cases() -> Vec<Case> {
    fn c(tag: &'static str, prompt: &'static str) -> Case {
        Case { tag, history: &[], seed: &[], prompt }
    }
    vec![
        // --- rambling / conversational, multiple buried intents ---
        c("ramble", "ok so this week is gonna be insane. monday i've got the dentist at 2, and honestly i should probably move my standup which is usually 9 to 9:30 to like 8:30 so i have time. also remind me to call my mom at some point she's been bugging me"),
        c("ramble", "ugh i'm so behind. i have a paper due monday, an exam on wednesday, and i promised i'd help my friend move on saturday. can you sort my life out"),
        c("emotional", "honestly i'm drowning. there's the board deck, the q3 numbers, and i still haven't booked the venue for the offsite which is in two weeks"),
        // --- contradiction / self-correction mid-sentence ---
        c("correction", "add lunch with sarah at 12, no wait make it 1, actually you know what just cancel it i'll grab something later"),
        c("correction", "put gym at 6am every day. hmm actually not weekends. and make it 45 minutes not an hour"),
        // --- weird times / dates ---
        c("weird-time", "quick 5 min sync with raj at quarter past 3 tomorrow"),
        c("weird-time", "dinner reservation at half past 7 on the 14th"),
        c("weird-date", "team retro every other tuesday at 4"),
        c("weird-date", "i'm off the grid from the 20th for a fortnight"),
        c("weird-date", "flight leaves 6:45am next friday, i need to leave for the airport 2 hours before"),
        // --- negation / exclusion ---
        c("negation", "block off my whole afternoon tomorrow except for the 2pm client call"),
        c("negation", "i'm free all day saturday except i have soccer 10 to noon"),
        // --- typos / voice-to-text ---
        c("typos", "meetign tmrw at 2 witht he team abt teh launch, shud take bout an hr"),
        c("typos", "add dctor appt wednsday afternon and remind me to pick up my perscription after"),
        // --- mixed task + event + habit in one ---
        c("mixed", "i want to start meditating every morning, i've got a 1:1 with my manager thursday at 11, and i need to submit my expenses by end of week"),
        // --- ambiguous / underspecified (should ASK, not guess) ---
        c("ambiguous", "schedule that thing we talked about"),
        c("ambiguous", "put it in for later"),
        c("ambiguous", "move it"),
        // --- overloaded multi-intent with a project ---
        c("overload", "cancel all my meetings friday, add a 3 hour deep work block friday morning, and i need to prep for the launch: write the blog post, line up 3 demos, and schedule the announcement email"),
        // --- unit / duration edge cases ---
        c("duration", "an all-day workshop on the 12th"),
        c("duration", "back to back interviews 1pm to 5pm, 30 min each"),
        // --- follow-up needing conversation history ---
        Case { tag: "history", history: &[("user", "can you set up a call with the investors?"), ("assistant", "Sure — what day and time?")], seed: &[], prompt: "thursday at 4, and make it 90 minutes" },
        // --- edit against existing calendar (seeded) ---
        Case { tag: "edit-ctx", history: &[], seed: &[("Standup", 1, (9, 0), 1, (9, 30)), ("Lunch with Dan", 1, (12, 0), 1, (13, 0))], prompt: "push my standup back 30 min and move lunch to 1:30" },
        // --- restraint: pure question, should NOT create anything ---
        c("restraint", "wait what do i even have going on tomorrow?"),
    ]
}

#[tokio::test]
#[ignore = "freeform pressure test; run with --ignored --nocapture while a llama-server is on :8080"]
async fn pressure() {
    let base = std::env::var("PUSHIN_LLM_URL").unwrap_or_else(|_| "http://127.0.0.1:8080".into());
    let model = std::env::var("PUSHIN_LLM_MODEL").unwrap_or_else(|_| Settings::default().model_id);
    let client = reqwest::Client::builder().timeout(Duration::from_secs(180)).build().unwrap();
    if client.get(format!("{base}/v1/models")).timeout(Duration::from_secs(3)).send().await.is_err() {
        eprintln!("\n⚠️  No llama-server at {base}. Skipping pressure test.\n");
        return;
    }
    println!("\n=== PRESSURE TEST — model: {model} @ {base} ===\n");

    for (i, case) in cases().into_iter().enumerate() {
        let path = std::env::temp_dir().join(format!("pushin_pressure_{}_{}.db", std::process::id(), i));
        let _ = std::fs::remove_file(&path);
        let conn = db::open(&path).expect("open temp db");
        let mut settings = Settings::default();
        settings.llm_base_url = base.clone();
        settings.model_id = model.clone();
        settings.sleep_enabled = false;
        for (title, sdo, (sh, sm), edo, (eh, em)) in case.seed {
            db::insert_event(&conn, title, &iso(*sdo, *sh, *sm), &iso(*edo, *eh, *em), "fixed").unwrap();
        }
        let current = db::list_events(&conn).unwrap_or_default();
        let history: Vec<ChatTurn> = case.history.iter().map(|(r, c)| ChatTurn { role: (*r).into(), content: (*c).into() }).collect();

        println!("── [{}] {:?}", case.tag, case.prompt);
        if !case.seed.is_empty() {
            println!("    seed: {}", current.iter().map(|e| format!("{}@{}", e.title, e.start)).collect::<Vec<_>>().join(", "));
        }
        match parser::plan(&client, &settings, &current, &history, case.prompt, &[]).await {
            Ok(plan) => match parser::store_plan(&conn, &settings, &plan) {
                Ok(o) => {
                    for e in db::list_events(&conn).unwrap_or_default() {
                        println!("    EVENT  {:?}  {} → {}", e.title, e.start, e.end);
                    }
                    for t in db::list_tasks(&conn).unwrap_or_default() {
                        println!("    TASK   {:?}  deadline={:?} est={}m prio={}", t.title, t.deadline, t.estimated_minutes, t.priority);
                    }
                    if !o.created_habit_names.is_empty() {
                        println!("    HABITS {:?}", o.created_habit_names);
                    }
                    if !o.updated_event_titles.is_empty() {
                        println!("    UPDATED {:?}", o.updated_event_titles);
                    }
                    if !o.removed_event_titles.is_empty() {
                        println!("    REMOVED {:?}", o.removed_event_titles);
                    }
                    if !o.clarifications.is_empty() {
                        println!("    ❓ ASKS {:?}", o.clarifications);
                    }
                }
                Err(e) => println!("    store_plan ERROR: {e}"),
            },
            Err(e) => println!("    plan ERROR: {e}"),
        }
        println!();
        drop(conn);
        let _ = std::fs::remove_file(&path);
    }
}
