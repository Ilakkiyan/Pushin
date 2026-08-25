//! REAL-WORLD battery — prompts shaped like how the user actually types (long, rambling, ambiguous,
//! overnight, multi-event), NOT the clean short prompts in `llm_eval.rs`. Scored PASS/FAIL against the
//! STORED calendar (what the user sees) so the number reflects reality. `#[ignore]`d + self-skips.
//!
//!   cargo test --test real_world_eval -- --ignored --nocapture   (with a llama-server on :8080)
//!
//! Each case's check runs on the events/tasks/habits after the full parse→store pipeline. Some checks
//! encode "clearly correct" behavior; where a prompt is genuinely ambiguous the check is lenient (e.g.
//! "not scheduled at 11pm") so a FAIL means a real, user-visible wrongness.

use std::time::Duration;

use chrono::{Duration as CD, Local, NaiveDateTime, Timelike};
use pushin_lib::model::{Event, Settings, Task};
use pushin_lib::parser::{self, ChatTurn};
use pushin_lib::{db, model::Habit};

struct Case {
    tag: &'static str,
    seed: &'static [(&'static str, i64, (u32, u32), (u32, u32))], // title, day-offset, start(h,m), end(h,m)
    prompt: &'static str,
    // (pass, detail)
    check: fn(&[Event], &[Task], &[Habit]) -> (bool, String),
}

fn pdt(s: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").ok()
}
fn dur_min(e: &Event) -> i64 {
    match (pdt(&e.start), pdt(&e.end)) {
        (Some(s), Some(en)) => (en - s).num_minutes(),
        _ => 0,
    }
}
fn find<'a>(evs: &'a [Event], needle: &str) -> Option<&'a Event> {
    evs.iter().find(|e| e.title.to_lowercase().contains(needle))
}
fn start_hour(e: &Event) -> u32 {
    pdt(&e.start).map(|d| d.hour()).unwrap_or(99)
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            tag: "ramble-6-intent",
            seed: &[],
            prompt: "im gonna play minecraft w my friends from like 1 - 3, call my gf from 3-5, then study from 5-7, gym from 7-8, study again from 8-10, then take an exam from 10-11",
            check: |ev, _t, _h| {
                let n = ["minecraft", "gf", "girlfriend", "study", "gym", "exam"].iter().filter(|k| find(ev, k).is_some()).count();
                (ev.len() >= 5, format!("{} events, {} key subjects present", ev.len(), n))
            },
        },
        Case {
            tag: "overnight-sleepover",
            seed: &[],
            prompt: "on saturday i have a sleepover from 7pm to 8 am the next day",
            check: |ev, _t, _h| {
                match find(ev, "sleepover") {
                    Some(e) => {
                        let d = dur_min(e);
                        (start_hour(e) == 19 && (700..=800).contains(&d), format!("start {}:00, duration {}m (want 19:00, ~780m)", start_hour(e), d))
                    }
                    None => (false, "no sleepover event".into()),
                }
            },
        },
        Case {
            tag: "weekend-preamble-no-habit",
            seed: &[],
            prompt: "i got a pretty packed weekend, on saturday i have a sleepover from 7pm to 8am",
            check: |_ev, _t, h| (h.is_empty(), format!("{} habits (want 0 — 'weekend' is descriptive)", h.len())),
        },
        Case {
            tag: "gf-morning-range-not-late",
            seed: &[],
            prompt: "im gonna call my gf from like 11 - 1 tmr",
            check: |ev, _t, _h| match find(ev, "gf").or_else(|| find(ev, "girlfriend")).or_else(|| find(ev, "call")) {
                Some(e) => (start_hour(e) <= 13, format!("start {}:00 (want daytime <=13, not 23)", start_hour(e))),
                None => (false, "no call event".into()),
            },
        },
        Case {
            tag: "world-cup-long-range",
            seed: &[],
            prompt: "on sunday from 2 to 11:30pm im watching the world cup finals",
            check: |ev, _t, _h| match find(ev, "world cup").or_else(|| find(ev, "final")) {
                Some(e) => {
                    let d = dur_min(e);
                    (start_hour(e) == 14 && (560..=580).contains(&d), format!("start {}:00 dur {}m (want 14:00, ~570m)", start_hour(e), d))
                }
                None => (false, "no world cup event".into()),
            },
        },
        Case {
            tag: "pm-less-lunch-range",
            seed: &[],
            prompt: "lunch meeting from 12 to 2",
            check: |ev, _t, _h| match ev.first() {
                Some(e) => (start_hour(e) == 12 && dur_min(e) == 120, format!("start {}:00 dur {}m (want 12:00, 120m)", start_hour(e), dur_min(e))),
                None => (false, "no event".into()),
            },
        },
        Case {
            tag: "controller-ramble-task",
            seed: &[],
            prompt: "i think veer's gonna come over to drop off his controller so i need to fix that controller at some point before the sleepover on saturday",
            check: |_ev, t, h| (t.len() >= 1 && t.len() <= 2 && h.is_empty(), format!("{} tasks {} habits (want 1-2 tasks, 0 habits)", t.len(), h.len())),
        },
        Case {
            tag: "decomposition-restraint",
            seed: &[],
            prompt: "i really need to finish my history essay by friday",
            check: |_ev, t, _h| (t.len() == 1, format!("{} tasks (want 1 — no fabricated subtasks)", t.len())),
        },
        Case {
            tag: "recurring-habit-still-works",
            seed: &[],
            prompt: "go to the gym every monday and wednesday",
            check: |_ev, _t, h| (h.len() == 1, format!("{} habits (want 1 — real recurrence)", h.len())),
        },
        Case {
            tag: "clean-single-event-control",
            seed: &[],
            prompt: "dentist tomorrow at 3pm",
            check: |ev, _t, _h| match find(ev, "dentist") {
                Some(e) => (start_hour(e) == 15, format!("start {}:00 (want 15:00)", start_hour(e))),
                None => (false, "no dentist event".into()),
            },
        },
        Case {
            tag: "cancel-existing",
            seed: &[("Dentist", 1, (14, 0), (15, 0))],
            prompt: "actually cancel my dentist appointment",
            check: |ev, _t, _h| (find(ev, "dentist").is_none(), format!("{} events remain (want dentist removed)", ev.len())),
        },
        Case {
            tag: "two-clean-events",
            seed: &[],
            prompt: "lunch with mom friday 12-2 and a graduation party from 6-10",
            check: |ev, _t, _h| {
                let ok = find(ev, "lunch").map(|e| dur_min(e) == 120).unwrap_or(false)
                    && find(ev, "party").or_else(|| find(ev, "graduation")).map(|e| dur_min(e) == 240 && start_hour(e) == 18).unwrap_or(false);
                (ok, format!("{} events", ev.len()))
            },
        },
    ]
}

