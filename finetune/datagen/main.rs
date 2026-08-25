//! Pushin fine-tuning **data generator** — Stages 1–4 of `finetune/PLAN.md`.
//!
//! For each generated template (a prompt with a *known* correct outcome) it:
//!   1. seeds a throwaway SQLite calendar,
//!   2. asks the **teacher** model for a label via the single-call union path (`parser::union_label`),
//!   3. runs the real `store_plan` + the template's `check` closure (reject-sampling), and
//!   4. on success, writes a ChatML row `{messages:[system,(history),user,assistant]}` to the SFT file.
//!
//! The label is the teacher's RAW schema JSON — exactly what the fine-tuned student should emit, so
//! training and inference share one format. Train/holdout is a stable hash split (no eval leakage).
//!
//! Run offline to inspect prompts:   `cargo run --example datagen -- --dry-run`
//! Run against a teacher llama-server (Qwen2.5-14B-Instruct on :8080 by default):
//!     `PUSHIN_TEACHER_MODEL=qwen2.5-14b-instruct-q4_k_m cargo run --example datagen`

mod templates;

use std::collections::{BTreeMap, HashSet};
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::time::Duration as StdDuration;

use chrono::{Duration, Local};
use pushin_lib::db;
use pushin_lib::model::Settings;
use pushin_lib::parser::{self, ChatTurn, ParsedPlan, PlanOutcome};
use rusqlite::Connection;
use serde_json::{json, Value};

fn arg(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned()
}

fn iso(off: i64, h: u32, m: u32) -> String {
    (Local::now().naive_local().date() + Duration::days(off)).and_hms_opt(h, m, 0).unwrap().format("%Y-%m-%dT%H:%M:%S").to_string()
}

/// Stable train/holdout split by prompt hash, so re-runs keep the same examples on the same side.
fn is_holdout(prompt: &str, frac: f64) -> bool {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    prompt.hash(&mut h);
    (h.finish() % 1000) as f64 / 1000.0 < frac
}

/// Strip fields the model never emits under `response_schema` from a serialized `ParsedPlan`, so the
/// label matches what the student should produce: drop the recovery-only / non-schema keys
/// (`span_days`, `shift_minutes`, `notes`) and all null optionals, recursively.
fn clean_label(v: &mut Value) {
    match v {
        Value::Object(map) => {
            for k in ["span_days", "shift_minutes", "notes"] {
                map.remove(k);
            }
            map.retain(|_, val| !val.is_null());
            for val in map.values_mut() {
                clean_label(val);
            }
        }
        Value::Array(arr) => arr.iter_mut().for_each(clean_label),
        _ => {}
    }
}

/// Serialize the router's recovered plan into a clean, minimal union-schema label.
fn plan_to_label(plan: &pushin_lib::parser::ParsedPlan) -> Value {
    let mut v = serde_json::to_value(plan).unwrap_or_else(|_| json!({}));
    clean_label(&mut v);
    if let Some(o) = v.as_object_mut() {
        // Every template is a complete instruction → no clarifications wanted (see build_row).
        o.insert("clarifications".into(), json!([]));
        // Drop optional arrays that are empty so the label matches the model's natural minimal style.
        for empty in ["updateEvents", "removeEvents", "habits"] {
            if o.get(empty).and_then(|x| x.as_array()).map(|a| a.is_empty()).unwrap_or(false) {
                o.remove(empty);
            }
        }
        // The schema requires these even when empty.
        o.entry("events").or_insert_with(|| json!([]));
        o.entry("projects").or_insert_with(|| json!([]));
    }
    v
}

/// Store a candidate plan into a fresh seeded scratch DB and run the template's check. Each
/// candidate needs its own DB since `store_plan` mutates. Returns (passed, outcome-for-logging).
fn validate(
    seed: &[templates::SeedEvent],
    settings: &Settings,
    plan: &ParsedPlan,
    check: &dyn Fn(&PlanOutcome, &Connection) -> bool,
    tag: &str,
) -> (bool, Option<PlanOutcome>) {
    let path = std::env::temp_dir().join(format!("pushin_val_{}_{}.db", std::process::id(), tag));
    let _ = std::fs::remove_file(&path);
    let conn = match db::open(&path) {
        Ok(c) => c,
        Err(_) => return (false, None),
    };
    for s in seed {
        db::insert_event(&conn, &s.title, &iso(s.s_off, s.sh, s.sm), &iso(s.e_off, s.eh, s.em), "fixed").ok();
    }
    let res = match parser::store_plan(&conn, settings, plan) {
        Ok(o) => (check(&o, &conn), Some(o)),
        Err(_) => (false, None),
    };
    drop(conn);
    let _ = std::fs::remove_file(&path);
    res
}

