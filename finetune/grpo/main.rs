//! GRPO reward scorer + training-prompt dumper (path B of the fine-tune plan).
//!
//! The reward is **verifiable ground truth**, not teacher imitation: a model completion is run
//! through the REAL pipeline (`apply_recovery` → `store_plan`) and scored by the template's own
//! `check` closure. Optimizing toward this can exceed the teacher's ceiling, because the signal is
//! "did it actually produce the right calendar", not "does it match a (capped) teacher's JSON".
//!
//! Reuses the datagen templates (they already carry per-prompt checks for the hard categories).
//!
//!   cargo run --example grpo -- --dump finetune/data/grpo_prompts.jsonl   # emit training prompts
//!   cargo run --example grpo -- --serve                                   # stdio reward server
//!
//! Reward server protocol (line-delimited JSON on stdin → one float per line on stdout):
//!   in:  {"idx": <usize>, "completion": "<model text>"}
//!   out: 0.0 (bad/empty) | 0.2 (valid JSON, check failed) | 1.0 (check passed)

#[path = "../datagen/templates.rs"]
mod templates;

use std::io::{BufRead, Write};

use chrono::{Duration, Local};
use pushin_lib::db;
use pushin_lib::model::Settings;
use pushin_lib::parser::{self, ChatTurn, ParsedPlan};
use serde_json::{json, Value};
use templates::Template;

fn iso(off: i64, h: u32, m: u32) -> String {
    (Local::now().naive_local().date() + Duration::days(off)).and_hms_opt(h, m, 0).unwrap().format("%Y-%m-%dT%H:%M:%S").to_string()
}

fn settings() -> Settings {
    let mut s = Settings::default();
    s.embed_model = String::new(); // recovery/store_plan only; no embeddings
    s
}

/// Pull the first balanced {...} object out of a model completion (it may wrap JSON in prose/fences).
fn extract_json(s: &str) -> Option<&str> {
    let bytes = s.as_bytes();
    let start = s.find('{')?;
    let mut depth = 0i32;
    let mut in_str = false;
    let mut esc = false;
    for i in start..bytes.len() {
        let c = bytes[i] as char;
        if in_str {
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
        } else {
            match c {
                '"' => in_str = true,
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(&s[start..=i]);
                    }
                }
                _ => {}
            }
        }
    }
    None
}

/// Verifiable reward for one completion against one template's check.
fn score(t: &Template, completion: &str, settings: &Settings) -> f64 {
    let json_str = match extract_json(completion) {
        Some(j) => j,
        None => return 0.0, // not even a JSON object
    };
    let mut plan: ParsedPlan = match serde_json::from_str(json_str) {
        Ok(p) => p,
        Err(_) => return 0.0,
    };
    let valid_json = 0.2; // partial credit: well-formed schema JSON, even if the check fails
    parser::apply_recovery(&mut plan, &t.prompt, Local::now().naive_local().date());

    let path = std::env::temp_dir().join(format!("pushin_grpo_{}.db", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let conn = match db::open(&path) {
        Ok(c) => c,
        Err(_) => return valid_json,
    };
    for s in &t.seed {
        db::insert_event(&conn, &s.title, &iso(s.s_off, s.sh, s.sm), &iso(s.e_off, s.eh, s.em), "fixed").ok();
    }
    let reward = match parser::store_plan(&conn, settings, &plan) {
        Ok(outcome) => {
            if (t.check)(&outcome, &conn) {
                1.0
            } else {
                valid_json
            }
        }
        Err(_) => valid_json,
    };
    drop(conn);
    let _ = std::fs::remove_file(&path);
    reward
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let templates = templates::all();
    let settings = settings();

    // --- dump mode: write GRPO training prompts (ChatML messages + the case idx) ---
    if let Some(pos) = args.iter().position(|a| a == "--dump") {
        let out_path = args.get(pos + 1).cloned().unwrap_or_else(|| "finetune/data/grpo_prompts.jsonl".into());
        if let Some(parent) = std::path::Path::new(&out_path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let mut f = std::fs::File::create(&out_path).expect("create grpo prompts file");
        for (i, t) in templates.iter().enumerate() {
            // The calendar the model sees (seed → list back).
            let p = std::env::temp_dir().join(format!("pushin_grpo_seed_{}_{}.db", std::process::id(), i));
            let _ = std::fs::remove_file(&p);
            let conn = db::open(&p).expect("open temp db");
            for s in &t.seed {
                db::insert_event(&conn, &s.title, &iso(s.s_off, s.sh, s.sm), &iso(s.e_off, s.eh, s.em), "fixed").ok();
            }
            let current = db::list_events(&conn).unwrap_or_default();
            drop(conn);
            let _ = std::fs::remove_file(&p);
            let history: Vec<ChatTurn> = t.history.iter().map(|(r, c)| ChatTurn { role: r.clone(), content: c.clone() }).collect();
            let messages = parser::union_messages(&settings, &current, &history, &t.prompt);
            let row = json!({ "idx": i, "category": t.category, "prompt": t.prompt, "messages": messages });
            let _ = writeln!(f, "{}", serde_json::to_string(&row).unwrap_or_default());
        }
        eprintln!("wrote {} GRPO prompts → {}", templates.len(), out_path);
        return;
    }

    // --- serve mode (default): line-delimited reward server over stdin/stdout ---
    eprintln!("grpo reward server ready ({} templates)", templates.len());
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) if !l.trim().is_empty() => l,
            _ => continue,
        };
        let reward = (|| {
            let req: Value = serde_json::from_str(&line).ok()?;
            let idx = req.get("idx")?.as_u64()? as usize;
            let completion = req.get("completion")?.as_str()?;
            let t = templates.get(idx)?;
            Some(score(t, completion, &settings))
        })()
        .unwrap_or(0.0);
        let _ = writeln!(stdout, "{reward}");
        let _ = stdout.flush();
    }
}