fn iso(off: i64, h: u32, m: u32) -> String {
    (Local::now().naive_local().date() + CD::days(off)).and_hms_opt(h, m, 0).unwrap().format("%Y-%m-%dT%H:%M:%S").to_string()
}

#[tokio::test]
#[ignore]
async fn real_world_eval() {
    let base = std::env::var("PUSHIN_LLM_URL").unwrap_or_else(|_| "http://127.0.0.1:8080".into());
    let model = std::env::var("PUSHIN_LLM_MODEL").unwrap_or_else(|_| Settings::default().model_id);
    let client = reqwest::Client::builder().timeout(Duration::from_secs(180)).build().unwrap();
    if client.get(format!("{base}/v1/models")).timeout(Duration::from_secs(3)).send().await.is_err() {
        eprintln!("\n⚠️  No llama-server at {base}. Skipping.\n");
        return;
    }
    println!("\n═══ REAL-WORLD BATTERY — {model} @ {base} ═══\n");
    let mut pass = 0;
    let all = cases();
    for (i, c) in all.iter().enumerate() {
        let path = std::env::temp_dir().join(format!("pushin_rw_{}_{}.db", std::process::id(), i));
        let _ = std::fs::remove_file(&path);
        let conn = db::open(&path).unwrap();
        let mut s = Settings::default();
        s.llm_base_url = base.clone();
        s.model_id = model.clone();
        s.sleep_enabled = false;
        for (t, d, (sh, sm), (eh, em)) in c.seed {
            db::insert_event(&conn, t, &iso(*d, *sh, *sm), &iso(*d, *eh, *em), "fixed").unwrap();
        }
        let current: Vec<Event> = db::list_events(&conn).unwrap_or_default();
        let hist: Vec<ChatTurn> = vec![];
        match parser::plan(&client, &s, &current, &hist, c.prompt, &[]).await {
            Ok(plan) => {
                let _ = parser::store_plan(&conn, &s, &plan);
                let evs = db::list_events(&conn).unwrap_or_default();
                let tasks = db::list_tasks(&conn).unwrap_or_default();
                let habits = db::list_habits(&conn).unwrap_or_default();
                let (ok, detail) = (c.check)(&evs, &tasks, &habits);
                if ok {
                    pass += 1;
                }
                println!("{}  [{}]  {}", if ok { "PASS" } else { "FAIL" }, c.tag, detail);
            }
            Err(e) => println!("FAIL  [{}]  plan error: {e}", c.tag),
        }
        let _ = std::fs::remove_file(&path);
    }
    println!("\n═══ REAL-WORLD TOTAL: {pass}/{} ═══\n", all.len());
}