fn build_row(messages: &Value, raw: &Value, category: &str) -> String {
    let mut arr = messages.as_array().cloned().unwrap_or_default();
    // Clean the label: every template is a COMPLETE instruction, so the ideal output asks for
    // nothing. The teacher (even the 14B) sometimes appends a forbidden "what's the duration?"
    // clarification that the runtime would filter out anyway — training on it would teach the
    // student to ask unwanted questions, so we blank it. (Only reaches here for labels that already
    // passed the check, i.e. the event/task/habit was placed correctly.)
    let mut label = raw.clone();
    if let Some(obj) = label.as_object_mut() {
        obj.insert("clarifications".into(), json!([]));
    }
    let label = serde_json::to_string(&label).unwrap_or_else(|_| "{}".into());
    arr.push(json!({ "role": "assistant", "content": label }));
    serde_json::to_string(&json!({ "messages": arr, "category": category })).unwrap_or_default()
}

/// Experiment #2: rewrite a templated prompt into a realistic, human-typed phrasing via a strong teacher,
/// preserving every scheduling fact so the template's known-truth `check` still validates the label.
/// Direct OpenAI-compatible chat call (prose, not schema-constrained). Returns None on any failure →
/// caller falls back to the original template prompt.
async fn paraphrase_prompt(client: &reqwest::Client, base: &str, model: &str, original: &str) -> Option<String> {
    let system = "You rewrite a scheduling request the way a real person casually types it to their \
calendar assistant. Keep EVERY fact identical: the same activities, people, places, exact day words, and \
exact times/durations — never add, drop, merge, or split anything. Vary the wording naturally \
(contractions, filler like \"gotta\"/\"need to\", different sentence order, maybe one tiny typo). Output \
ONLY the rewritten message on one line — no quotes, no preamble, no explanation.";
    let body = json!({
        "model": model,
        "temperature": 0.8,
        "max_tokens": 200,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": original},
        ]
    });
    let url = format!("{}/v1/chat/completions", base.trim_end_matches('/'));
    let v: Value = client.post(&url).json(&body).send().await.ok()?.json().await.ok()?;
    let text = v["choices"][0]["message"]["content"].as_str()?;
    let line = text.trim().lines().find(|l| !l.trim().is_empty())?.trim().trim_matches('"').trim().to_string();
    (!line.is_empty() && line.len() <= 300).then_some(line)
}

/// Normalize a prompt for leakage comparison: lowercase, keep only alphanumerics, single-space words.
fn normalize_prompt(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = true; // trim leading
    for ch in s.to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            prev_space = false;
        } else if !prev_space {
            out.push(' ');
            prev_space = true;
        }
    }
    out.trim_end().to_string()
}

/// The held-out battery's prompts (parsed from `src-tauri/tests/llm_eval.rs`), normalized. Used to
/// DENY any templated candidate that matches — the eval battery must NEVER appear in training data
/// (see docs/notes/GUARDS_TO_99_PLAN.md A1). Self-maintaining: always reflects the current battery.
fn load_eval_prompts() -> HashSet<String> {
    let src = ["src-tauri/tests/llm_eval.rs", "tests/llm_eval.rs", "../src-tauri/tests/llm_eval.rs"]
        .iter()
        .find_map(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_default();
    let mut set = HashSet::new();
    for chunk in src.split("prompt:").skip(1) {
        // Extract the first double-quoted string literal on this `prompt:` line (bail at newline).
        let mut chars = chunk.chars();
        let mut opened = false;
        for c in chars.by_ref() {
            if c == '"' {
                opened = true;
                break;
            }
            if c == '\n' {
                break;
            }
        }
        if !opened {
            continue;
        }
        let (mut s, mut esc) = (String::new(), false);
        for c in chars {
            if esc {
                s.push(if c == 'n' { ' ' } else { c });
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                break;
            } else {
                s.push(c);
            }
        }
        let n = normalize_prompt(&s);
        if !n.is_empty() {
            set.insert(n);
        }
    }
    set
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out_path = arg(&args, "--out").unwrap_or_else(|| "finetune/data/dataset.jsonl".into());
    let holdout_path = arg(&args, "--holdout").unwrap_or_else(|| "finetune/data/holdout.jsonl".into());
    let limit: usize = arg(&args, "--limit").and_then(|s| s.parse().ok()).unwrap_or(usize::MAX);
    let holdout_frac: f64 = arg(&args, "--holdout-frac").and_then(|s| s.parse().ok()).unwrap_or(0.1);
    let dry_run = args.iter().any(|a| a == "--dry-run");
    let show_rejects = args.iter().any(|a| a == "--show-rejects");
    let only = arg(&args, "--only"); // restrict to one category (debugging)

    // Flags win over env (WSL→Windows doesn't forward env vars to the exe), env over default.
    let base = arg(&args, "--teacher-url")
        .or_else(|| std::env::var("PUSHIN_TEACHER_URL").ok())
        .unwrap_or_else(|| "http://127.0.0.1:8080".into());
    let model = arg(&args, "--teacher-model")
        .or_else(|| std::env::var("PUSHIN_TEACHER_MODEL").ok())
        .unwrap_or_else(|| "qwen2.5-14b-instruct-q4_k_m".into());
    // Experiment #2: realistic-prompt mode. A strong teacher rewrites each templated prompt into a
    // human-typed phrasing (all scheduling facts preserved) so the template's known-truth check still
    // validates the label — a paraphrase that corrupts a fact just fails reject-sampling. Defaults to
    // the same teacher url/model; point --paraphrase-url/model at the 30B-A3B for the best realism.
    let paraphrase = args.iter().any(|a| a == "--paraphrase");
    let pp_base = arg(&args, "--paraphrase-url").or_else(|| std::env::var("PUSHIN_PARAPHRASE_URL").ok()).unwrap_or_else(|| base.clone());
    let pp_model = arg(&args, "--paraphrase-model").or_else(|| std::env::var("PUSHIN_PARAPHRASE_MODEL").ok()).unwrap_or_else(|| model.clone());

    let all = templates::all();
    // Anti-leakage (docs/notes/GUARDS_TO_99_PLAN.md A1): never train on a prompt that appears in the held-out
    // battery. Drop any templated candidate whose normalized text matches an `llm_eval.rs` prompt.
    let eval_prompts = load_eval_prompts();
    let leaked_dropped = all.iter().filter(|t| eval_prompts.contains(&normalize_prompt(&t.prompt))).count();
    if eval_prompts.is_empty() {
        eprintln!("⚠️  Anti-leakage: couldn't read llm_eval.rs — eval denylist is EMPTY. Run from the project root so the battery can be excluded from training.");
    } else {
        println!("Anti-leakage: {} battery prompts loaded; {leaked_dropped} templated candidate(s) denied (would have leaked into training).", eval_prompts.len());
    }
    let templates: Vec<templates::Template> = all
        .into_iter()
        .filter(|t| only.as_deref().map(|c| t.category == c).unwrap_or(true))
        .filter(|t| !eval_prompts.contains(&normalize_prompt(&t.prompt)))
        .take(limit)
        .collect();

    let mut total_by_cat: BTreeMap<&str, usize> = BTreeMap::new();
    for t in &templates {
        *total_by_cat.entry(t.category).or_default() += 1;
    }
    println!("Generated {} candidate prompts:", templates.len());
    for (cat, n) in &total_by_cat {
        println!("  {cat:<16} {n}");
    }

    if dry_run {
        println!("\n--- sample prompts (--dry-run, no teacher contacted) ---");
        let mut shown: BTreeMap<&str, usize> = BTreeMap::new();
        for t in &templates {
            let c = shown.entry(t.category).or_default();
            if *c < 3 {
                println!("  [{}] {}", t.category, t.prompt);
                *c += 1;
            }
        }
        return;
    }

    let client = reqwest::Client::builder().timeout(StdDuration::from_secs(180)).build().unwrap();
    if client.get(format!("{base}/v1/models")).timeout(StdDuration::from_secs(3)).send().await.is_err() {
        eprintln!("\n⚠️  No teacher LLM reachable at {base}.");
        eprintln!("    Serve Qwen2.5-14B-Instruct (GGUF) on :8080, or set PUSHIN_TEACHER_URL/PUSHIN_TEACHER_MODEL.");
        eprintln!("    (Tip: `cargo run --example datagen -- --dry-run` inspects prompts without a server.)\n");
        std::process::exit(1);
    }

    if let Some(parent) = std::path::Path::new(&out_path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut train = File::create(&out_path).expect("create dataset file");
    let mut hold = File::create(&holdout_path).expect("create holdout file");

    println!("\nLabeling with teacher {model} @ {base} …\n");
    let (mut kept, mut failed, mut errored) = (0usize, 0usize, 0usize);
    let mut kept_by_cat: BTreeMap<&str, usize> = BTreeMap::new();

    let mut settings = Settings::default();
    settings.llm_base_url = base.clone();
    settings.model_id = model.clone();
    settings.sleep_enabled = false;
    settings.embed_model = String::new(); // no embed server in datagen → static few-shot exemplars

    for (i, t) in templates.iter().enumerate() {
        // The calendar the model sees: seed a scratch DB, list it back, discard.
        let current = {
            let p = std::env::temp_dir().join(format!("pushin_seed_{}_{}.db", std::process::id(), i));
            let _ = std::fs::remove_file(&p);
            let c = db::open(&p).expect("open temp db");
            for s in &t.seed {
                db::insert_event(&c, &s.title, &iso(s.s_off, s.sh, s.sm), &iso(s.e_off, s.eh, s.em), "fixed").ok();
            }
            let ev = db::list_events(&c).unwrap_or_default();
            drop(c);
            let _ = std::fs::remove_file(&p);
            ev
        };
        let history: Vec<ChatTurn> = t.history.iter().map(|(r, c)| ChatTurn { role: r.clone(), content: c.clone() }).collect();
        // Selective paraphrase: the LLM rewrite adds realistic human phrasing (great for create/multi
        // prompts) but it DRIFTS the exact facts that carry the truth in date/edit/remove categories —
        // "in 10 days" → "next week", "cancel X" softened — and reject-sampling then drops every drifted
        // label (rel-date/remove cratered to 0% with blanket paraphrase). So skip paraphrase for those
        // truth-fragile categories; keep it where the facts survive a reword.
        const PARAPHRASE_SKIP: &[&str] = &["rel-date", "remove", "correction", "multiday-window", "edit", "day-of-month"];
        let prompt_text = if paraphrase && !PARAPHRASE_SKIP.contains(&t.category) {
            paraphrase_prompt(&client, &pp_base, &pp_model, &t.prompt).await.unwrap_or_else(|| t.prompt.clone())
        } else {
            t.prompt.clone()
        };
        let messages = parser::union_messages(&settings, &current, &history, &prompt_text);

        // Best-of-two teachers: the ROUTER routes tasks/removes/titles correctly; the single UNION
        // call handles date/time edge cases the router fumbles (overnight splits, relative/ordinal
        // dates). Try the router first, fall back to union; keep whichever the check verifies.
        let mut chosen: Option<Value> = None;
        let mut any_ok = false;
        let mut reject_dbg: Option<(Value, Option<PlanOutcome>)> = None;

        if let Ok(plan) = parser::plan(&client, &settings, &current, &history, &prompt_text, &[]).await {
            any_ok = true;
            let (pass, outcome) = validate(&t.seed, &settings, &plan, &*t.check, &format!("{i}r"));
            let label = plan_to_label(&plan);
            if pass {
                chosen = Some(label);
            } else {
                reject_dbg = Some((label, outcome));
            }
        }
        if chosen.is_none() {
            if let Ok((_, _raw, plan)) = parser::union_label(&client, &settings, &current, &history, &prompt_text).await {
                any_ok = true;
                let (pass, outcome) = validate(&t.seed, &settings, &plan, &*t.check, &format!("{i}u"));
                let label = plan_to_label(&plan);
                if pass {
                    chosen = Some(label);
                } else {
                    reject_dbg = Some((label, outcome));
                }
            }
        }

        match chosen {
            Some(label) => {
                let row = build_row(&messages, &label, t.category);
                let sink = if is_holdout(&t.prompt, holdout_frac) { &mut hold } else { &mut train };
                let _ = writeln!(sink, "{row}");
                kept += 1;
                *kept_by_cat.entry(t.category).or_default() += 1;
            }
            None if !any_ok => errored += 1,
            None => {
                failed += 1;
                if show_rejects {
                    eprintln!("  ✗ [{}] {:?}", t.category, t.prompt);
                    if let Some((label, outcome)) = &reject_dbg {
                        eprintln!("      label: {}", serde_json::to_string(label).unwrap_or_default());
                        if let Some(o) = outcome {
                            eprintln!(
                                "      → events:{:?} updated:{:?} removed:{:?} tasks:{} habits:{:?}",
                                o.created_event_titles, o.updated_event_titles, o.removed_event_titles, o.created_task_ids.len(), o.created_habit_names
                            );
                        }
                    }
                }
            }
        }

        if (i + 1) % 25 == 0 {
            eprintln!("  …{}/{} processed (kept {kept}, rejected {failed}, errors {errored})", i + 1, templates.len());
        }
    }

    println!("\n--- kept by category (passed the teacher + reject-sampling gate) ---");
    for (cat, total) in &total_by_cat {
        let k = kept_by_cat.get(cat).copied().unwrap_or(0);
        println!("  {cat:<16} {k}/{total}  ({:.0}%)", 100.0 * k as f64 / *total as f64);
    }
    println!(
        "\n=== kept {kept}, rejected {failed} (teacher got it wrong), errors {errored}.  →  {out_path} (+ holdout {holdout_path}) ===\n"
    );
}
