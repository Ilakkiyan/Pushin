//! Natural-language → structured plan. The small LLM extracts events/tasks and a
//! **day phrase + time** (which it does well); Rust computes the actual calendar date
//! (which the model does badly). Dates are never trusted from the model.

use crate::model::{Event, Settings};
use crate::scheduler::{fmt_dt, parse_dt};
use crate::{hermes, llm, model_manager};
use anyhow::Result;
use chrono::{Datelike, Duration, Local, NaiveDate, NaiveDateTime, NaiveTime, Weekday};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::OnceLock;

/// One prior chat turn, passed in for conversational context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatTurn {
    pub role: String, // "user" | "assistant"
    pub content: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ParsedTask {
    pub title: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default = "default_minutes")]
    pub estimated_minutes: i64,
    #[serde(default)]
    pub deadline: Option<String>,
    #[serde(default = "default_priority")]
    pub priority: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default = "default_true")]
    pub chunkable: bool,
}

fn default_minutes() -> i64 {
    60
}
fn default_priority() -> String {
    "medium".into()
}
fn default_true() -> bool {
    true
}

/// A fixed calendar item. The model gives a day phrase + time; Rust resolves the date.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedEvent {
    pub title: String,
    /// "today" | "tomorrow" | weekday name, or null.
    #[serde(default)]
    pub day: Option<String>,
    /// Explicit "YYYY-MM-DD" if the user gave one.
    #[serde(default)]
    pub date: Option<String>,
    #[serde(default, rename = "startTime")]
    pub start_time: Option<String>,
    #[serde(default, rename = "endTime")]
    pub end_time: Option<String>,
    /// Event length in minutes — an alternative to `endTime` for "a 2 hour meeting".
    #[serde(default, rename = "durationMinutes")]
    pub duration_minutes: Option<i64>,
    /// Number of days the event spans (a trip / multi-day event). Set deterministically in
    /// Rust from the text ("for two weeks" → 14); when present the event is all-day.
    #[serde(default)]
    pub span_days: Option<i64>,
}

/// Change an existing event (matched by a fuzzy title/description).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateEvent {
    #[serde(rename = "match")]
    pub target: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub day: Option<String>,
    #[serde(default, rename = "startTime")]
    pub start_time: Option<String>,
    #[serde(default, rename = "endTime")]
    pub end_time: Option<String>,
    /// New length in minutes, keeping the start — for "make it 2 hours instead of 1".
    #[serde(default, rename = "durationMinutes")]
    pub duration_minutes: Option<i64>,
    /// Explicit "YYYY-MM-DD" and/or a multi-day span — both resolved in Rust from the text.
    #[serde(default)]
    pub date: Option<String>,
    #[serde(default)]
    pub span_days: Option<i64>,
    /// A relative time shift in minutes ("push back an hour" → +60, "move up 30 min" → −30),
    /// resolved in Rust and applied to the existing event's current start. Never from the model.
    #[serde(default)]
    pub shift_minutes: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedProject {
    pub name: String,
    #[serde(default)]
    pub tasks: Vec<ParsedTask>,
}

/// A recurring routine the user does regularly ("practice violin every day"). Routed to the
/// habit tracker instead of being a one-off event or task. The model only supplies name/duration;
/// `cadence`/`days`/`interval_days` are filled in deterministically by `route_recurring_to_habits`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ParsedHabit {
    pub name: String,
    #[serde(default = "default_minutes", rename = "durationMinutes")]
    pub duration_minutes: i64,
    #[serde(default = "default_cadence")]
    pub cadence: String,
    #[serde(default)]
    pub days: Vec<u8>,
    #[serde(default = "default_interval_days")]
    pub interval_days: i64,
}

fn default_cadence() -> String {
    "daily".into()
}
fn default_interval_days() -> i64 {
    1
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ParsedPlan {
    #[serde(default)]
    pub events: Vec<ParsedEvent>,
    #[serde(default, rename = "updateEvents")]
    pub update_events: Vec<UpdateEvent>,
    #[serde(default, rename = "removeEvents")]
    pub remove_events: Vec<String>,
    #[serde(default)]
    pub projects: Vec<ParsedProject>,
    #[serde(default)]
    pub habits: Vec<ParsedHabit>,
    #[serde(default)]
    pub clarifications: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanOutcome {
    pub created_task_ids: Vec<i64>,
    pub project_names: Vec<String>,
    pub created_event_titles: Vec<String>,
    pub updated_event_titles: Vec<String>,
    pub removed_event_titles: Vec<String>,
    pub created_habit_names: Vec<String>,
    pub clarifications: Vec<String>,
    /// Vault notes auto-recalled to inform this plan (surfaced in chat for transparency). Filled by
    /// `plan_tasks` after `store_plan`, so it defaults empty everywhere else.
    #[serde(default)]
    pub recalled_notes: Vec<String>,
}

fn priority_to_int(p: &str) -> i64 {
    match p.to_lowercase().as_str() {
        "low" => 1,
        "high" => 3,
        "urgent" | "critical" => 4,
        _ => 2,
    }
}

fn response_schema() -> Value {
    // maxLength / maxItems become grammar bounds in llama.cpp, preventing runaway fields.
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "events": {
                "type": "array",
                "maxItems": 15,
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "title": { "type": "string", "minLength": 1, "maxLength": 100 },
                        "day": { "type": ["string", "null"], "enum": ["today", "tomorrow", "monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday", null] },
                        "date": { "type": ["string", "null"] },
                        "startTime": { "type": ["string", "null"] },
                        "endTime": { "type": ["string", "null"] },
                        "durationMinutes": { "type": ["integer", "null"] }
                    },
                    "required": ["title"]
                }
            },
            "updateEvents": {
                "type": "array",
                "maxItems": 15,
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "match": { "type": "string", "minLength": 1, "maxLength": 100 },
                        "title": { "type": ["string", "null"], "maxLength": 100 },
                        "day": { "type": ["string", "null"], "enum": ["today", "tomorrow", "monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday", null] },
                        "startTime": { "type": ["string", "null"] },
                        "endTime": { "type": ["string", "null"] },
                        "durationMinutes": { "type": ["integer", "null"] }
                    },
                    "required": ["match"]
                }
            },
            "removeEvents": {
                "type": "array",
                "maxItems": 15,
                "items": { "type": "string", "minLength": 1, "maxLength": 100 }
            },
            "projects": {
                "type": "array",
                "maxItems": 6,
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "name": { "type": "string", "minLength": 1, "maxLength": 80 },
                        "tasks": {
                            "type": "array",
                            "maxItems": 25,
                            "items": {
                                "type": "object",
                                "additionalProperties": false,
                                "properties": {
                                    "title": { "type": "string", "minLength": 1, "maxLength": 100 },
                                    "estimated_minutes": { "type": "integer" },
                                    "deadline": { "type": ["string", "null"] },
                                    "priority": { "type": "string", "enum": ["low", "medium", "high", "urgent"] },
                                    "depends_on": { "type": "array", "maxItems": 12, "items": { "type": "string", "minLength": 1, "maxLength": 100 } },
                                    "chunkable": { "type": "boolean" }
                                },
                                "required": ["title", "estimated_minutes", "priority"]
                            }
                        }
                    },
                    "required": ["name", "tasks"]
                }
            },
            "habits": {
                "type": "array",
                "maxItems": 10,
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "name": { "type": "string", "minLength": 1, "maxLength": 80 },
                        "durationMinutes": { "type": ["integer", "null"] }
                    },
                    "required": ["name"]
                }
            },
            "clarifications": { "type": "array", "maxItems": 5, "items": { "type": "string", "maxLength": 200 } }
        },
        "required": ["events", "projects", "clarifications"]
    })
}

/// A relative day word for an event date, so the model can map "today"/"tomorrow"/a
/// weekday straight onto what it sees on the calendar.
fn day_phrase(today: NaiveDate, d: NaiveDate) -> String {
    if d == today {
        "today".into()
    } else if d == today + Duration::days(1) {
        "tomorrow".into()
    } else {
        d.format("%A").to_string().to_lowercase()
    }
}

/// Compact human duration ("1h", "90m"→"1h30", "45m") for the calendar listing.
fn fmt_dur(mins: i64) -> String {
    let (h, m) = (mins / 60, mins % 60);
    match (h, m) {
        (0, m) => format!("{m}m"),
        (h, 0) => format!("{h}h"),
        (h, m) => format!("{h}h{m}m"),
    }
}

/// Short, human-readable summary of the user's sleep + recurring commitments, so the model
/// knows what time is already spoken for. The deterministic scheduler enforces these; this just
/// gives the model context (e.g. so it won't propose a 3am meeting or re-ask about lunch).
fn routine_summary(s: &Settings) -> String {
    const ABBR: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    let mut lines: Vec<String> = Vec::new();
    if s.sleep_enabled && !s.sleep_start.is_empty() && !s.sleep_end.is_empty() {
        lines.push(format!("- Sleep {}-{}", s.sleep_start, s.sleep_end));
    }
    for c in &s.commitments {
        if c.start.is_empty() || c.end.is_empty() {
            continue;
        }
        let when = if c.days.is_empty() {
            "daily".to_string()
        } else {
            c.days
                .iter()
                .filter_map(|d| ABBR.get((*d as usize).wrapping_sub(1)).copied())
                .collect::<Vec<_>>()
                .join("/")
        };
        let name = if c.name.is_empty() { "Blocked" } else { &c.name };
        lines.push(format!("- {} {}-{} ({})", name, c.start, c.end, when));
    }
    if lines.is_empty() {
        String::new()
    } else {
        format!(
            "The user's routine — already reserved, plan around it and never schedule work here:\n{}\n",
            lines.join("\n")
        )
    }
}

/// The calendar the model sees — each event's day, time range, and length — so it can recognize an
/// event and change/remove it without re-asking. Shared by the union prompt and the router/extractors.
fn calendar_listing(events: &[Event]) -> String {
    let now = Local::now().naive_local();
    if events.is_empty() {
        return "(the calendar is currently empty)".to_string();
    }
    events
        .iter()
        .take(30)
        .map(|e| match (parse_dt(&e.start), parse_dt(&e.end)) {
            (Some(s), Some(en)) => format!(
                "- {} — {} {}-{} ({})",
                e.title,
                day_phrase(now.date(), s.date()),
                s.format("%H:%M"),
                en.format("%H:%M"),
                fmt_dur((en - s).num_minutes().max(0)),
            ),
            (Some(s), None) => format!("- {} — {} {}", e.title, day_phrase(now.date(), s.date()), s.format("%H:%M")),
            _ => format!("- {} ({})", e.title, e.start),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The fixed example block used by the union prompt when dynamic exemplars are unavailable (embed
/// server down or semantic off). Covers all five output shapes. `union_extract` swaps in
/// message-relevant exemplars (`select_union_exemplars`) when embeddings are up.
const STATIC_EXAMPLES: &str = "Examples:\n\
user: lunch with mom friday 12-2 → {\"events\":[{\"title\":\"Lunch with mom\",\"day\":\"friday\",\"startTime\":\"12:00\",\"endTime\":\"14:00\"}]}\n\
user: remove all sleepovers → {\"removeEvents\":[\"sleepover\"]}\n\
user: make the sleepover 8pm to 8am → {\"updateEvents\":[{\"match\":\"sleepover\",\"startTime\":\"20:00\",\"endTime\":\"08:00\"}]}\n\
user: make the meeting today 2 hours instead of 1 → {\"updateEvents\":[{\"match\":\"Meeting\",\"durationMinutes\":120}]}\n\
user: practice violin every day from 4pm to 5pm → {\"habits\":[{\"name\":\"Violin practice\",\"durationMinutes\":60}]}\n\
user: exercise daily → {\"habits\":[{\"name\":\"Exercise\",\"durationMinutes\":30}]}\n\
user: plan a blog - pick platform, write posts → {\"projects\":[{\"name\":\"Blog\",\"tasks\":[{\"title\":\"Pick platform\",\"estimated_minutes\":60,\"priority\":\"high\"}]}]}";

fn system_prompt(events: &[Event], settings: &Settings, memory: &[String], examples: &str) -> String {
    let now = Local::now().naive_local();
    let calendar = calendar_listing(events);
    // Auto-recalled vault notes the user has saved — kept short (small models are prompt-sensitive)
    // so the planner can honor stated preferences without bloating the prompt.
    let memory_block = if memory.is_empty() {
        String::new()
    } else {
        let lines: String = memory.iter().map(|m| format!("- {}\n", m.replace('\n', " "))).collect();
        format!("Notes the user has saved (use only if relevant; never invent events from these):\n{lines}")
    };

    format!(
        "Convert the user's message (use the whole conversation for context) into JSON with \
`events`, `updateEvents`, `removeEvents`, `projects`, and `clarifications`.\n\
Choose the right action:\n\
- CREATE a NEW event → add to `events`. EVENTS are things at a set time (lunch, dinner, meeting, \
appointment, call, party). Fields: `title`, `day`, `startTime`, `endTime` (24-hour \"HH:MM\"). \
If the user gives a time RANGE, you MUST fill both `startTime` and `endTime`.\n\
- CHANGE an existing event (new time/day/title) → add to `updateEvents` with `match` = the existing \
event's title, plus only the fields that change. Do NOT also create it. To change only its LENGTH \
(\"make it 2 hours\", \"shorten to 30 min\") set `durationMinutes` and leave the start alone.\n\
- DELETE/remove an existing event → add its title (or a word from it) to `removeEvents`. \
\"remove all sleepovers\" → removeEvents: [\"sleepover\"]. Do NOT create anything.\n\
- TASKS (work to do: write, design, build, study, plan) → `projects[].tasks` with `estimated_minutes`, \
`priority`, `depends_on`. NEVER put work as an event.\n\
- RECURRING routines done regularly (\"every day\", \"daily\", \"each morning\", \"every night\") → \
`habits` with `name` and `durationMinutes`. NOT an event, NOT a task. Don't ask which weekdays.\n\
Rules:\n\
- `day` is the EXACT word the user used (\"today\", \"tomorrow\", or a weekday). NEVER output a computed \
date. One day can cover several events (\"X tomorrow and Y as well\") — set it on EACH. Never ask whether \
a day word means what it says; it does.\n\
- Now is {now} ({weekday}). \"12 - 2\" → startTime 12:00, endTime 14:00; assume PM for ambiguous hours \
unless clearly morning. Overnight ranges are fine (\"8pm to 8am\").\n\
- If the user is editing/removing, use updateEvents/removeEvents — do NOT add a duplicate event.\n\
- The calendar below shows each event's day, time, and length — READ it. Never ask for a time or day \
that is already shown there; just use it.\n\
- `clarifications` only for genuinely missing info, each a question ending with \"?\". Never restate. \
NEVER ask for an end time or duration — if the user gave a range/length use it, else the app defaults it. \
If no day is given, assume today; don't ask.\n\
- Never output the same item twice.\n\
- Only output items from THIS message. The examples below are formatting samples — never copy \
their titles (e.g. \"Blog\", \"Pick platform\") unless the user actually mentions them.\n\
{routine}\
{memory_block}\
Events already on the calendar (reference these to change or remove them):\n\
{calendar}\n\
{examples}",
        now = now.format("%Y-%m-%d %H:%M"),
        weekday = now.format("%A"),
        routine = routine_summary(settings),
        memory_block = memory_block,
        calendar = calendar,
        examples = examples,
    )
}

/// NL → structured plan. **Default: a single UNION extraction** — one `chat_json` call against the
/// full schema (the system prompt shows the calendar + few-shot examples). On the eval battery this
/// matches the older Tier-2 router (classify-then-narrow-extract) at ~84% of checks — equal to the
/// 14B — while making ONE model call instead of a classification + per-intent extractor calls. The
/// **router pipeline (`route_intents` + `extract_by_intents`) is kept as a fallback** for the rare
/// case the union call fails outright. The deterministic recovery layer (`apply_recovery`) and
/// `store_plan` operate on the assembled `ParsedPlan` either way. Holds no DB lock.
pub async fn plan(
    client: &reqwest::Client,
    settings: &Settings,
    current_events: &[Event],
    history: &[ChatTurn],
    user_text: &str,
    memory: &[String],
) -> Result<ParsedPlan> {
    // **Single-call union extraction is the default.** On the eval battery it matches the Tier-2
    // router's accuracy (~84% of checks — equal to the 14B) while making ONE model call instead of
    // a router classification + per-intent extractor calls: faster, simpler, and the path the
    // deterministic recovery layer was tuned against. The router pipeline is retained as a fallback
    // for the rare case the union call fails outright (network/HTTP error or unparseable JSON), so a
    // clear request never silently does nothing.
    let mut parsed = match union_extract(client, settings, current_events, history, user_text, memory).await {
        Ok(p) => p,
        Err(_) => match route_intents(client, settings, current_events, history, user_text).await {
            Ok(intents) if !intents.is_empty() => {
                extract_by_intents(client, settings, current_events, history, user_text, &intents).await.unwrap_or_default()
            }
            _ => ParsedPlan::default(),
        },
    };

    apply_recovery(&mut parsed, user_text, Local::now().naive_local().date());
    Ok(parsed)
}

/// Pull **durable** personal facts/preferences worth remembering long-term out of a chat message
/// (e.g. "Sarah prefers afternoon meetings", "I don't work Fridays") — NOT one-off scheduling. One
/// small `chat_json` call with a tight, capped schema; returns at most a few short facts (possibly
/// none). The caller confirms before saving, so a stray result is harmless. Holds no DB lock.
pub async fn extract_memories(client: &reqwest::Client, settings: &Settings, user_text: &str) -> Result<Vec<String>> {
    let schema = json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "facts": {
                "type": "array",
                "maxItems": 3,
                "items": { "type": "string", "minLength": 4, "maxLength": 160 }
            }
        },
        "required": ["facts"]
    });
    let system = "Extract DURABLE personal facts or preferences from the user's message that are worth \
remembering long-term — people's preferences, recurring constraints, stable facts about the user or \
their world (e.g. \"Sarah prefers afternoon meetings\", \"I don't work Fridays\", \"my manager is Alex\"). \
Do NOT include one-off tasks, events, dates, or scheduling for this week. If there is nothing durable, \
return an empty list. Each fact is a short standalone sentence in third person.";
    let messages = build_messages(system.to_string(), &[], user_text);
    let raw = llm::chat_json(client, &settings.llm_base_url, &settings.model_id, messages, schema).await?;
    let facts = raw["facts"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.trim().to_string())).filter(|s| !s.is_empty()).collect())
        .unwrap_or_default();
    Ok(facts)
}

/// **Eval only** (`PUSHIN_EVAL_ROUTER=1`). Force the router pipeline (classify + per-intent extract
/// with the dynamic exemplar bank) that `plan()` now keeps only as an error fallback, so the harness
/// can A/B router vs union on the same battery. Same recovery layer + `store_plan` as `plan()`.
pub async fn route_eval(
    client: &reqwest::Client,
    settings: &Settings,
    current_events: &[Event],
    history: &[ChatTurn],
    user_text: &str,
) -> Result<ParsedPlan> {
    let mut parsed = match route_intents(client, settings, current_events, history, user_text).await {
        Ok(intents) if !intents.is_empty() => {
            extract_by_intents(client, settings, current_events, history, user_text, &intents).await.unwrap_or_default()
        }
        _ => ParsedPlan::default(),
    };
    apply_recovery(&mut parsed, user_text, Local::now().naive_local().date());
    Ok(parsed)
}

/// The deterministic recovery layer that runs after extraction — HTML-unescape, deadline
/// validation, task/event field recovery (days, durations, spans, deadlines), and habit routing.
/// Shared by `plan` (the router pipeline) and the datagen `union_label` path so a training label is
/// recovered exactly the way inference will recover the student's output.
pub fn apply_recovery(plan: &mut ParsedPlan, user_text: &str, today: NaiveDate) {
    unescape_plan(plan);
    resolve_task_deadlines(plan);
    backfill_task_fields(plan, user_text, today);
    backfill_task_dependencies(plan, user_text);
    backfill_event_fields(plan, user_text, today);
    route_recurring_to_habits(plan, user_text);
    apply_restraint_guard(plan, user_text, today);
}

/// **Restraint guard.** Small on-device models — especially once shown create-shaped few-shot
/// examples — sometimes fabricate an event/task from a message that asks for nothing: a greeting
/// ("hey, how's it going?") or a past-tense report ("I already finished the laundry earlier"). When
/// the plan holds ONLY fabricated creates (no edit/remove/habit — those are high-confidence actions),
/// the message carries NO scheduling cue (time/date/recurrence/scheduling verb), AND it reads as a
/// greeting or a completed-action report, drop the fabrication and ask what to schedule instead.
/// Conservative by construction: all three gates must hold, so a real request that merely opens with
/// "hi" ("hi, add gym tomorrow at 6am") is never suppressed (its time/verb cue trips `has_action_cue`).
fn apply_restraint_guard(plan: &mut ParsedPlan, text: &str, today: NaiveDate) {
    let fabricated = !plan.events.is_empty() || plan.projects.iter().any(|p| !p.tasks.is_empty());
    let strong_action = !plan.update_events.is_empty() || !plan.remove_events.is_empty() || !plan.habits.is_empty();
    if !fabricated || strong_action || has_action_cue(text, today) {
        return;
    }
    if is_greeting(text) || is_past_completion(text) || is_vague_plans(text) {
        plan.events.clear();
        plan.projects.clear();
        if plan.clarifications.is_empty() {
            plan.clarifications.push("What would you like to schedule?".into());
        }
    }
}

/// The message gestures at unspecified future activity ("I have some stuff going on next week")
/// without naming any concrete event — the model loves to elaborate this into invented items. Paired
/// with the `has_action_cue` gate in the guard, so a vague phrase next to a real time/verb is spared.
fn is_vague_plans(text: &str) -> bool {
    let t = text.to_lowercase();
    const VAGUE: &[&str] = &[
        "some stuff", "stuff going on", "some things", "a few things", "bunch of stuff", "lot going on",
        "a lot to do", "lots going on", "lots to do", "some plans", "things going on", "a lot going on",
    ];
    VAGUE.iter().any(|p| t.contains(p))
}

/// Future-oriented scheduling signals: a clock time, a resolvable date/span, a recurrence, or an
/// explicit scheduling/edit verb. Deliberately ignores bare activity nouns and past-tense verbs so a
/// completed-action report ("I already finished the laundry") isn't mistaken for a live request.
fn has_action_cue(text: &str, today: NaiveDate) -> bool {
    if mentions_clock_time(text) {
        return true;
    }
    if find_explicit_date(text, today).is_some()
        || find_relative_date(text, today).is_some()
        || find_day_of_month(text, today).is_some()
        || find_span_days(text).is_some()
        || recurrence(text).is_some()
    {
        return true;
    }
    // Word/phrase-bounded match (punctuation → spaces, wrapped in spaces) so "going to" doesn't fire
    // on "going today" and "set a" doesn't fire on "set apart".
    let norm: String = text.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() || c == '\'' { c } else { ' ' }).collect();
    let padded = format!(" {} ", norm.split_whitespace().collect::<Vec<_>>().join(" "));
    const VERBS: &[&str] = &[
        "schedule", "add", "book", "remind", "set up", "setup", "set a", "make a", "put", "plan",
        "create", "move", "reschedule", "cancel", "remove", "delete", "rename", "need to", "needs to",
        "have to", "has to", "want to", "wanna", "going to", "gonna", "will", "i'll", "let's", "block", "pencil",
    ];
    VERBS.iter().any(|v| padded.contains(&format!(" {v} ")))
}

/// True if a clock time is mentioned ("2pm", "9:30", "3 pm", "at 3", "noon"). Narrow on purpose: the
/// "am"/"pm" must sit on a digit-bearing or numeric-preceding token, so words like "team"/"I am" don't match.
fn mentions_clock_time(text: &str) -> bool {
    let l = text.to_lowercase();
    if l.contains("noon") || l.contains("midnight") || l.contains("o'clock") || l.contains("oclock") {
        return true;
    }
    let chars: Vec<char> = l.chars().collect();
    for i in 1..chars.len() {
        if chars[i] == ':' && chars[i - 1].is_ascii_digit() && chars.get(i + 1).map_or(false, |c| c.is_ascii_digit()) {
            return true;
        }
    }
    let toks: Vec<&str> = l.split_whitespace().collect();
    for (i, w) in toks.iter().enumerate() {
        if *w == "at" && toks.get(i + 1).and_then(|n| n.chars().next()).map_or(false, |c| c.is_ascii_digit()) {
            return true;
        }
        let trimmed = w.trim_end_matches(|c: char| c == '.' || c == ',' || c == '!' || c == '?');
        let has_digit = trimmed.chars().any(|c| c.is_ascii_digit());
        if has_digit && (trimmed.ends_with("am") || trimmed.ends_with("pm")) {
            return true;
        }
        if (*w == "am" || *w == "pm") && i > 0 && toks[i - 1].chars().next().map_or(false, |c| c.is_ascii_digit()) {
            return true;
        }
    }
    false
}

/// The message is essentially a greeting / social pleasantry.
fn is_greeting(text: &str) -> bool {
    let t = text.trim().to_lowercase();
    if t.is_empty() {
        return false;
    }
    const PHRASES: &[&str] = &[
        "how's it going", "hows it going", "how are you", "how is it going", "how's your day",
        "what's up", "whats up", "how have you been", "nice to meet you", "good to see you",
    ];
    if PHRASES.iter().any(|p| t.contains(p)) {
        return true;
    }
    let first: String = t.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '\'').collect();
    matches!(first.as_str(), "hi" | "hey" | "hello" | "yo" | "sup" | "howdy" | "hiya" | "heya" | "greetings")
        || t.starts_with("good morning")
        || t.starts_with("good afternoon")
        || t.starts_with("good evening")
}

/// The message reports an action the user ALREADY did (past tense + a completion marker, with no
/// future framing) — not a new thing to schedule.
fn is_past_completion(text: &str) -> bool {
    let t = text.to_lowercase();
    const DONE: &[&str] = &[
        "finished", "completed", "wrapped up", "took care of", "got done", "did the", "have done",
        "i've done", "ive done", "i did", "knocked out", "already done",
    ];
    if !DONE.iter().any(|d| t.contains(d)) {
        return false;
    }
    const PAST: &[&str] = &[
        "already", "earlier", "yesterday", "last night", "this morning", "this afternoon", "just now", "a while ago", "ago",
    ];
    const FUTURE: &[&str] = &[
        "need to", "have to", "want to", "going to", "gonna", "will ", "i'll", "should ", "by ", "before ", "tomorrow", "next ",
    ];
    PAST.iter().any(|p| t.contains(p)) && !FUTURE.iter().any(|f| t.contains(f))
}

/// **Datagen only.** The single-call (union-format) chat messages the fine-tuned student will see at
/// inference — system prompt + (gated) history + user. Pair this with a label produced by the
/// *router* pipeline (`plan`), which routes far better than a one-shot union call: that distills the
/// router's correctness into the one-call format the student uses.
pub fn union_messages(settings: &Settings, current_events: &[Event], history: &[ChatTurn], user_text: &str) -> Value {
    // Datagen uses the static example block (no embed server in the finetune toolchain).
    build_messages(system_prompt(current_events, settings, &[], STATIC_EXAMPLES), history, user_text)
}

/// **Datagen only** (see `finetune/`). Run the SINGLE union extraction (no router) against a teacher
/// model and return `(request messages, the model's RAW schema JSON, the recovered plan)`:
/// - the raw JSON is the fine-tuning **label** (schema-valid by construction — it came through the
///   `response_schema` grammar), and
/// - the recovered `ParsedPlan` is what to feed `store_plan` for validation.
///
/// The fine-tuned student is meant to run this same one-call path at inference, so the label format
/// matches inference exactly. Holds no DB lock.
pub async fn union_label(
    client: &reqwest::Client,
    settings: &Settings,
    current_events: &[Event],
    history: &[ChatTurn],
    user_text: &str,
) -> Result<(Value, Value, ParsedPlan)> {
    let system = system_prompt(current_events, settings, &[], STATIC_EXAMPLES);
    let messages = build_messages(system, history, user_text);
    let raw = llm::chat_json(client, &settings.llm_base_url, &settings.model_id, messages.clone(), response_schema()).await?;
    let mut plan: ParsedPlan = serde_json::from_value(raw.clone()).unwrap_or_default();
    apply_recovery(&mut plan, user_text, Local::now().naive_local().date());
    Ok((messages, raw, plan))
}

// ---------------- Router pass + narrow per-intent extractors (Tier 2) ----------------

/// An actionable thing the user's message asks for. "none"/unknown router outputs map to nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Intent {
    CreateEvent,
    EditEvent,
    RemoveEvent,
    CreateTask,
    CreateHabit,
}

fn intent_from_str(s: &str) -> Option<Intent> {
    Some(match s {
        "createEvent" => Intent::CreateEvent,
        "editEvent" => Intent::EditEvent,
        "removeEvent" => Intent::RemoveEvent,
        "createTask" => Intent::CreateTask,
        "createHabit" => Intent::CreateHabit,
        _ => return None,
    })
}

/// Build the chat messages, feeding prior turns only to genuine follow-ups (see `needs_history`).
fn build_messages(system: String, history: &[ChatTurn], user_text: &str) -> Value {
    let mut messages: Vec<Value> = vec![json!({ "role": "system", "content": system })];
    if needs_history(user_text) {
        for turn in history.iter().rev().take(6).rev() {
            let role = if turn.role == "assistant" { "assistant" } else { "user" };
            messages.push(json!({ "role": role, "content": turn.content }));
        }
    }
    messages.push(json!({ "role": "user", "content": user_text }));
    Value::Array(messages)
}

/// Now + routine (+ optional calendar) context shared by the router and extractor prompts.
fn ctx_block(events: &[Event], settings: &Settings, with_calendar: bool) -> String {
    let now = Local::now().naive_local();
    let mut s = format!("Now is {} ({}).\n{}", now.format("%Y-%m-%d %H:%M"), now.format("%A"), routine_summary(settings));
    if with_calendar {
        s.push_str(&format!("Current calendar (reference for edits/removes):\n{}\n", calendar_listing(events)));
    }
    s
}

/// Stage 1: classify the message into the actions it needs.
async fn route_intents(
    client: &reqwest::Client,
    settings: &Settings,
    events: &[Event],
    history: &[ChatTurn],
    user_text: &str,
) -> Result<Vec<Intent>> {
    let system = format!(
        "Classify what the user wants done with their calendar. Output {{\"intents\":[...]}} using ONLY:\n\
- createEvent: a NEW thing at a set time (lunch, meeting, appointment, call, party, trip).\n\
- editEvent: change an EXISTING calendar item (move, rename, make longer/shorter).\n\
- removeEvent: delete or cancel an EXISTING calendar item.\n\
- createTask: work to do with NO fixed time (study, write, build, plan, finish, read).\n\
- createHabit: a recurring routine — \"every day\"/\"daily\", \"every morning/night\", \"every other \
day\", \"every Monday and Wednesday\", \"weekdays\", \"weekends\". Recurs on a schedule, no single date.\n\
- none: a greeting, or a vague/unactionable message.\n\
Pick ALL that apply — one message can need several (e.g. an event AND a task). Output ONLY the intents array.\n\
{}",
        ctx_block(events, settings, true)
    );
    let raw = llm::chat_json(client, &settings.llm_base_url, &settings.model_id, build_messages(system, history, user_text), router_schema()).await?;
    let mut out: Vec<Intent> = Vec::new();
    if let Some(arr) = raw["intents"].as_array() {
        for v in arr {
            if let Some(i) = v.as_str().and_then(intent_from_str) {
                if !out.contains(&i) {
                    out.push(i);
                }
            }
        }
    }
    Ok(out)
}

/// Run one narrow extractor and deserialize its slice into a `ParsedPlan` (other fields stay empty).
async fn run_extractor(
    client: &reqwest::Client,
    settings: &Settings,
    system: String,
    history: &[ChatTurn],
    user_text: &str,
    schema: Value,
) -> Result<ParsedPlan> {
    let raw = llm::chat_json(client, &settings.llm_base_url, &settings.model_id, build_messages(system, history, user_text), schema).await?;
    Ok(serde_json::from_value(raw).unwrap_or_default())
}

/// Stage 2: run the relevant extractor for each routed intent and merge the slices.
// ---- Dynamic few-shot: retrieve the exemplars most similar to the user's message (Tier 2) ----
// Instead of a fixed example per extractor, we keep a small bank of (intent, utterance, gold-JSON)
// exemplars, embed it once on-device via the bge server, and inject the top-k most similar to the
// actual input. This kills example-parroting and gives the model an example shaped like its task
// (e.g. a dependency exemplar surfaces for "fix bug then test then deploy"). Falls back to the first
// exemplars of the intent when embeddings are unavailable — so it never regresses below static few-shot.

struct Exemplar {
    intent: Intent,
    text: &'static str,
    output: &'static str,
}

const EXEMPLARS: &[Exemplar] = &[
    // createEvent — single thing → ONE event; multiple only when several are listed.
    Exemplar { intent: Intent::CreateEvent, text: "gym tomorrow at 6 in the morning", output: "{\"events\":[{\"title\":\"Gym\",\"day\":\"tomorrow\",\"startTime\":\"06:00\"}]}" },
    Exemplar { intent: Intent::CreateEvent, text: "coffee with alex friday at 3pm", output: "{\"events\":[{\"title\":\"Coffee with Alex\",\"day\":\"friday\",\"startTime\":\"15:00\"}]}" },
    Exemplar { intent: Intent::CreateEvent, text: "team sync at noon for 45 minutes", output: "{\"events\":[{\"title\":\"Team sync\",\"startTime\":\"12:00\",\"durationMinutes\":45}]}" },
    Exemplar { intent: Intent::CreateEvent, text: "lunch with mom friday 12-2 and a graduation party from 6-10", output: "{\"events\":[{\"title\":\"Lunch with mom\",\"day\":\"friday\",\"startTime\":\"12:00\",\"endTime\":\"14:00\"},{\"title\":\"Graduation party\",\"day\":\"friday\",\"startTime\":\"18:00\",\"endTime\":\"22:00\"}]}" },
    // createTask — single activity → ONE task; explicit steps decompose; sequential → depends_on.
    Exemplar { intent: Intent::CreateTask, text: "study for the exam, about 4 hours", output: "{\"projects\":[{\"name\":\"Exam\",\"tasks\":[{\"title\":\"Study for the exam\",\"estimated_minutes\":240,\"priority\":\"high\"}]}]}" },
    Exemplar { intent: Intent::CreateTask, text: "plan a blog - pick platform, write 3 posts", output: "{\"projects\":[{\"name\":\"Blog\",\"tasks\":[{\"title\":\"Pick platform\",\"estimated_minutes\":60,\"priority\":\"medium\"},{\"title\":\"Write 3 posts\",\"estimated_minutes\":180,\"priority\":\"medium\"}]}]}" },
    Exemplar { intent: Intent::CreateTask, text: "to launch the app I need to fix the login bug, then write tests, then deploy", output: "{\"projects\":[{\"name\":\"Launch\",\"tasks\":[{\"title\":\"Fix the login bug\",\"estimated_minutes\":60,\"priority\":\"high\"},{\"title\":\"Write tests\",\"estimated_minutes\":60,\"priority\":\"high\",\"depends_on\":[\"Fix the login bug\"]},{\"title\":\"Deploy\",\"estimated_minutes\":30,\"priority\":\"high\",\"depends_on\":[\"Write tests\"]}]}]}" },
    Exemplar { intent: Intent::CreateTask, text: "write the report, about 90 minutes, and email it, 10 minutes", output: "{\"projects\":[{\"name\":\"Report\",\"tasks\":[{\"title\":\"Write the report\",\"estimated_minutes\":90,\"priority\":\"medium\"},{\"title\":\"Email the report\",\"estimated_minutes\":10,\"priority\":\"medium\"}]}]}" },
    // createHabit
    Exemplar { intent: Intent::CreateHabit, text: "practice violin every day from 4 to 5pm", output: "{\"habits\":[{\"name\":\"Violin practice\",\"durationMinutes\":60}]}" },
    Exemplar { intent: Intent::CreateHabit, text: "meditate for 10 minutes every morning", output: "{\"habits\":[{\"name\":\"Meditate\",\"durationMinutes\":10}]}" },
    Exemplar { intent: Intent::CreateHabit, text: "go to the gym every monday and wednesday", output: "{\"habits\":[{\"name\":\"Gym\",\"durationMinutes\":60}]}" },
    Exemplar { intent: Intent::CreateHabit, text: "study US history every other day for 2 hours", output: "{\"habits\":[{\"name\":\"Study US history\",\"durationMinutes\":120}]}" },
    // editEvent
    Exemplar { intent: Intent::EditEvent, text: "make the meeting 2 hours instead of 1", output: "{\"updateEvents\":[{\"match\":\"Meeting\",\"durationMinutes\":120}]}" },
    Exemplar { intent: Intent::EditEvent, text: "move the dentist to 3pm", output: "{\"updateEvents\":[{\"match\":\"Dentist\",\"startTime\":\"15:00\"}]}" },
    Exemplar { intent: Intent::EditEvent, text: "rename my gym session to morning workout", output: "{\"updateEvents\":[{\"match\":\"Gym session\",\"title\":\"Morning workout\"}]}" },
    // removeEvent
    Exemplar { intent: Intent::RemoveEvent, text: "remove all sleepovers", output: "{\"removeEvents\":[\"sleepover\"]}" },
    Exemplar { intent: Intent::RemoveEvent, text: "cancel lunch with dan", output: "{\"removeEvents\":[\"lunch with Dan\"]}" },
];

/// Embed the exemplar bank once per process (one batch call) and cache it. `None` if the embed
/// server isn't available — callers then fall back to static exemplars.
async fn bank_embeddings(client: &reqwest::Client, base: &str, model: &str) -> Option<&'static [Vec<f32>]> {
    static BANK: OnceLock<Vec<Vec<f32>>> = OnceLock::new();
    if let Some(b) = BANK.get() {
        return Some(b.as_slice());
    }
    let inputs: Vec<&str> = EXEMPLARS.iter().map(|e| e.text).collect();
    let embs = hermes::embed_batch(client, base, model, &inputs).await.ok()?;
    if embs.len() != EXEMPLARS.len() {
        return None;
    }
    let _ = BANK.set(embs);
    BANK.get().map(|b| b.as_slice())
}

/// Top-k exemplars of `intent` by cosine to the query embedding.
fn select_exemplars(query: &[f32], bank: &[Vec<f32>], intent: Intent, k: usize) -> Vec<&'static Exemplar> {
    let mut scored: Vec<(f32, &'static Exemplar)> = EXEMPLARS
        .iter()
        .enumerate()
        .filter(|(_, e)| e.intent == intent)
        .map(|(i, e)| (hermes::cosine(query, &bank[i]), e))
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.into_iter().take(k).map(|(_, e)| e).collect()
}

/// The first `k` exemplars of an intent — the static fallback when embeddings are unavailable.
fn default_exemplars(intent: Intent, k: usize) -> Vec<&'static Exemplar> {
    EXEMPLARS.iter().filter(|e| e.intent == intent).take(k).collect()
}

/// Embed the user message + the exemplar bank (best-effort, on-device). Returns `(query, bank)`;
/// either being `None` means callers fall back to static exemplars. Shared by the union path and the
/// router's per-intent extractors so both pick exemplars the same way. Gated on `embed_model`
/// (empty = semantic off).
async fn query_and_bank(
    client: &reqwest::Client,
    settings: &Settings,
    user_text: &str,
) -> (Option<Vec<f32>>, Option<&'static [Vec<f32>]>) {
    if settings.embed_model.trim().is_empty() {
        return (None, None);
    }
    let base = model_manager::embed_base_url();
    let bank = bank_embeddings(client, &base, &settings.embed_model).await;
    let q = hermes::embed_text(client, &base, &settings.embed_model, user_text).await.ok();
    match (q, bank) {
        (Some(q), Some(_)) => (Some(q), bank),
        _ => (None, None),
    }
}

/// Exemplars for the UNION prompt, which has no routed intents. Guarantees one example of EACH output
/// shape (so habit/task forms never disappear for a message that reads like a different intent —
/// directly guarding the multi-intent habit-drop case), choosing the most query-similar exemplar of
/// each, then reinforces the dominant intent with the next-most-similar overall, capped at 7 (≈ the
/// static block's size, so the prompt doesn't grow — longer prompts degrade the small model).
fn select_union_exemplars(query: &[f32], bank: &[Vec<f32>]) -> Vec<&'static Exemplar> {
    let mut scored: Vec<(f32, usize)> = (0..EXEMPLARS.len()).map(|i| (hermes::cosine(query, &bank[i]), i)).collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let order: Vec<usize> = scored.iter().map(|(_, i)| *i).collect();

    let mut picks: Vec<usize> = Vec::new();
    for intent in [Intent::CreateEvent, Intent::CreateTask, Intent::CreateHabit, Intent::EditEvent, Intent::RemoveEvent] {
        if let Some(&i) = order.iter().find(|&&i| EXEMPLARS[i].intent == intent) {
            picks.push(i);
        }
    }
    for &i in &order {
        if picks.len() >= 7 {
            break;
        }
        if !picks.contains(&i) {
            picks.push(i);
        }
    }
    picks.into_iter().map(|i| &EXEMPLARS[i]).collect()
}

/// The union prompt's example block — same `Examples:\n user: … → …` shape as `STATIC_EXAMPLES`. Leads
/// with a restraint demonstration: the dynamic exemplars always show one "create" of each intent, which
/// otherwise primes the model to fabricate an event from a greeting/no-op message (observed regression);
/// showing greeting → nothing-but-a-question counteracts that.
fn union_examples_block(ex: &[&Exemplar]) -> String {
    let mut s = String::from(
        "Examples:\n\
user: hey, how's it going? → {\"clarifications\":[\"What would you like to schedule?\"]}\n",
    );
    for e in ex {
        s.push_str(&format!("user: {} → {}\n", e.text, e.output));
    }
    s
}

fn exemplar_block(ex: &[&Exemplar]) -> String {
    let mut s = String::from("Examples →\n");
    for e in ex {
        s.push_str(&format!("user: {} → {}\n", e.text, e.output));
    }
    s
}

async fn extract_by_intents(
    client: &reqwest::Client,
    settings: &Settings,
    events: &[Event],
    history: &[ChatTurn],
    user_text: &str,
    intents: &[Intent],
) -> Result<ParsedPlan> {
    let now = Local::now().naive_local();

    // Embed the query + bank once (on-device, best-effort). If unavailable, `pick` falls back to
    // static exemplars, so this never regresses below the fixed-example behavior.
    let (qvec, bank) = query_and_bank(client, settings, user_text).await;
    let pick = |intent: Intent| -> String {
        let ex = match (&qvec, bank) {
            (Some(q), Some(b)) => select_exemplars(q, b, intent, 2),
            _ => default_exemplars(intent, 2),
        };
        exemplar_block(&ex)
    };

    let mut plan = ParsedPlan::default();
    for intent in intents {
        match intent {
            Intent::CreateEvent => {
                let system = format!(
                    "Extract the NEW event(s) the user is scheduling into {{\"events\":[...]}}. Each event: \
`title`, `day` (the EXACT word \"today\"/\"tomorrow\"/a weekday — NEVER a computed date), `startTime`, \
`endTime` (24-hour \"HH:MM\"). If a time RANGE is given you MUST fill BOTH. \"12-2\" → 12:00/14:00; assume \
PM for ambiguous hours unless clearly morning; overnight is fine (\"8pm to 8am\"). One day can cover several \
events. Output exactly the events stated in THIS message — never invent extra ones.\n\
Now is {now} ({wd}).\n\
{examples}",
                    now = now.format("%Y-%m-%d %H:%M"), wd = now.format("%A"), examples = pick(Intent::CreateEvent),
                );
                let p = run_extractor(client, settings, system, history, user_text, events_schema()).await?;
                plan.events.extend(p.events);
            }
            Intent::CreateTask => {
                let system = format!(
                    "Extract the work the user needs to do into {{\"projects\":[{{\"name\":...,\"tasks\":[...]}}]}}. \
Each task: `title`, `estimated_minutes`, `priority` (low/medium/high/urgent), optional `depends_on` (titles \
of tasks that must finish first — use it for sequenced work like \"X then Y then Z\"). Tasks are work with NO \
fixed time. Group related tasks under one short project name.\n\
- A SINGLE activity is exactly ONE task — even with a duration or time (\"study for 2 hours\"). NEVER break \
it into invented sub-steps.\n\
- Output MULTIPLE tasks ONLY when the user explicitly lists several distinct steps.\n\
Output only tasks from THIS message — never invent extra tasks.\n\
Now is {now}.\n\
{examples}",
                    now = now.format("%Y-%m-%d %H:%M"), examples = pick(Intent::CreateTask),
                );
                let p = run_extractor(client, settings, system, history, user_text, projects_schema()).await?;
                plan.projects.extend(p.projects);
            }
            Intent::CreateHabit => {
                let system = format!(
                    "Extract the recurring routine(s) into {{\"habits\":[{{\"name\":...,\"durationMinutes\":...}}]}}. \
A habit is anything done on a repeating schedule — daily, every morning/night, every other day, certain \
weekdays (e.g. Mon & Wed), weekdays, or weekends. Give it a short `name` (e.g. \"Gym\", \"Study US \
history\") and a `durationMinutes` if a length is stated. Always output a habit when the message says it \
recurs; never leave it empty or ask which days. Output only routines from THIS message.\n\
{examples}",
                    examples = pick(Intent::CreateHabit),
                );
                let p = run_extractor(client, settings, system, history, user_text, habits_schema()).await?;
                plan.habits.extend(p.habits);
            }
            Intent::EditEvent => {
                let system = format!(
                    "The user wants to CHANGE an existing calendar item. Output {{\"updateEvents\":[{{\"match\":<the \
existing title or a word from it>, ...only the fields that change...}}]}}. Settable: `title`, `day`, \
`startTime`, `endTime`, `durationMinutes`. To change only the LENGTH set `durationMinutes` and leave the \
start. Times are 24-hour \"HH:MM\"; `day` is a day word, never a date. Match against the calendar below.\n\
{ctx}\n\
{examples}",
                    ctx = ctx_block(events, settings, true), examples = pick(Intent::EditEvent),
                );
                let p = run_extractor(client, settings, system, history, user_text, update_schema()).await?;
                plan.update_events.extend(p.update_events);
            }
            Intent::RemoveEvent => {
                let system = format!(
                    "The user wants to DELETE/cancel an existing calendar item. Output {{\"removeEvents\":[<title or a \
word from it>]}}. Match against the calendar below; if several items share a word, prefer the most specific \
phrase so siblings are spared.\n\
{ctx}\n\
{examples}",
                    ctx = ctx_block(events, settings, true), examples = pick(Intent::RemoveEvent),
                );
                let p = run_extractor(client, settings, system, history, user_text, remove_schema()).await?;
                plan.remove_events.extend(p.remove_events);
            }
        }
    }
    Ok(plan)
}

/// Fallback: the original single union call. Used only when the router call fails outright, so a
/// clear request never silently does nothing.
async fn union_extract(
    client: &reqwest::Client,
    settings: &Settings,
    events: &[Event],
    history: &[ChatTurn],
    user_text: &str,
    memory: &[String],
) -> Result<ParsedPlan> {
    // Dynamic few-shot: swap the static example block for the exemplars most relevant to THIS
    // message (best-effort; falls back to the static block when embeddings are unavailable).
    let (qvec, bank) = query_and_bank(client, settings, user_text).await;
    let examples = match (qvec.as_deref(), bank) {
        (Some(q), Some(b)) => union_examples_block(&select_union_exemplars(q, b)),
        _ => STATIC_EXAMPLES.to_string(),
    };
    let messages = build_messages(system_prompt(events, settings, memory, &examples), history, user_text);
    let raw = llm::chat_json(client, &settings.llm_base_url, &settings.model_id, messages, response_schema()).await?;
    Ok(serde_json::from_value(raw)?)
}

fn router_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "intents": { "type": "array", "maxItems": 6, "items": { "type": "string", "enum": ["createEvent", "editEvent", "removeEvent", "createTask", "createHabit", "none"] } }
        },
        "required": ["intents"]
    })
}

/// The `day` enum the model may emit (day words + JSON null), shared by the event/edit schemas.
fn day_enum() -> Value {
    json!(["today", "tomorrow", "monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday", null])
}

fn events_schema() -> Value {
    json!({
        "type": "object", "additionalProperties": false,
        "properties": { "events": { "type": "array", "maxItems": 15, "items": {
            "type": "object", "additionalProperties": false,
            "properties": {
                "title": { "type": "string", "minLength": 1, "maxLength": 100 },
                "day": { "type": ["string", "null"], "enum": day_enum() },
                "date": { "type": ["string", "null"] },
                "startTime": { "type": ["string", "null"] },
                "endTime": { "type": ["string", "null"] },
                "durationMinutes": { "type": ["integer", "null"] }
            }, "required": ["title"] } } },
        "required": ["events"]
    })
}

fn update_schema() -> Value {
    json!({
        "type": "object", "additionalProperties": false,
        "properties": { "updateEvents": { "type": "array", "maxItems": 15, "items": {
            "type": "object", "additionalProperties": false,
            "properties": {
                "match": { "type": "string", "minLength": 1, "maxLength": 100 },
                "title": { "type": ["string", "null"], "maxLength": 100 },
                "day": { "type": ["string", "null"], "enum": day_enum() },
                "startTime": { "type": ["string", "null"] },
                "endTime": { "type": ["string", "null"] },
                "durationMinutes": { "type": ["integer", "null"] }
            }, "required": ["match"] } } },
        "required": ["updateEvents"]
    })
}

fn remove_schema() -> Value {
    json!({
        "type": "object", "additionalProperties": false,
        "properties": { "removeEvents": { "type": "array", "maxItems": 15, "items": { "type": "string", "minLength": 1, "maxLength": 100 } } },
        "required": ["removeEvents"]
    })
}

fn projects_schema() -> Value {
    json!({
        "type": "object", "additionalProperties": false,
        "properties": { "projects": { "type": "array", "maxItems": 6, "items": {
            "type": "object", "additionalProperties": false,
            "properties": {
                "name": { "type": "string", "minLength": 1, "maxLength": 80 },
                "tasks": { "type": "array", "maxItems": 25, "items": {
                    "type": "object", "additionalProperties": false,
                    "properties": {
                        "title": { "type": "string", "minLength": 1, "maxLength": 100 },
                        "estimated_minutes": { "type": "integer" },
                        "deadline": { "type": ["string", "null"] },
                        "priority": { "type": "string", "enum": ["low", "medium", "high", "urgent"] },
                        "depends_on": { "type": "array", "maxItems": 12, "items": { "type": "string", "minLength": 1, "maxLength": 100 } },
                        "chunkable": { "type": "boolean" }
                    }, "required": ["title", "estimated_minutes", "priority"] } }
            }, "required": ["name", "tasks"] } } },
        "required": ["projects"]
    })
}

fn habits_schema() -> Value {
    json!({
        "type": "object", "additionalProperties": false,
        "properties": { "habits": { "type": "array", "maxItems": 10, "items": {
            "type": "object", "additionalProperties": false,
            "properties": { "name": { "type": "string", "minLength": 1, "maxLength": 80 }, "durationMinutes": { "type": ["integer", "null"] } },
            "required": ["name"] } } },
        "required": ["habits"]
    })
}

/// Does this message lean on the prior conversation (a follow-up/edit), so the planner needs
/// history? We feed history *only* to genuine follow-ups — passing it to a fresh, self-contained
/// request is what lets a stale entity bleed in (the "surgery" → "Study" contamination). Biased
/// toward keeping history when unsure: a missed follow-up (lost context, hallucinated subject)
/// hurts more than a redundant one. A standalone request ("on 6/12 I have a surgery at 10am")
/// hits none of the cues and goes in cold; "move it to 9pm" / "this friday at 7pm" keep context.
fn needs_history(text: &str) -> bool {
    let lc = text.to_lowercase();
    let words: HashSet<&str> = lc.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    // Pronouns / demonstratives that point back at an earlier turn.
    const REF_WORDS: &[&str] =
        &["it", "its", "that", "this", "those", "them", "they", "same", "instead", "again", "earlier", "one", "ones"];
    // Verbs implying an item that already exists (an edit, not a creation).
    const EDIT_WORDS: &[&str] = &[
        "move", "reschedule", "change", "rename", "push", "cancel", "cancelled", "canceled", "delete", "remove",
        "update", "postpone",
    ];
    if REF_WORDS.iter().chain(EDIT_WORDS.iter()).any(|w| words.contains(w)) {
        return true;
    }
    // Continuation openers ("and also at 6pm…", "no, make it Tuesday").
    const OPENERS: &[&str] = &["and", "also", "but", "then", "actually", "no", "wait", "plus", "instead"];
    lc.split(|c: char| !c.is_alphanumeric())
        .find(|w| !w.is_empty())
        .map(|first| OPENERS.contains(&first))
        .unwrap_or(false)
}

/// Daily-recurrence language. Recurring routines become habits, not one-off events/tasks.
fn daily_recurrence(text: &str) -> bool {
    let t = text.to_lowercase();
    ["every day", "everyday", "each day", "daily", "every morning", "every night", "every evening", "each morning", "each night"]
        .iter()
        .any(|p| t.contains(p))
}

/// Minutes spanned by a time range in the text ("4pm to 5pm" → 60), for sizing a habit.
fn range_minutes(text: &str) -> Option<i64> {
    let (start, end_raw) = find_time_range(text)?;
    let base = NaiveDate::from_ymd_opt(2000, 1, 1).unwrap().and_time(start);
    let mins = (compute_end(base, Some(&end_raw)) - base).num_minutes();
    (mins > 0).then_some(mins)
}

/// Do two titles plausibly name the same thing, ignoring word order? ("Practice violin" vs the
/// "Violin practice" habit). Token-overlap, since `event_matches` is substring-only and misses
/// reordered words. Used to suppress the one-off the model double-emits next to a habit.
fn titles_refer_same(a: &str, b: &str) -> bool {
    const FILLER: &[&str] = &["the", "and", "for", "with", "session", "time"];
    let toks = |s: &str| -> HashSet<String> {
        s.to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.len() >= 3 && !FILLER.contains(w))
            .map(str::to_string)
            .collect()
    };
    let (ta, tb) = (toks(a), toks(b));
    if ta.is_empty() || tb.is_empty() {
        return a.trim().eq_ignore_ascii_case(b.trim());
    }
    ta.intersection(&tb).next().is_some()
}

/// Pull a habit name out of free text when the model emitted nothing to convert — strip the
/// intent lead-in, recurrence words, and any duration/time tokens, and keep the activity.
/// "Exercise daily for 30 minutes" → "Exercise"; "...practice violin every day from 4-5pm" →
/// "Practice violin". Returns `None` if nothing sensible is left (so we don't invent garbage).
fn synthesize_habit_name(text: &str) -> Option<String> {
    let mut s = format!(" {} ", text.to_lowercase());
    for p in [
        " i want to ", " i'd like to ", " i would like to ", " i need to ", " i'm going to ",
        " i am going to ", " i will ", " i wanna ", " let me ", " remind me to ", " i should ", " please ",
    ] {
        s = s.replace(p, " ");
    }
    for p in [
        " every day ", " everyday ", " each day ", " daily ", " every morning ", " every evening ",
        " every night ", " each morning ", " each evening ", " each night ", " every week ", " weekly ",
    ] {
        s = s.replace(p, " ");
    }
    // Drop connectors, duration/time units, recurrence/weekday words, and any numeric/clock token.
    const DROP: &[&str] = &[
        "for", "at", "from", "to", "minutes", "minute", "mins", "min", "hours", "hour", "hrs", "hr",
        "h", "m", "am", "pm", "a", "an", "the", "my", "of", "on", "in", "go", "and", "every", "other",
        "each", "day", "days", "week", "weeks", "weekday", "weekdays", "weekend", "weekends", "second", "alternate",
        "monday", "mon", "mondays", "tuesday", "tue", "tues", "tuesdays", "wednesday", "wed", "weds", "wednesdays",
        "thursday", "thu", "thur", "thurs", "thursdays", "friday", "fri", "fridays", "saturday", "sat", "saturdays",
        "sunday", "sun", "sundays",
    ];
    let words: Vec<String> = s
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_string())
        .filter(|w| {
            !w.is_empty()
                && !w.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false)
                && !DROP.contains(&w.as_str())
        })
        .collect();
    let name = words.join(" ");
    let name = name.trim();
    if name.is_empty() || name.split_whitespace().count() > 5 {
        return None;
    }
    let mut chars = name.chars();
    chars.next().map(|f| f.to_uppercase().collect::<String>() + chars.as_str())
}

/// Route recurring routines to the habit tracker. The model is told to emit `habits` directly,
/// but as a deterministic safety net we (a) convert a single recurring event/task it routed as a
/// one-off, (b) synthesize a habit from the text when it emitted nothing usable, and (c) suppress
/// any one-off event/task it double-emitted next to the habit. Always dedupes.
/// Detect a recurring routine and its cadence from the user's text → (cadence, weekdays, interval).
/// "every other day"/"every N days" → interval; weekdays/"weekdays"/"weekends" or "every <weekday>"
/// → weekly; plain "every day"/"daily" → daily. None when there's no recurrence.
fn recurrence(text: &str) -> Option<(String, Vec<u8>, i64)> {
    let t = text.to_lowercase();
    let wd = |s: &str| -> Option<u8> {
        Some(match s {
            "monday" | "mon" | "mondays" => 1,
            "tuesday" | "tue" | "tues" | "tuesdays" => 2,
            "wednesday" | "wed" | "weds" | "wednesdays" => 3,
            "thursday" | "thu" | "thur" | "thurs" | "thursdays" => 4,
            "friday" | "fri" | "fridays" => 5,
            "saturday" | "sat" | "saturdays" => 6,
            "sunday" | "sun" | "sundays" => 7,
            _ => return None,
        })
    };
    let toks: Vec<&str> = t.split(|c: char| !c.is_ascii_alphanumeric()).filter(|s| !s.is_empty()).collect();

    // interval — "every other day", "every 3 days"
    if t.contains("every other day") || t.contains("every second day") || t.contains("alternate days") {
        return Some(("interval".into(), vec![], 2));
    }
    for w in toks.windows(3) {
        if w[0] == "every" {
            if let Some(n) = w[1].parse::<i64>().ok().or_else(|| word_number(w[1])) {
                if matches!(w[2], "day" | "days") && (2..=30).contains(&n) {
                    return Some(("interval".into(), vec![], n));
                }
            }
        }
    }
    // weekday sets
    if t.contains("weekday") || t.contains("week day") {
        return Some(("weekly".into(), vec![1, 2, 3, 4, 5], 1));
    }
    if t.contains("weekend") {
        return Some(("weekly".into(), vec![6, 7], 1));
    }
    // specific weekdays, but only with a recurrence cue ("every"/"each", or a plural like "mondays")
    let cue = t.contains("every") || t.contains("each") || toks.iter().any(|w| w.ends_with('s') && wd(w).is_some());
    if cue {
        let mut days: Vec<u8> = Vec::new();
        for w in &toks {
            if let Some(d) = wd(w) {
                if !days.contains(&d) {
                    days.push(d);
                }
            }
        }
        if !days.is_empty() {
            // "every day except sunday" / "weekdays but not friday": the named days are EXCLUSIONS
            // from the full week, not the target days.
            if t.contains("except") || t.contains("but not") || t.contains("excluding") || t.contains("besides") {
                let ex = days.clone();
                days = (1u8..=7).filter(|d| !ex.contains(d)).collect();
            }
            days.sort_unstable();
            if !days.is_empty() {
                return Some(("weekly".into(), days, 1));
            }
        }
    }
    if daily_recurrence(text) {
        return Some(("daily".into(), vec![], 1));
    }
    None
}

/// Sequential task language ("fix the bug, **then** write tests, **then** deploy") implies an
/// ordered dependency chain. The small model lists the steps in order but rarely fills `depends_on`,
/// so chain each task onto the previous one (only where the model left deps empty). Gated on an
/// explicit sequencing cue so unordered lists ("buy milk and eggs") aren't chained.
fn backfill_task_dependencies(plan: &mut ParsedPlan, user_text: &str) {
    let lc = user_text.to_lowercase();
    let sequential = lc.contains(" then ")
        || lc.contains(", then")
        || lc.contains("and then")
        || lc.contains("after that")
        || lc.contains("followed by");
    if !sequential {
        return;
    }
    for proj in &mut plan.projects {
        for i in 1..proj.tasks.len() {
            if proj.tasks[i].depends_on.is_empty() {
                let prev = proj.tasks[i - 1].title.clone();
                if !prev.trim().is_empty() {
                    proj.tasks[i].depends_on.push(prev);
                }
            }
        }
    }
}

/// Split a message into clauses on punctuation and conjunctions, so a single recurring clause can be
/// isolated from a multi-intent sentence ("dinner Sunday at 6pm; and journal every night").
fn split_clauses(text: &str) -> Vec<String> {
    let mut parts = vec![text.to_string()];
    for sep in [";", ",", " and ", " plus ", " also "] {
        parts = parts.iter().flat_map(|p| p.split(sep)).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    }
    parts
}

/// Multi-intent rescue: when a message mixes a recurring routine with other items, the small model
/// often routes the routine as a one-off task/event (or drops it). Walk each clause; any that carries
/// its OWN recurrence cue becomes a habit (with that clause's cadence/duration) unless one already
/// names it. The same-named one-off the model mis-emitted is then dropped by the dedupe pass in
/// `route_recurring_to_habits`. Per-clause `recurrence` is more precise than the whole-message call —
/// it won't fold an unrelated "dinner Sunday" into the journal habit's cadence.
fn recover_recurring_clauses(plan: &mut ParsedPlan, user_text: &str) {
    for clause in split_clauses(user_text) {
        if let Some((cadence, days, interval_days)) = recurrence(&clause) {
            let name = match synthesize_habit_name(&clause) {
                Some(n) => n,
                None => continue,
            };
            if plan.habits.iter().any(|h| titles_refer_same(&h.name, &name)) {
                continue;
            }
            let d = find_duration_minutes(&clause).or_else(|| range_minutes(&clause)).unwrap_or(30);
            plan.habits.push(ParsedHabit { name, duration_minutes: d, cadence, days, interval_days });
        }
    }
}

fn route_recurring_to_habits(plan: &mut ParsedPlan, user_text: &str) {
    if let Some((cadence, days, interval_days)) = recurrence(user_text) {
        let dur = find_duration_minutes(user_text).or_else(|| range_minutes(user_text));

        if plan.habits.is_empty() {
            // Cluster everything the model emitted by subject. For a recurring routine it commonly
            // DOUBLE-EMITS the same thing — an event AND a same-named update (and sometimes a task).
            // If it's all one subject, collapse to a single habit (the cleanup below drops the rest).
            let mut subjects: Vec<String> = Vec::new();
            let add = |t: &str, subjects: &mut Vec<String>| {
                let t = t.trim();
                if !t.is_empty() && !is_placeholder_title(t) && !subjects.iter().any(|s| titles_refer_same(s, t)) {
                    subjects.push(t.to_string());
                }
            };
            for e in &plan.events {
                add(&e.title, &mut subjects);
            }
            for u in &plan.update_events {
                add(&u.target, &mut subjects);
            }
            for p in &plan.projects {
                for t in &p.tasks {
                    add(&t.title, &mut subjects);
                }
            }

            if subjects.len() == 1 {
                // One recurring subject (possibly split across event/update/task) → one habit.
                let name = plan
                    .events
                    .first()
                    .map(|e| e.title.clone())
                    .or_else(|| plan.projects.iter().flat_map(|p| &p.tasks).next().map(|t| t.title.clone()))
                    .unwrap_or_else(|| subjects[0].clone());
                let d = dur
                    .or_else(|| plan.events.first().and_then(|e| e.duration_minutes))
                    .or_else(|| plan.projects.iter().flat_map(|p| &p.tasks).next().map(|t| t.estimated_minutes.max(15)))
                    .unwrap_or(60);
                plan.habits.push(ParsedHabit { name, duration_minutes: d, ..Default::default() });
            } else if subjects.is_empty() {
                // The model produced nothing actionable (just asked "what time?") — synthesize, so a
                // clear recurring routine isn't silently dropped.
                if let Some(name) = synthesize_habit_name(user_text) {
                    plan.habits.push(ParsedHabit { name, duration_minutes: dur.unwrap_or(30), ..Default::default() });
                }
            } else {
                // Multi-subject mixed message (routine + unrelated items): isolate the recurring
                // clause(s) and recover them as habit(s) instead of leaving the routine mis-routed as
                // a one-off task/event (the multi-intent habit-drop failure).
                recover_recurring_clauses(plan, user_text);
            }
        }

        // A recurring routine IS the habit — it must not also linger as a one-off event/update/task.
        // The model often double-emits ("Practice violin" event beside the "Violin practice"
        // habit); drop creates/updates/tasks that name the same thing (order-insensitive).
        if !plan.habits.is_empty() {
            let names: Vec<String> = plan.habits.iter().map(|h| h.name.clone()).collect();
            let dup = |title: &str| names.iter().any(|n| titles_refer_same(n, title));
            plan.events.retain(|e| !dup(&e.title));
            plan.update_events.retain(|u| !dup(&u.target));
            for proj in &mut plan.projects {
                proj.tasks.retain(|t| !dup(&t.title));
            }
            plan.projects.retain(|p| !p.tasks.is_empty());
        }

        // Stamp the detected cadence onto every habit the model couldn't express it for. Skip habits
        // that already carry a cadence (the per-clause recovery sets its own, more precise one — the
        // whole-message `recurrence` can mis-fold an unrelated weekday into it).
        for h in &mut plan.habits {
            if h.cadence.is_empty() {
                h.cadence = cadence.clone();
                h.days = days.clone();
                h.interval_days = interval_days;
            }
        }
    }

    // Dedupe by name and clamp durations to something sane.
    let mut seen = HashSet::new();
    plan.habits.retain(|h| !h.name.trim().is_empty() && seen.insert(h.name.trim().to_lowercase()));
    for h in &mut plan.habits {
        h.duration_minutes = h.duration_minutes.clamp(5, 24 * 60);
    }
}

/// Distinct day words ("today", "tomorrow", a weekday) appearing in the user's text, in
/// order. Used to spread a single stated day across every event in the message.
fn find_day_phrases(text: &str) -> Vec<String> {
    const DAYS: &[&str] = &["today", "tomorrow", "monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday"];
    let mut found: Vec<String> = Vec::new();
    for tok in text.to_lowercase().split(|c: char| !c.is_ascii_alphabetic()) {
        if DAYS.contains(&tok) && !found.iter().any(|f| f == tok) {
            found.push(tok.to_string());
        }
    }
    found
}

/// Two or more named days in one message describe a multi-day span ("orientation wednesday
/// and thursday" → all-day Wed–Thu). Returns (start date, day count) from earliest to latest.
fn find_weekday_span(text: &str, today: NaiveDate) -> Option<(NaiveDate, i64)> {
    let mut dates: Vec<NaiveDate> = find_day_phrases(text).iter().filter_map(|d| resolve_day(today, d)).collect();
    dates.sort();
    dates.dedup();
    if dates.len() < 2 {
        return None;
    }
    let (lo, hi) = (*dates.first()?, *dates.last()?);
    let span = (hi - lo).num_days() + 1;
    (2..=31).contains(&span).then_some((lo, span))
}

fn word_number(w: &str) -> Option<i64> {
    Some(match w {
        "a" | "an" | "one" => 1,
        "two" => 2,
        "three" => 3,
        "four" => 4,
        "five" => 5,
        "six" => 6,
        "seven" => 7,
        "eight" => 8,
        "nine" => 9,
        "ten" => 10,
        _ => return None,
    })
}

/// A multi-day span in the text ("two weeks" → 14, "5 days" → 5, "a week" → 7). Drives
/// all-day multi-day events (trips), which the model can't express as start/end times.
fn find_span_days(text: &str) -> Option<i64> {
    let lower = text.to_lowercase();
    let toks: Vec<&str> = lower.split(|c: char| !c.is_ascii_alphanumeric()).filter(|s| !s.is_empty()).collect();
    for w in toks.windows(2) {
        let count = w[0].parse::<i64>().ok().or_else(|| word_number(w[0]));
        if let Some(c) = count.filter(|c| *c > 0 && *c <= 365) {
            match w[1] {
                "day" | "days" => return Some(c),
                "week" | "weeks" => return Some(c * 7),
                _ => {}
            }
        }
    }
    None
}

/// An explicit "M/D" or "M/D/YYYY" date in the text (the model is unreliable at numeric
/// dates). Year defaults to the current year, bumped forward if the bare date is well past.
fn find_explicit_date(text: &str, today: NaiveDate) -> Option<NaiveDate> {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut i = 0;
    while i < n {
        if chars[i].is_ascii_digit() && (i == 0 || !chars[i - 1].is_ascii_digit()) {
            let ms = i;
            while i < n && chars[i].is_ascii_digit() && i - ms < 2 {
                i += 1;
            }
            if i < n && chars[i] == '/' {
                let ds = i + 1;
                let mut de = ds;
                while de < n && chars[de].is_ascii_digit() && de - ds < 2 {
                    de += 1;
                }
                if de > ds {
                    let month: u32 = chars[ms..i].iter().collect::<String>().parse().unwrap_or(0);
                    let day: u32 = chars[ds..de].iter().collect::<String>().parse().unwrap_or(0);
                    let mut year = today.year();
                    let mut had_year = false;
                    if de < n && chars[de] == '/' {
                        let ys = de + 1;
                        let mut ye = ys;
                        while ye < n && chars[ye].is_ascii_digit() && ye - ys < 4 {
                            ye += 1;
                        }
                        if let Ok(y) = chars[ys..ye].iter().collect::<String>().parse::<i32>() {
                            year = if y < 100 { 2000 + y } else { y };
                            had_year = true;
                        }
                    }
                    if (1..=12).contains(&month) && (1..=31).contains(&day) {
                        if let Some(d) = NaiveDate::from_ymd_opt(year, month, day) {
                            if !had_year && d < today - Duration::days(30) {
                                return NaiveDate::from_ymd_opt(year + 1, month, day).or(Some(d));
                            }
                            return Some(d);
                        }
                    }
                }
            }
        }
        i += 1;
    }
    None
}

/// Day-of-month from an ordinal token ("25th" → 25), 1..=31. Requires the `st/nd/rd/th` suffix.
fn parse_ordinal(tok: &str) -> Option<u32> {
    for suf in ["st", "nd", "rd", "th"] {
        if let Some(num) = tok.strip_suffix(suf) {
            if let Ok(d) = num.parse::<u32>() {
                if (1..=31).contains(&d) {
                    return Some(d);
                }
            }
        }
    }
    None
}

/// The next calendar date with day-of-month `day` (this month if not yet past, else the next month
/// that has that day). Skips months without the day (e.g. the 31st of a 30-day month).
fn next_day_of_month(today: NaiveDate, day: u32) -> Option<NaiveDate> {
    for add in 0..=13u32 {
        let base = today.checked_add_months(chrono::Months::new(add))?;
        if let Some(d) = NaiveDate::from_ymd_opt(base.year(), base.month(), day) {
            if d >= today {
                return Some(d);
            }
        }
    }
    None
}

/// An ordinal day-of-month the user wrote ("on the 25th", "the 3rd") → the next such date. The model
/// can't express a bare day-of-month and Rust owns dates, so we resolve it here. Requires a preceding
/// "the" so rank words ("my 1st meeting") aren't mistaken for a date.
fn find_day_of_month(text: &str, today: NaiveDate) -> Option<NaiveDate> {
    let lower = text.to_lowercase();
    let toks: Vec<&str> = lower.split(|c: char| !c.is_ascii_alphanumeric()).filter(|s| !s.is_empty()).collect();
    for w in toks.windows(2) {
        if w[0] == "the" {
            if let Some(day) = parse_ordinal(w[1]) {
                if let Some(d) = next_day_of_month(today, day) {
                    return Some(d);
                }
            }
        }
    }
    None
}

/// A relative date the model can't express, resolved in Rust: "the day after tomorrow" (+2),
/// "in N days/weeks", "within N days", "in a week". Requires the "in"/"within" preposition so a
/// bare duration ("for two weeks" = a trip span) is NOT mistaken for a date — see the span guard in
/// `backfill_event_fields`.
fn find_relative_date(text: &str, today: NaiveDate) -> Option<NaiveDate> {
    let lc = text.to_lowercase();
    if lc.contains("day after tomorrow") {
        return Some(today + Duration::days(2));
    }
    let toks: Vec<&str> = lc.split(|c: char| !c.is_ascii_alphanumeric()).filter(|s| !s.is_empty()).collect();
    for w in toks.windows(3) {
        if w[0] == "in" || w[0] == "within" {
            if let Some(n) = w[1].parse::<i64>().ok().or_else(|| word_number(w[1])) {
                let days = match w[2] {
                    "day" | "days" => Some(n),
                    "week" | "weeks" => Some(n * 7),
                    _ => None,
                };
                if let Some(d) = days.filter(|d| *d > 0 && *d <= 365) {
                    return Some(today + Duration::days(d));
                }
            }
        }
    }
    None
}

/// Some models HTML-escape titles ("A&M" → "A&amp;M"). Undo the common entities so titles
/// render correctly. `&amp;` is unescaped last to avoid double-decoding.
fn unescape_html(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Clean HTML entities out of every title-ish field the model produced.
fn unescape_plan(plan: &mut ParsedPlan) {
    for e in &mut plan.events {
        e.title = unescape_html(&e.title);
    }
    for u in &mut plan.update_events {
        u.target = unescape_html(&u.target);
        u.title = u.title.as_deref().map(unescape_html);
    }
    for r in &mut plan.remove_events {
        *r = unescape_html(r);
    }
    for p in &mut plan.projects {
        p.name = unescape_html(&p.name);
        for t in &mut p.tasks {
            t.title = unescape_html(&t.title);
        }
    }
    for h in &mut plan.habits {
        h.name = unescape_html(&h.name);
    }
}

/// "make it a full day" language → the targeted event(s) should be all-day.
fn find_all_day(text: &str) -> bool {
    let t = text.to_lowercase();
    ["full day", "full-day", "all day", "all-day", "whole day", "entire day"].iter().any(|p| t.contains(p))
}

/// The small model frequently drops or mis-assigns the optional fields it's worst at:
/// `endTime`/`durationMinutes` (→ events collapse to the 60-min default or duration edits
/// loop) and the day when one day covers several events ("birthday lunch tomorrow … and a
/// party … as well" → only one lands on tomorrow). Since the user literally typed these,
/// recover them deterministically from their text — the same way we never trust the model
/// for dates. Only fills/corrects what the model got wrong.
fn backfill_event_fields(plan: &mut ParsedPlan, user_text: &str, today: NaiveDate) {
    // --- Day: if the user named exactly ONE day for the whole message, it applies to every
    // event they're creating ("… tomorrow … and a party … as well"). One day word means
    // there's no other day they could have meant, so override the model's guess too. ---
    let days = find_day_phrases(user_text);
    if days.len() == 1 {
        for ev in &mut plan.events {
            ev.day = Some(days[0].clone());
            ev.date = None; // a model-invented date must not override the day we just set
        }
        // For edits, only correct a day the model already intended to change (don't
        // fabricate a move on a pure rename/time edit that happens to mention a day).
        for up in &mut plan.update_events {
            if up.day.is_some() {
                up.day = Some(days[0].clone());
            }
        }
    }

    // --- "full day" / "all day": mark every targeted event all-day (span 1), unless it
    // already has a multi-day span. Applies broadly since "all day" is unambiguous. ---
    if find_all_day(user_text) {
        for ev in &mut plan.events {
            ev.span_days = ev.span_days.or(Some(1));
        }
        for up in &mut plan.update_events {
            up.span_days = up.span_days.or(Some(1));
        }
    }

    let range = find_time_range(user_text);
    let duration = find_duration_minutes(user_text);
    // "in two weeks" is a relative DATE, not a trip span — when one is present, suppress the span so
    // the event lands on that day instead of becoming an all-day multi-day block. ("for two weeks"
    // has no "in"/"within", so trips are unaffected.)
    let relative_date = find_relative_date(user_text, today);
    let span = if relative_date.is_some() { None } else { find_span_days(user_text) };
    // An explicit M/D, an ordinal day-of-month ("the 25th"), or a relative date ("the day after
    // tomorrow", "in two weeks") — all resolved in Rust, never trusted from the model.
    let explicit_date = find_explicit_date(user_text, today)
        .or_else(|| find_day_of_month(user_text, today))
        .or(relative_date);
    // A range of named days ("wednesday and thursday") → a multi-day all-day span.
    let weekday_span = find_weekday_span(user_text, today);
    let shift = find_time_shift(user_text);
    if range.is_none() && duration.is_none() && span.is_none() && explicit_date.is_none() && weekday_span.is_none() && shift.is_none() {
        return;
    }
    let date_str = explicit_date.map(|d| d.format("%Y-%m-%d").to_string());
    // A time string is "unset" if the model omitted it or it can't be parsed.
    let unset = |s: &Option<String>| s.as_deref().and_then(parse_hm).is_none();

    // --- Named day range ("wednesday to thursday") = ONE multi-day all-day event. The model
    // often splits it into a create + a per-day update (or several creates); collapse those
    // back into a single spanning event so it isn't a lone 1-hour block. Only do this when the
    // message is about a SINGLE event — "lunch today and dinner tomorrow" is two events, not a
    // span, so we leave those alone. ---
    let distinct_titles: HashSet<String> = plan.events.iter().map(|e| e.title.to_lowercase()).collect();
    let distinct_targets: HashSet<String> = plan.update_events.iter().map(|u| u.target.to_lowercase()).collect();
    let single_subject = if plan.events.is_empty() {
        distinct_targets.len() <= 1
    } else {
        distinct_titles.len() == 1 && distinct_targets.iter().all(|t| event_matches(&plan.events[0].title, t))
    };
    if let Some((lo, sp)) = weekday_span.filter(|_| single_subject) {
        let date = lo.format("%Y-%m-%d").to_string();
        if !plan.events.is_empty() {
            let title = {
                let ev = &mut plan.events[0];
                ev.date = Some(date);
                ev.day = None;
                ev.span_days = Some(sp);
                ev.title.clone()
            };
            // Drop the model's duplicate per-day copies of the same event.
            let mut seen = HashSet::new();
            plan.events.retain(|e| seen.insert(e.title.to_lowercase()));
            plan.update_events.retain(|u| !event_matches(&title, &u.target));
        } else if !plan.update_events.is_empty() {
            {
                let up = &mut plan.update_events[0];
                up.date = Some(date);
                up.day = None;
                up.span_days = Some(sp);
            }
            let target = plan.update_events[0].target.clone();
            let mut kept_first = false;
            plan.update_events.retain(|u| {
                if event_matches(&target, &u.target) {
                    let keep = !kept_first;
                    kept_first = true;
                    keep
                } else {
                    true
                }
            });
        }
    }

    // Explicit-date trip ("from 6/12 … for two weeks"): the model often emits the create PLUS
    // several self-updates of the same event, which makes the single-event span logic below bail.
    // Collapse to one all-day spanning event and drop the redundant updates. (Named-weekday ranges
    // are handled by the block above; this covers numeric spans with an explicit/own date.)
    if let Some(sp) = span.filter(|_| single_subject && weekday_span.is_none()) {
        if !plan.events.is_empty() {
            let title = {
                let ev = &mut plan.events[0];
                ev.span_days = Some(sp);
                if let Some(d) = &date_str {
                    ev.date = Some(d.clone());
                }
                ev.title.clone()
            };
            let mut seen = HashSet::new();
            plan.events.retain(|e| seen.insert(e.title.to_lowercase()));
            plan.update_events.retain(|u| !event_matches(&title, &u.target));
        }
    }

    // Multiple events, each with its OWN range in reading order ("lunch 12-2 and a party 6-10")
    // → assign ranges to events positionally. The single-target logic below bails on multiple
    // events, so without this each collapses to the 60-min default. Gated to equal counts (and no
    // edits/removes) so a stray time can't shift the mapping.
    let all_ranges = find_time_ranges(user_text);
    if plan.update_events.is_empty() && plan.remove_events.is_empty() && plan.events.len() >= 2 && plan.events.len() == all_ranges.len() {
        for (ev, (start_norm, end_raw)) in plan.events.iter_mut().zip(all_ranges.iter()) {
            if unset(&ev.start_time) {
                ev.start_time = Some(start_norm.format("%H:%M").to_string());
            }
            if unset(&ev.end_time) {
                ev.end_time = Some(end_raw.clone());
            }
        }
    }

    // Only act when there's a single, unambiguous target, so a range/length is never
    // mis-assigned across multiple events in one message. (Counts reflect the collapse above.)
    let single_create = plan.events.len() == 1 && plan.update_events.is_empty() && plan.remove_events.is_empty();
    let single_update = plan.update_events.len() == 1 && plan.events.is_empty() && plan.remove_events.is_empty();

    if single_create {
        let ev = &mut plan.events[0];
        if let Some(d) = &date_str {
            ev.date = Some(d.clone()); // user typed "6/12" — trust it over the model
        }
        if span.is_some() {
            ev.span_days = span; // multi-day trip → all-day, overriding any bogus end time
        }
        if let Some((start_norm, end_raw)) = range {
            if unset(&ev.start_time) {
                ev.start_time = Some(start_norm.format("%H:%M").to_string());
            }
            if unset(&ev.end_time) {
                ev.end_time = Some(end_raw);
            }
        } else if let Some(d) = duration {
            // A bare "2 hour meeting at 3pm": fill length only if the model gave no end.
            if ev.duration_minutes.is_none() && unset(&ev.end_time) {
                ev.duration_minutes = Some(d);
            }
        }
    } else if single_update {
        let up = &mut plan.update_events[0];
        if let Some(d) = &date_str {
            up.date = Some(d.clone());
        }
        if span.is_some() {
            up.span_days = span;
        }
        if let Some((start_norm, end_raw)) = range {
            if unset(&up.start_time) {
                up.start_time = Some(start_norm.format("%H:%M").to_string());
            }
            if unset(&up.end_time) {
                up.end_time = Some(end_raw);
            }
        } else if let Some(d) = duration {
            // "make the meeting 2 hours" — keep the existing start, just set the new length.
            if up.duration_minutes.is_none() && unset(&up.end_time) {
                up.duration_minutes = Some(d);
            }
        }
        // Relative shift ("push it back an hour") — recorded here, applied to the existing event's
        // current start in `store_plan` (the model is unreliable at this arithmetic). A shift and an
        // explicit new time don't co-occur, so a present `start_time` takes precedence downstream.
        if up.start_time.is_none() {
            up.shift_minutes = shift;
        }
    }
}

/// A bare magnitude of time ("an hour" → 60, "half an hour" → 30, "30 minutes" → 30), for relative
/// shifts where the quantity may be worded ("an hour") rather than numeric.
fn shift_magnitude(lc: &str) -> Option<i64> {
    if lc.contains("hour and a half") {
        return Some(90);
    }
    if lc.contains("half an hour") || lc.contains("half hour") {
        return Some(30);
    }
    if let Some(m) = find_duration_minutes(lc) {
        return Some(m); // "30 minutes", "2 hours", "1.5 hrs", "45 min"
    }
    if lc.contains("an hour") || lc.contains("a hour") || lc.contains("one hour") {
        return Some(60);
    }
    None
}

/// A relative time shift the user asked for on an existing event ("push it back an hour" → +60,
/// "move it up 30 minutes" → −30). Signed minutes; `None` if there's no clear shift phrase + amount.
/// "back/later/delay/postpone" = later (+); "up/earlier/sooner" = earlier (−). Ambiguous words
/// (a bare "forward"/"back") are intentionally excluded.
fn find_time_shift(text: &str) -> Option<i64> {
    let lc = text.to_lowercase();
    // "back" reads as later here — the dominant scheduling idiom ("push/move a meeting back" =
    // postpone). It only fires alongside an amount (below), so "move it back to 3pm" won't trip it.
    let later = ["back", "postpone", "delay", "later", "push out"];
    let earlier = ["move up", "moved up", "move it up", "bump up", "earlier", "sooner"];
    let dir = if later.iter().any(|p| lc.contains(p)) {
        1
    } else if earlier.iter().any(|p| lc.contains(p)) {
        -1
    } else {
        return None;
    };
    shift_magnitude(&lc).map(|m| dir * m)
}

/// Pull an explicit length ("2 hours", "90 min", "1.5 hrs") out of free text. Returns the
/// first match in minutes; ignores bare numbers without a unit ("instead of 1").
fn find_duration_minutes(text: &str) -> Option<i64> {
    let chars: Vec<char> = text.to_lowercase().chars().collect();
    let n = chars.len();
    let mut i = 0;
    while i < n {
        if !chars[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let num_start = i;
        while i < n && chars[i].is_ascii_digit() {
            i += 1;
        }
        if i + 1 < n && chars[i] == '.' && chars[i + 1].is_ascii_digit() {
            i += 1;
            while i < n && chars[i].is_ascii_digit() {
                i += 1;
            }
        }
        let value: f64 = chars[num_start..i].iter().collect::<String>().parse().unwrap_or(0.0);
        let mut j = i;
        while j < n && chars[j] == ' ' {
            j += 1;
        }
        let unit_start = j;
        while j < n && chars[j].is_ascii_alphabetic() {
            j += 1;
        }
        let unit: String = chars[unit_start..j].iter().collect();
        let mins = match unit.as_str() {
            "h" | "hr" | "hrs" | "hour" | "hours" => Some((value * 60.0).round() as i64),
            "m" | "min" | "mins" | "minute" | "minutes" => Some(value.round() as i64),
            _ => None,
        };
        if let Some(m) = mins {
            if m > 0 {
                return Some(m);
            }
        }
        i = j.max(i); // don't re-scan the unit
    }
    // Word-number durations the digit scan misses: "two hours", "an hour", "three hrs".
    let lower: String = chars.iter().collect();
    let toks: Vec<&str> = lower.split(|c: char| !c.is_ascii_alphanumeric()).filter(|s| !s.is_empty()).collect();
    for w in toks.windows(2) {
        if let Some(n) = word_number(w[0]) {
            match w[1] {
                "hour" | "hours" | "hr" | "hrs" => return Some(n * 60),
                "minute" | "minutes" | "min" | "mins" => return Some(n),
                _ => {}
            }
        }
    }
    None
}

/// Validate task deadlines (drop unparseable ones).
fn resolve_task_deadlines(plan: &mut ParsedPlan) {
    for proj in &mut plan.projects {
        for t in &mut proj.tasks {
            t.deadline = t
                .deadline
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("null"))
                .and_then(parse_dt)
                .map(fmt_dt);
        }
    }
}

/// Deadline dates stated in the user's text, resolved in Rust (the model is unreliable at dates,
/// and the `deadline` field's format is never shown to it, so its values are usually dropped).
/// Recognizes a keyword ("due/by/before/deadline") followed within a few words by a day word or
/// `M/D` date, plus relative forms ("in 3 weeks", "within 5 days"). Distinct + sorted.
fn find_deadline_dates(text: &str, today: NaiveDate) -> Vec<NaiveDate> {
    let lower = text.to_lowercase();
    let toks: Vec<&str> = lower.split(|c: char| !c.is_ascii_alphanumeric() && c != '/').filter(|s| !s.is_empty()).collect();
    let mut out: Vec<NaiveDate> = Vec::new();

    for (i, tok) in toks.iter().enumerate() {
        if !matches!(*tok, "due" | "by" | "before" | "deadline") {
            continue;
        }
        // Look a few tokens ahead for the first day word or explicit date.
        for t in toks.iter().skip(i + 1).take(4) {
            if let Some(d) = resolve_day(today, t).or_else(|| find_explicit_date(t, today)) {
                out.push(d);
                break;
            }
        }
    }
    // Relative deadlines: "in 3 weeks", "within 5 days", "in a week".
    for w in toks.windows(3) {
        if w[0] == "in" || w[0] == "within" {
            if let Some(n) = w[1].parse::<i64>().ok().or_else(|| word_number(w[1])) {
                let days = match w[2] {
                    "day" | "days" => Some(n),
                    "week" | "weeks" => Some(n * 7),
                    _ => None,
                };
                if let Some(d) = days.filter(|d| *d > 0 && *d <= 365) {
                    out.push(today + Duration::days(d));
                }
            }
        }
    }
    // Dedup keeping first-appearance order — positional per-task assignment relies on text order.
    let mut seen = HashSet::new();
    out.retain(|d| seen.insert(*d));
    out
}

/// Recover the task fields the small model gets wrong, the same way we recover event fields —
/// from the user's own words, deterministically:
///   • **Deadline** — a single deadline in the text applies to every task that lacks one
///     ("prep for the exam friday: …" → all due Friday). When the message states ONE deadline per
///     task ("ch.1 by Monday and ch.2 by Wednesday"), they're assigned in reading order.
///   • **Estimate** — for a lone task, an explicit length in the text ("study about 3 hours")
///     overrides the model's guess (which defaults to 60). Gated to a single task with no
///     competing event so a duration is never mis-assigned.
fn backfill_task_fields(plan: &mut ParsedPlan, text: &str, today: NaiveDate) {
    let total: usize = plan.projects.iter().map(|p| p.tasks.len()).sum();
    if total == 0 {
        return;
    }
    let task_only = plan.events.is_empty() && plan.update_events.is_empty();
    let mut deadlines = find_deadline_dates(text, today);
    // Fallback: in a message that's purely about tasks, a lone day word is the deadline
    // ("prep for my exam friday: review chapters…" → due Friday). Gated to task-only + exactly
    // one day so an event's day is never mistaken for a deadline.
    if deadlines.is_empty() && task_only {
        let days = find_day_phrases(text);
        if days.len() == 1 {
            if let Some(d) = resolve_day(today, &days[0]) {
                deadlines.push(d);
            }
        }
    }
    let eod = |d: NaiveDate| fmt_dt(d.and_hms_opt(23, 59, 0).unwrap());
    if deadlines.len() == 1 {
        // One deadline → every task that lacks one inherits it.
        let dl = eod(deadlines[0]);
        for proj in &mut plan.projects {
            for t in &mut proj.tasks {
                if t.deadline.as_deref().and_then(parse_dt).is_none() {
                    t.deadline = Some(dl.clone());
                }
            }
        }
    } else if deadlines.len() >= 2 && deadlines.len() == total {
        // One deadline per task, paired in reading order ("ch.1 by Monday and ch.2 by Wednesday").
        // No `task_only` gate: these come from EXPLICIT "by/due/before" phrases (never a bare day), and
        // the exact deadlines==tasks count guards mis-pairing — so it's safe even when the model also
        // double-emits a same-named event (which would otherwise disable the whole backfill).
        let mut it = deadlines.iter();
        for proj in &mut plan.projects {
            for t in &mut proj.tasks {
                if let Some(d) = it.next() {
                    if t.deadline.as_deref().and_then(parse_dt).is_none() {
                        t.deadline = Some(eod(*d));
                    }
                }
            }
        }
    }
    if total == 1 && plan.events.is_empty() && plan.update_events.is_empty() {
        if let Some(d) = find_duration_minutes(text) {
            if let Some(t) = plan.projects.iter_mut().flat_map(|p| p.tasks.iter_mut()).next() {
                t.estimated_minutes = d;
            }
        }
    }
}

/// Parse a time the model might write as "14:00", "2pm", "2:00 PM", "9", "14:00:00".
fn parse_hm(s: &str) -> Option<NaiveTime> {
    let lower = s.trim().to_lowercase();
    let pm = lower.contains("pm");
    let am = lower.contains("am");
    let digits: String = lower.chars().filter(|c| c.is_ascii_digit() || *c == ':').collect();
    let mut it = digits.split(':');
    let mut h: u32 = it.next()?.parse().ok()?;
    let m: u32 = it.next().unwrap_or("0").parse().unwrap_or(0);
    if pm && h < 12 {
        h += 12;
    }
    if am && h == 12 {
        h = 0;
    }
    NaiveTime::from_hms_opt(h, m, 0)
}

/// A clock time located in free text, with the slice it came from.
struct TimeTok {
    start: usize, // char index of the first digit
    end: usize,   // char index one past the last consumed char
    norm: NaiveTime,
    raw: String, // the verbatim slice ("2", "4pm", "5:30") — fed to `compute_end` later
}

/// Words/characters that join the two ends of a time range in free text.
fn is_range_gap(gap: &str) -> bool {
    matches!(
        gap.trim(),
        "-" | "–" | "—" | "to" | "til" | "till" | "until" | "thru" | "through"
    )
}

/// Blank out `YYYY-MM-DD` substrings so an ISO date isn't mistaken for a "06-04" range.
fn strip_iso_dates(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = chars.clone();
    let d = |c: char| c.is_ascii_digit();
    let mut i = 0;
    while i + 10 <= chars.len() {
        let c = &chars[i..i + 10];
        if d(c[0]) && d(c[1]) && d(c[2]) && d(c[3]) && c[4] == '-' && d(c[5]) && d(c[6]) && c[7] == '-' && d(c[8]) && d(c[9]) {
            for ch in out.iter_mut().skip(i).take(10) {
                *ch = ' ';
            }
            i += 10;
        } else {
            i += 1;
        }
    }
    out.into_iter().collect()
}

/// Try to read a clock time (`H`, `H:MM`, optional `am`/`pm`) starting at `start`.
/// Rejects 3+ digit runs (years) and anything embedded in a longer number.
fn parse_time_at(chars: &[char], start: usize) -> Option<TimeTok> {
    if start > 0 && (chars[start - 1].is_ascii_digit() || chars[start - 1] == ':') {
        return None; // mid-number / mid-time, not a fresh token
    }
    let mut j = start;
    while j < chars.len() && chars[j].is_ascii_digit() {
        j += 1;
    }
    if j == start || j - start > 2 {
        return None; // need 1–2 hour digits
    }
    let hour: u32 = chars[start..j].iter().collect::<String>().parse().ok()?;

    let mut minute: u32 = 0;
    let mut k = j;
    if k < chars.len() && chars[k] == ':' {
        let ms = k + 1;
        let mut me = ms;
        while me < chars.len() && chars[me].is_ascii_digit() && me - ms < 2 {
            me += 1;
        }
        if me == ms {
            return None; // lone colon
        }
        minute = chars[ms..me].iter().collect::<String>().parse().ok()?;
        k = me;
    }

    let mut m = k;
    while m < chars.len() && chars[m] == ' ' {
        m += 1;
    }
    let mut meridian: Option<bool> = None; // Some(true) = pm
    if m + 1 < chars.len() {
        let (c0, c1) = (chars[m].to_ascii_lowercase(), chars[m + 1].to_ascii_lowercase());
        if (c0 == 'a' || c0 == 'p') && c1 == 'm' {
            meridian = Some(c0 == 'p');
            m += 2;
        }
    }
    let end = if meridian.is_some() { m } else { k };

    if end < chars.len() && chars[end].is_ascii_digit() {
        return None; // bumps into a longer number
    }
    if minute > 59 {
        return None;
    }
    let valid = match meridian {
        Some(_) => (1..=12).contains(&hour),
        None => hour <= 23,
    };
    if !valid {
        return None;
    }
    let h24 = match meridian {
        Some(true) => if hour == 12 { 12 } else { hour + 12 },
        Some(false) => if hour == 12 { 0 } else { hour },
        // Bare hour: match the prompt's "assume PM for ambiguous hours" convention.
        None => if (1..=11).contains(&hour) { hour + 12 } else { hour },
    };
    Some(TimeTok {
        start,
        end,
        norm: NaiveTime::from_hms_opt(h24, minute, 0)?,
        raw: chars[start..end].iter().collect(),
    })
}

/// Find ALL time ranges ("12-2", "2pm to 4pm", "3:30–5") in free text, in reading order.
/// Each is (start normalized to 24h, the verbatim end slice). The end is returned raw so the
/// existing `compute_end` PM-recovery still handles "12-2" → 14:00, overnight, etc. Both ends of
/// a matched range are consumed, so "lunch 12-2 and a party 6-10" yields two distinct ranges.
fn find_time_ranges(text: &str) -> Vec<(NaiveTime, String)> {
    let chars: Vec<char> = strip_iso_dates(text).chars().collect();
    let mut toks: Vec<TimeTok> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_ascii_digit() {
            if let Some(tok) = parse_time_at(&chars, i) {
                i = tok.end;
                toks.push(tok);
                continue;
            }
        }
        i += 1;
    }
    let mut out = Vec::new();
    let mut k = 0;
    while k + 1 < toks.len() {
        let gap: String = chars[toks[k].end..toks[k + 1].start].iter().collect();
        if is_range_gap(&gap) {
            out.push((toks[k].norm, toks[k + 1].raw.clone()));
            k += 2; // consume both ends so the next range starts fresh
        } else {
            k += 1;
        }
    }
    out
}

/// The first time range in the text (the common single-event case).
fn find_time_range(text: &str) -> Option<(NaiveTime, String)> {
    find_time_ranges(text).into_iter().next()
}

/// Fuzzy match an existing event title against a user-provided needle (bidirectional
/// substring, so "sleepover" matches "Sleepover" and "all sleepovers" matches "Sleepover").
fn event_matches(event_title: &str, needle: &str) -> bool {
    let e = event_title.to_lowercase();
    let n = needle.trim().to_lowercase();
    !n.is_empty() && n.len() >= 3 && (e.contains(&n) || n.contains(&e))
}

fn weekday_from_str(s: &str) -> Option<Weekday> {
    Some(match s.to_lowercase().as_str() {
        "monday" => Weekday::Mon,
        "tuesday" => Weekday::Tue,
        "wednesday" => Weekday::Wed,
        "thursday" => Weekday::Thu,
        "friday" => Weekday::Fri,
        "saturday" => Weekday::Sat,
        "sunday" => Weekday::Sun,
        _ => return None,
    })
}

/// Resolve a day phrase to a date — Rust does this, never the model.
fn resolve_day(today: NaiveDate, day: &str) -> Option<NaiveDate> {
    match day.to_lowercase().as_str() {
        "today" => Some(today),
        "tomorrow" => Some(today + Duration::days(1)),
        other => weekday_from_str(other).map(|wd| {
            // Nearest upcoming occurrence (including today).
            (0..7)
                .map(|i| today + Duration::days(i))
                .find(|d| d.weekday() == wd)
                .unwrap_or(today)
        }),
    }
}

/// True if the event carries any usable time signal (start, end, a positive duration, or a
/// multi-day span).
fn event_has_time(ev: &ParsedEvent) -> bool {
    ev.start_time.as_deref().and_then(parse_hm).is_some()
        || ev.end_time.as_deref().and_then(parse_hm).is_some()
        || ev.duration_minutes.map(|d| d > 0).unwrap_or(false)
        || ev.span_days.map(|d| d >= 1).unwrap_or(false)
}

/// Resolve an event's (start, end). Returns None only when there's no day AND no time at
/// all — i.e. nothing concrete enough to place. A timed event with no day defaults to today
/// rather than being silently dropped ("added, but never scheduled").
fn resolve_event(now: NaiveDateTime, ev: &ParsedEvent) -> Option<(NaiveDateTime, NaiveDateTime)> {
    let explicit_date = ev
        .date
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("null"))
        .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok());
    let day_date = ev
        .day
        .as_deref()
        .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("null"))
        .and_then(|d| resolve_day(now.date(), d));

    // Guardrail: never lose a timed event just because the model omitted the day.
    let date = explicit_date.or(day_date).or_else(|| event_has_time(ev).then(|| now.date()))?;

    // Multi-day all-day event (a trip): span whole days from the start date. Takes precedence
    // over any (often bogus) end time the model produced for "two weeks".
    if let Some(span) = ev.span_days.filter(|d| *d >= 1) {
        let start = date.and_hms_opt(0, 0, 0).unwrap();
        let end = (date + Duration::days(span)).and_hms_opt(0, 0, 0).unwrap();
        return Some((start, end));
    }

    let start_time = ev.start_time.as_deref().and_then(parse_hm).unwrap_or(NaiveTime::from_hms_opt(12, 0, 0).unwrap());
    let start = date.and_time(start_time);
    // An explicit end wins; otherwise a stated duration; otherwise the 60-min default.
    let end = match ev.duration_minutes.and_then(sane_duration) {
        Some(d) if ev.end_time.as_deref().and_then(parse_hm).is_none() => start + Duration::minutes(d),
        _ => compute_end(start, ev.end_time.as_deref()),
    };
    Some((start, end_after(start, end)))
}

/// A multi-day event that carries a *daily* time window ("robotics competition, 3 days, 8am–5pm")
/// is several same-time days, not one all-day block — expand it into one dated `(start, end)` per
/// day so the window survives and the scheduler treats each day as busy. Returns `None` for a plain
/// multi-day span with no time of day (a trip), which stays a single all-day block in `resolve_event`.
/// Capped at 14 days so a long span ("30 days, 9–5") can't explode the calendar — those fall back
/// to the all-day path.
fn expand_daily_span(now: NaiveDateTime, ev: &ParsedEvent) -> Option<Vec<(NaiveDateTime, NaiveDateTime)>> {
    let span = ev.span_days.filter(|d| (2..=14).contains(d))?;
    let start_t = ev.start_time.as_deref().and_then(parse_hm)?;
    let has_end = ev.end_time.as_deref().and_then(parse_hm).is_some();
    let dur = ev.duration_minutes.and_then(sane_duration);
    if !has_end && dur.is_none() {
        return None; // a span with only a start time isn't a clear daily window — leave it all-day
    }
    // First-day date: explicit date, else the day word, else today (mirrors `resolve_event`).
    let date = ev
        .date
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("null"))
        .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
        .or_else(|| {
            ev.day
                .as_deref()
                .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("null"))
                .and_then(|d| resolve_day(now.date(), d))
        })
        .unwrap_or(now.date());
    let days = (0..span)
        .map(|i| {
            let start = (date + Duration::days(i)).and_time(start_t);
            let end = match dur {
                Some(m) if !has_end => start + Duration::minutes(m),
                _ => compute_end(start, ev.end_time.as_deref()),
            };
            (start, end_after(start, end))
        })
        .collect();
    Some(days)
}

/// Safety clamp: an event's end must always be strictly after its start, or the calendar
/// renders nothing and the scheduler skips it. Falls back to a 60-minute block.
fn end_after(start: NaiveDateTime, end: NaiveDateTime) -> NaiveDateTime {
    if end > start {
        end
    } else {
        start + Duration::minutes(60)
    }
}

/// Accept a duration only if it's positive and at most a day, so a stray huge value from the
/// model (or "120 hours") can't create a runaway multi-week block. `None` → fall back.
fn sane_duration(mins: i64) -> Option<i64> {
    (1..=24 * 60).contains(&mins).then_some(mins)
}

/// Reject a title the small model emitted as filler rather than a real name. Two failure modes
/// we've seen leak straight onto the calendar / task list as un-actionable junk:
///   - **blank / glyphless** — empty, whitespace, or only punctuation/symbols that render as
///     nothing (e.g. `<>`, `--`, a stray zero-width char) → no alphanumeric content at all;
///   - **template placeholders** — the model parrots a schema slot like `<NAME>`, `[task]`,
///     `{title}`, or a bare word like "untitled"/"tbd"/"n/a".
/// Used to gate task titles, project names, and event titles before we persist them.
fn is_placeholder_title(s: &str) -> bool {
    let t = s.trim();
    if !t.chars().any(|c| c.is_alphanumeric()) {
        return true; // blank, or only brackets/punctuation/zero-width — nothing to show
    }
    // Strip wrapping template brackets (<name>, [task], {title}) before matching bare words.
    let inner = t.trim_matches(|c: char| "<>[]{}()".contains(c)).trim().to_lowercase();
    matches!(
        inner.as_str(),
        "name" | "title" | "untitled" | "task" | "subtask" | "event" | "item"
            | "todo" | "to-do" | "tbd" | "n/a" | "na" | "none" | "null" | "placeholder"
            | "example" | "description" | "task name" | "event name" | "your task"
    )
}

/// Derive an event's end from its start + an optional end-time string.
/// Handles the model dropping PM ("12 - 2" → end "02:00" → 14:00) and overnight ranges
/// ("8pm - 8am" → 08:00 next day) via up to two +12h bumps.
fn compute_end(start: NaiveDateTime, end_time: Option<&str>) -> NaiveDateTime {
    match end_time.and_then(parse_hm) {
        Some(t) => {
            let mut e = start.date().and_time(t);
            if e <= start {
                e += Duration::hours(12); // dropped-PM (same day): 02:00 → 14:00
            }
            if e <= start {
                e += Duration::hours(12); // overnight (next day): 08:00 → 08:00 +1 day
            }
            if e > start {
                e
            } else {
                start + Duration::minutes(90)
            }
        }
        None => start + Duration::minutes(60),
    }
}

/// Apply a partial change to an existing event, keeping fields the user didn't mention
/// (so "move it to 9pm" preserves the duration, "end at 7am" preserves the start).
/// Returns (title, start_iso, end_iso).
#[allow(clippy::too_many_arguments)]
fn merge_event(
    existing: &Event,
    now: NaiveDateTime,
    day: Option<&str>,
    explicit_date: Option<&str>,
    start_time: Option<&str>,
    end_time: Option<&str>,
    duration: Option<i64>,
    span: Option<i64>,
    title: Option<&str>,
) -> (String, String, String) {
    let cur_start = parse_dt(&existing.start).unwrap_or(now);
    let cur_dur = parse_dt(&existing.end)
        .map(|e| (e - cur_start).num_minutes())
        .filter(|d| *d > 0)
        .unwrap_or(60);
    let date = explicit_date
        .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("null"))
        .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
        .or_else(|| {
            day.filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("null"))
                .and_then(|d| resolve_day(now.date(), d))
        })
        .unwrap_or(cur_start.date());
    let new_title = title
        .filter(|t| !t.trim().is_empty())
        .map(|t| t.to_string())
        .unwrap_or_else(|| existing.title.clone());

    // Multi-day all-day update (a trip): span whole days from the (possibly new) start date.
    if let Some(sp) = span.filter(|d| *d >= 1) {
        let s = date.and_hms_opt(0, 0, 0).unwrap();
        let e = (date + Duration::days(sp)).and_hms_opt(0, 0, 0).unwrap();
        return (new_title, fmt_dt(s), fmt_dt(e));
    }

    let st = start_time.and_then(parse_hm).unwrap_or(cur_start.time());
    let new_start = date.and_time(st);
    // An explicit end wins; else a stated new duration ("make it 2 hours"); else keep the
    // current length (so "move it to 9pm" preserves duration).
    let new_end = if end_time.and_then(parse_hm).is_some() {
        compute_end(new_start, end_time)
    } else if let Some(d) = duration.and_then(sane_duration) {
        new_start + Duration::minutes(d)
    } else {
        new_start + Duration::minutes(cur_dur)
    };
    (new_title, fmt_dt(new_start), fmt_dt(end_after(new_start, new_end)))
}

/// Generic "anything I can do?" filler the small model tacks on — never a real question.
fn is_chatter(c_lc: &str) -> bool {
    c_lc.contains("anything else") || c_lc.contains("is there anything") || c_lc.contains("let me know")
}

/// A question that merely confirms how to read a day/date — something Rust already resolved.
fn confirms_day(c_lc: &str) -> bool {
    const DAYS: &[&str] = &["today", "tomorrow", "monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday"];
    DAYS.iter().any(|d| c_lc.contains(d)) || c_lc.contains("referring to") || c_lc.contains("day after")
}

/// A question about a property an already-placed event necessarily has (its duration/length,
/// or a generic "this event" time question). Redundant once we've created/updated the event —
/// it has a concrete start and end. The `durationMinutes` field made the model especially
/// prone to asking "what is the duration in minutes?" even when the user gave a range.
fn asks_placed_property(c_lc: &str) -> bool {
    c_lc.contains("duration")
        || c_lc.contains("how long")
        || c_lc.contains("how many minutes")
        || c_lc.contains("length")
        || c_lc.contains("this event")
        || c_lc.contains("the event")
}

/// A question quibbling over the recurrence we already resolved by making it a daily habit
/// ("did you mean every weekday?" / "every week instead of everyday?"). Pure loop fuel.
fn is_recurrence_question(c_lc: &str) -> bool {
    c_lc.contains("weekday") || c_lc.contains("every week") || c_lc.contains("everyday")
        || c_lc.contains("every day") || c_lc.contains("each day") || c_lc.contains("weekly")
        || c_lc.contains("recurr")
}

/// Decide which clarifying questions to actually surface. The core job is breaking the
/// loop where, after successfully editing an event, the model keeps asking for the time/day
/// it just set. We drop any model question that names a distinctive word from an event we
/// just created/updated/removed (e.g. the title "Meeting with my friend" suppresses "what's
/// the start time of the meeting?"). Generic English filler is ignored so a question about a
/// *different* event still gets through. Our own "couldn't place X" questions are always kept.
fn filter_clarifications(
    model: &[String],
    extra: &[String],
    created: &[String],
    updated: &[String],
    removed: &[String],
    created_habits: &[String],
) -> Vec<String> {
    // Common words that carry no identity — matching on these would over-suppress.
    const FILLER: &[&str] = &["with", "the", "and", "you", "your", "for", "that", "this", "from", "into", "about", "new"];
    let touched_words: Vec<String> = created
        .iter()
        .chain(updated.iter())
        .chain(removed.iter())
        .chain(created_habits.iter())
        .flat_map(|t| t.to_lowercase().split_whitespace().map(str::to_string).collect::<Vec<_>>())
        .filter(|w| w.len() >= 3 && !FILLER.contains(&w.as_str()))
        .collect();
    let placed_event = !created.is_empty() || !updated.is_empty();
    let made_habit = !created_habits.is_empty();

    let mut out: Vec<String> = Vec::new();
    let push_unique = |dst: &mut Vec<String>, c: &str| {
        if !dst.iter().any(|x| x.eq_ignore_ascii_case(c)) {
            dst.push(c.to_string());
        }
    };
    for c in model {
        let c = c.trim();
        if !c.contains('?') {
            continue; // skip restatements / chatter
        }
        let c_lc = c.to_lowercase();
        if is_chatter(&c_lc) {
            continue; // "is there anything else…" filler, not a real question
        }
        if touched_words.iter().any(|w| c_lc.contains(w.as_str())) {
            continue; // already handled — we created/changed/removed that event
        }
        // Once an event is placed it has a concrete day, time, and length on the calendar, so
        // the model "confirming" a day ("is 'tomorrow' the day after today?") or asking for a
        // property it already set ("what is the duration in minutes?") is pure noise.
        if placed_event && (confirms_day(&c_lc) || asks_placed_property(&c_lc)) {
            continue;
        }
        // We turned a recurring routine into a daily habit — don't quibble over weekdays.
        if made_habit && is_recurrence_question(&c_lc) {
            continue;
        }
        push_unique(&mut out, c);
    }
    // Our own clarifications (an event we genuinely couldn't place) are always real.
    for c in extra {
        let c = c.trim();
        if c.contains('?') {
            push_unique(&mut out, c);
        }
    }
    out
}

/// Persist a parsed plan; resolve event dates in Rust; dedupe; clean up clarifications.
pub fn store_plan(conn: &Connection, settings: &Settings, plan: &ParsedPlan) -> Result<PlanOutcome> {
    let now = Local::now().naive_local();
    let mut created_task_ids = Vec::new();
    let mut project_names = Vec::new();
    let mut title_to_id: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    let mut extra_clarifications: Vec<String> = Vec::new();

    // ---- Habits (recurring routines) ----
    // Create new habits, skipping ones that already exist (so re-running doesn't duplicate).
    const HABIT_PALETTE: &[&str] = &["#22c55e", "#0ea5e9", "#a855f7", "#f59e0b", "#ec4899", "#14b8a6"];
    let mut created_habit_names: Vec<String> = Vec::new();
    if !plan.habits.is_empty() {
        let existing: HashSet<String> = crate::db::list_habits(conn)?.iter().map(|h| h.name.to_lowercase()).collect();
        let mut seen: HashSet<String> = existing.clone();
        for h in &plan.habits {
            let name = h.name.trim();
            let key = name.to_lowercase();
            if name.is_empty() || !seen.insert(key) {
                continue;
            }
            let color = HABIT_PALETTE[created_habit_names.len() % HABIT_PALETTE.len()];
            let cadence = if h.cadence.is_empty() { "daily" } else { &h.cadence };
            crate::db::insert_habit(conn, name, color, cadence, &h.days, h.interval_days.max(1), h.duration_minutes.clamp(5, 24 * 60))?;
            created_habit_names.push(name.to_string());
        }
    }
    // Anything we just made a habit must not also be created as an event/task.
    let habit_lc: HashSet<String> = created_habit_names.iter().map(|n| n.to_lowercase()).collect();

    // Projects + tasks. Skip tasks that duplicate an existing active task — the small model
    // tends to re-emit example tasks ("Pick platform") every turn, which otherwise piles up.
    let mut existing_task_lc: HashSet<String> =
        crate::db::list_tasks(conn)?.iter().filter(|t| t.status != "done").map(|t| t.title.to_lowercase()).collect();
    for proj in &plan.projects {
        // Keep only genuinely new tasks (not a duplicate, a habit, or blank).
        let mut seen_titles: HashSet<String> = HashSet::new();
        let new_tasks: Vec<&ParsedTask> = proj
            .tasks
            .iter()
            .filter(|t| {
                let lc = t.title.trim().to_lowercase();
                !is_placeholder_title(&t.title) && !habit_lc.contains(&lc) && !existing_task_lc.contains(&lc) && seen_titles.insert(lc)
            })
            .collect();
        if new_tasks.is_empty() {
            continue; // don't create an empty/duplicate project
        }
        // A junk project name ("<NAME>", blank) still gets its genuine tasks kept — just
        // unassigned ("No project") instead of spawning a garbage project header.
        let pid = if is_placeholder_title(&proj.name) {
            None
        } else {
            let id = crate::db::insert_project(conn, &proj.name, "#6366f1")?;
            project_names.push(proj.name.clone());
            Some(id)
        };
        for t in new_tasks {
            let min_chunk = if t.chunkable { settings.default_min_chunk } else { t.estimated_minutes.max(15) };
            let id = crate::db::insert_task(
                conn,
                pid,
                &t.title,
                &t.notes,
                t.estimated_minutes.max(15),
                t.deadline.as_deref(),
                priority_to_int(&t.priority),
                min_chunk,
                settings.default_max_chunk,
                &[],
            )?;
            title_to_id.insert(t.title.clone(), id);
            existing_task_lc.insert(t.title.trim().to_lowercase());
            created_task_ids.push(id);
        }
    }
    for proj in &plan.projects {
        for t in &proj.tasks {
            if let Some(&tid) = title_to_id.get(&t.title) {
                for dep_title in &t.depends_on {
                    if let Some(&dep_id) = title_to_id.get(dep_title) {
                        if dep_id != tid {
                            crate::db::add_task_dep(conn, tid, dep_id)?;
                        }
                    }
                }
            }
        }
    }

    // ---- Calendar event operations: remove → update → create ----
    let mut removed_event_titles = Vec::new();
    let mut updated_event_titles = Vec::new();

    // REMOVE existing events whose title matches a needle.
    if !plan.remove_events.is_empty() {
        for e in crate::db::list_events(conn)? {
            if plan.remove_events.iter().any(|n| event_matches(&e.title, n)) {
                crate::db::delete_event(conn, e.id)?;
                removed_event_titles.push(e.title);
            }
        }
    }

    // UPDATE existing events (new time/day/title), keeping unspecified fields.
    for up in &plan.update_events {
        for e in crate::db::list_events(conn)? {
            if !event_matches(&e.title, &up.target) {
                continue;
            }
            // A relative shift becomes a concrete new start, computed from THIS event's current
            // start (Rust does the arithmetic the model can't). An explicit start_time wins over it.
            let shifted = up
                .shift_minutes
                .filter(|_| up.start_time.is_none())
                .and_then(|d| parse_dt(&e.start).map(|s| (s + Duration::minutes(d)).format("%H:%M").to_string()));
            let start_arg = up.start_time.clone().or(shifted);
            let (t, s, en) = merge_event(&e, now, up.day.as_deref(), up.date.as_deref(), start_arg.as_deref(), up.end_time.as_deref(), up.duration_minutes, up.span_days, up.title.as_deref());
            crate::db::update_event(conn, e.id, &t, &s, &en)?;
            updated_event_titles.push(t);
        }
    }

    // CREATE — but the small model often routes an EDIT (e.g. "move the sleepover to 9pm")
    // as a fresh create. So if an event with the same title already exists, UPDATE it
    // instead of making a duplicate. This converges to one event per title and makes
    // edits work however the model routes them.
    let mut current = crate::db::list_events(conn)?;
    let mut created_event_titles = Vec::new();
    for ev in &plan.events {
        // Guardrail: never persist a blank or placeholder-titled event ("<NAME>", "", "event"),
        // it'd show as an un-addressable block. Also skip anything we just made a habit.
        if is_placeholder_title(&ev.title) || habit_lc.contains(&ev.title.trim().to_lowercase()) {
            continue;
        }
        // Phantom-duplicate guard: the 3B sometimes both edits an event AND emits a near-duplicate
        // create (e.g. updates "Dentist" but also creates "Dentist (original)"). If this create
        // fuzzy-matches something we just updated or removed this turn, drop it.
        if updated_event_titles.iter().chain(removed_event_titles.iter()).any(|t| event_matches(&ev.title, t)) {
            continue;
        }
        // Edit routed as a create: same title exists → merge the change in (keep unspecified fields).
        if let Some(existing) = current.iter_mut().find(|x| x.title.eq_ignore_ascii_case(&ev.title)) {
            let (t, s, e) = merge_event(existing, now, ev.day.as_deref(), ev.date.as_deref(), ev.start_time.as_deref(), ev.end_time.as_deref(), ev.duration_minutes, ev.span_days, None);
            crate::db::update_event(conn, existing.id, &t, &s, &e)?;
            existing.start = s;
            existing.end = e;
            updated_event_titles.push(t);
            continue;
        }
        // Genuinely new event. A multi-day event with a daily time window ("robotics competition,
        // 3 days, 8am–5pm") becomes one dated event per day so the 8–5 window survives; plain
        // multi-day trips (no time of day) fall through to the all-day span path in `resolve_event`.
        if let Some(days) = expand_daily_span(now, ev) {
            for (start, end) in days {
                let (s, e) = (fmt_dt(start), fmt_dt(end));
                let id = crate::db::insert_event(conn, &ev.title, &s, &e, "fixed")?;
                current.push(Event {
                    id,
                    title: ev.title.clone(),
                    start: s,
                    end: e,
                    kind: "fixed".into(),
                    source: "manual".into(),
                    created_at: String::new(),
                    provider: None,
                    external_id: None,
                    account_id: None,
                    etag: None,
                });
            }
            created_event_titles.push(ev.title.clone()); // one logical event for the chat summary
            continue;
        }
        match resolve_event(now, ev) {
            Some((start, end)) => {
                let (s, e) = (fmt_dt(start), fmt_dt(end));
                let id = crate::db::insert_event(conn, &ev.title, &s, &e, "fixed")?;
                created_event_titles.push(ev.title.clone());
                current.push(Event {
                    id,
                    title: ev.title.clone(),
                    start: s,
                    end: e,
                    kind: "fixed".into(),
                    source: "manual".into(),
                    created_at: String::new(),
                    provider: None,
                    external_id: None,
                    account_id: None,
                    etag: None,
                });
            }
            None => extra_clarifications.push(format!("What date and time is \"{}\"?", ev.title)),
        }
    }

    let clarifications = filter_clarifications(
        &plan.clarifications,
        &extra_clarifications,
        &created_event_titles,
        &updated_event_titles,
        &removed_event_titles,
        &created_habit_names,
    );

    Ok(PlanOutcome {
        created_task_ids,
        project_names,
        created_event_titles,
        updated_event_titles,
        removed_event_titles,
        created_habit_names,
        clarifications,
        recalled_notes: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(day: &str, st: Option<&str>, et: Option<&str>) -> ParsedEvent {
        ParsedEvent {
            title: "x".into(),
            day: Some(day.into()),
            date: None,
            start_time: st.map(String::from),
            end_time: et.map(String::from),
            duration_minutes: None,
            span_days: None,
        }
    }

    fn d() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 6, 12).unwrap()
    }

    #[test]
    fn system_prompt_injects_memory_only_when_present() {
        let s = Settings::default();
        // No memory → no memory block, prompt unchanged in that respect.
        let without = system_prompt(&[], &s, &[], "EXAMPLES");
        assert!(!without.contains("Notes the user has saved"));
        // With memory → the facts appear, newlines flattened, under the labeled block.
        let mem = vec!["Sarah prefers afternoon meetings".to_string(), "line one\nline two".to_string()];
        let with = system_prompt(&[], &s, &mem, "EXAMPLES");
        assert!(with.contains("Notes the user has saved"));
        assert!(with.contains("Sarah prefers afternoon meetings"));
        assert!(with.contains("line one line two"), "newlines in a fact are flattened");
        // Memory sits before the calendar/examples, not appended at the very end.
        assert!(with.find("Notes the user has saved").unwrap() < with.find("EXAMPLES").unwrap());
    }

    #[test]
    fn restraint_guard_suppresses_greeting_fabrication() {
        let mut e = ev("today", None, None);
        e.title = "How's it going today".into();
        let mut plan = ParsedPlan { events: vec![e], ..Default::default() };
        apply_restraint_guard(&mut plan, "Hey! How's it going today?", d());
        assert!(plan.events.is_empty(), "a greeting must not fabricate an event");
        assert!(!plan.clarifications.is_empty(), "should ask what to schedule instead");
    }

    #[test]
    fn restraint_guard_suppresses_past_completion() {
        let mut plan = ParsedPlan {
            projects: vec![ParsedProject {
                name: "Laundry".into(),
                tasks: vec![ParsedTask { title: "Laundry".into(), estimated_minutes: 60, priority: "medium".into(), ..Default::default() }],
            }],
            ..Default::default()
        };
        apply_restraint_guard(&mut plan, "I already finished the laundry earlier.", d());
        assert_eq!(plan.projects.iter().map(|p| p.tasks.len()).sum::<usize>(), 0, "a past-tense report is not a new task");
    }

    #[test]
    fn restraint_guard_spares_real_requests() {
        // Opens with a greeting but is a real timed request → must survive.
        let mut e = ev("tomorrow", Some("06:00"), None);
        e.title = "Gym".into();
        let mut plan = ParsedPlan { events: vec![e], ..Default::default() };
        apply_restraint_guard(&mut plan, "Hi! Add gym tomorrow at 6am.", d());
        assert_eq!(plan.events.len(), 1, "a real request opening with a greeting must not be suppressed");

        // Plain create, no greeting/past pattern at all.
        let mut e2 = ev("friday", Some("14:00"), None);
        e2.title = "Dentist".into();
        let mut plan2 = ParsedPlan { events: vec![e2], ..Default::default() };
        apply_restraint_guard(&mut plan2, "Dentist this Friday at 2pm.", d());
        assert_eq!(plan2.events.len(), 1);

        // A genuine past-completion verb but with future framing → not suppressed.
        let mut e3 = ev("tomorrow", Some("15:00"), None);
        e3.title = "Review".into();
        let mut plan3 = ParsedPlan { events: vec![e3], ..Default::default() };
        apply_restraint_guard(&mut plan3, "I finished chapter 1, schedule a review tomorrow at 3pm.", d());
        assert_eq!(plan3.events.len(), 1);
    }

    #[test]
    fn action_cue_and_noop_detectors() {
        assert!(mentions_clock_time("call at 3pm"));
        assert!(mentions_clock_time("standup 9:30"));
        assert!(mentions_clock_time("lunch at noon"));
        assert!(mentions_clock_time("meeting at 3"));
        assert!(mentions_clock_time("flight tomorrow at 1400"));
        assert!(!mentions_clock_time("how's it going today"));
        assert!(!mentions_clock_time("i am on the team"));

        assert!(has_action_cue("remind me to stretch every morning", d()));
        assert!(has_action_cue("schedule a review", d()));
        assert!(has_action_cue("dentist on 6/25", d()));
        assert!(!has_action_cue("hey how's it going", d()));
        assert!(!has_action_cue("i already finished the laundry earlier", d()));

        assert!(is_greeting("Hey! How's it going today?"));
        assert!(is_greeting("good morning"));
        assert!(!is_greeting("dentist friday at 2pm"));

        assert!(is_past_completion("I already finished the laundry earlier."));
        assert!(!is_past_completion("I need to finish the report by friday"));

        assert!(is_vague_plans("I have some stuff going on next week."));
        assert!(!is_vague_plans("dentist friday at 2pm"));
    }

    #[test]
    fn restraint_guard_suppresses_vague_elaboration() {
        // "some stuff going on next week" with no concrete time/title → the model's invented events
        // and tasks must be dropped.
        let mut plan = ParsedPlan {
            events: vec![
                { let mut e = ev("today", None, None); e.title = "Work meeting".into(); e },
                { let mut e = ev("today", None, None); e.title = "Team lunch".into(); e },
            ],
            projects: vec![ParsedProject {
                name: "Week".into(),
                tasks: vec![ParsedTask { title: "Project review".into(), estimated_minutes: 60, priority: "medium".into(), ..Default::default() }],
            }],
            ..Default::default()
        };
        apply_restraint_guard(&mut plan, "I have some stuff going on next week.", d());
        assert!(plan.events.is_empty() && plan.projects.iter().all(|p| p.tasks.is_empty()), "vague input must not fabricate a calendar");
    }

    #[test]
    fn multi_intent_recurring_clause_becomes_a_habit() {
        // The model mis-routed "stretch every morning" as a task beside the haircut event. The
        // recurring clause must be recovered as a (daily) habit, the mis-routed task dropped, and the
        // unrelated event left alone — and the per-clause daily cadence must NOT be clobbered by the
        // whole-message recurrence (which would mis-read the haircut's "Saturday" as weekly).
        let mut e = ev("saturday", Some("11:00"), None);
        e.title = "Haircut".into();
        let mut plan = ParsedPlan {
            events: vec![e],
            projects: vec![ParsedProject {
                name: "Misc".into(),
                tasks: vec![ParsedTask { title: "Stretch".into(), estimated_minutes: 15, priority: "medium".into(), ..Default::default() }],
            }],
            ..Default::default()
        };
        route_recurring_to_habits(&mut plan, "Book a haircut Saturday at 11am, and remind me to stretch every morning.");
        assert_eq!(plan.habits.len(), 1, "the recurring clause becomes exactly one habit");
        assert!(plan.habits[0].name.to_lowercase().contains("stretch"));
        assert_eq!(plan.habits[0].cadence, "daily", "per-clause daily cadence, not the haircut's Saturday");
        assert!(plan.events.iter().any(|e| e.title == "Haircut"), "the unrelated event survives");
        assert_eq!(plan.projects.iter().map(|p| p.tasks.len()).sum::<usize>(), 0, "the mis-routed task is dropped");
    }

    #[test]
    fn history_only_for_followups() {
        // Self-contained requests go in cold — no history to contaminate them.
        for standalone in [
            "on 6/12 i have a surgery at 10 am",
            "tomorrow i am going to study for 2 hours starting from 1 pm",
            "lunch with Sam friday at noon",
            "i need to write the report by friday",
        ] {
            assert!(!needs_history(standalone), "{standalone:?} should NOT pull history");
        }
        // Follow-ups / edits lean on prior turns — keep context.
        for followup in [
            "move it to 9pm",
            "this friday at 7pm",
            "actually make that 2 hours",
            "no, reschedule the meeting to Tuesday",
            "and also block 6-7pm",
            "cancel that",
        ] {
            assert!(needs_history(followup), "{followup:?} SHOULD pull history");
        }
    }

    #[test]
    fn rejects_blank_and_placeholder_titles() {
        // Real names the model should keep.
        for good in ["Surgery", "study for class", "Pick up the mail", "1:1 with Sam", "Gym 💪"] {
            assert!(!is_placeholder_title(good), "{good:?} should be kept");
        }
        // Junk the model parrots — blank, glyphless, or a template slot — must be dropped.
        for junk in ["", "   ", "<NAME>", "<name>", "[task]", "{title}", "()", "--", "...", "untitled", "TBD", "N/A", "Task Name"] {
            assert!(is_placeholder_title(junk), "{junk:?} should be rejected");
        }
    }

    #[test]
    fn parse_hm_formats() {
        let t = |h, m| NaiveTime::from_hms_opt(h, m, 0);
        assert_eq!(parse_hm("14:00"), t(14, 0));
        assert_eq!(parse_hm("2pm"), t(14, 0));
        assert_eq!(parse_hm("2:00 PM"), t(14, 0));
        assert_eq!(parse_hm("9"), t(9, 0));
        assert_eq!(parse_hm("12am"), t(0, 0));
        assert_eq!(parse_hm("14:00:00"), t(14, 0));
    }

    #[test]
    fn daily_window_span_expands_per_day() {
        let now = NaiveDate::from_ymd_opt(2026, 6, 9).unwrap().and_hms_opt(12, 0, 0).unwrap();
        // "robotics competition, 3 days, 8am–5pm" → three dated 8–5 days, not one all-day block.
        let mut e = ev("thursday", Some("08:00"), Some("5 pm"));
        e.span_days = Some(3);
        e.date = Some("2026-06-11".into());
        e.day = None;
        let days = expand_daily_span(now, &e).expect("a daily-window span expands per day");
        assert_eq!(days.len(), 3);
        for (i, (s, en)) in days.iter().enumerate() {
            assert_eq!(s.date(), NaiveDate::from_ymd_opt(2026, 6, 11 + i as u32).unwrap());
            assert_eq!(s.time(), NaiveTime::from_hms_opt(8, 0, 0).unwrap());
            assert_eq!((*en - *s).num_minutes(), 540); // 8am–5pm
        }
        // A plain multi-day trip (no time of day) stays a single all-day block (no expansion).
        let mut trip = ev("monday", None, None);
        trip.span_days = Some(14);
        assert!(expand_daily_span(now, &trip).is_none());
        // A long windowed span (> 14 days) also falls back to all-day, not hundreds of events.
        let mut long = ev("monday", Some("09:00"), Some("17:00"));
        long.span_days = Some(30);
        assert!(expand_daily_span(now, &long).is_none());
    }

    fn sample_event() -> crate::model::Event {
        crate::model::Event {
            id: 1,
            title: "Sleepover".into(),
            start: "2026-06-06T20:00:00".into(),
            end: "2026-06-07T08:00:00".into(), // 12h overnight
            kind: "fixed".into(),
            source: "manual".into(),
            created_at: String::new(),
            provider: None,
            external_id: None,
            account_id: None,
            etag: None,
        }
    }

    #[test]
    fn merge_keeps_unspecified_fields() {
        let now = NaiveDate::from_ymd_opt(2026, 6, 3).unwrap().and_hms_opt(9, 0, 0).unwrap();
        let e = sample_event();
        let dur = |s: &str, en: &str| (parse_dt(en).unwrap() - parse_dt(s).unwrap()).num_minutes();

        // merge_event(existing, now, day, date, start, end, duration, span, title)
        // Move start only → keep the 12h duration.
        let (_t, s, en) = merge_event(&e, now, None, None, Some("21:00"), None, None, None, None);
        assert_eq!(s, "2026-06-06T21:00:00");
        assert_eq!(dur(&s, &en), 720);

        // Change end only → keep the original start.
        let (_t, s, en) = merge_event(&e, now, None, None, None, Some("07:00"), None, None, None);
        assert_eq!(s, "2026-06-06T20:00:00");
        assert_eq!(dur(&s, &en), 660);

        // Rename only → keep times.
        let (t, s, _en) = merge_event(&e, now, None, None, None, None, None, None, Some("Movie Night"));
        assert_eq!(t, "Movie Night");
        assert_eq!(s, "2026-06-06T20:00:00");

        // New duration only ("make it 2 hours") → keep start, set length, ignore old end.
        let (_t, s, en) = merge_event(&e, now, None, None, None, None, Some(120), None, None);
        assert_eq!(s, "2026-06-06T20:00:00");
        assert_eq!(dur(&s, &en), 120);

        // An explicit end still wins over a duration if both are somehow present.
        let (_t, s, en) = merge_event(&e, now, None, None, None, Some("23:00"), Some(120), None, None);
        assert_eq!(dur(&s, &en), 180);

        // A multi-day span makes it an all-day trip from the start date.
        let (_t, s, en) = merge_event(&e, now, None, None, None, None, None, Some(14), None);
        assert_eq!(s, "2026-06-06T00:00:00");
        assert_eq!(en, "2026-06-20T00:00:00"); // +14 days, all-day
    }

    #[test]
    fn finds_time_ranges_in_text() {
        let t = |h, m| NaiveTime::from_hms_opt(h, m, 0).unwrap();
        // hyphenated, bare hours (assume-PM for start)
        assert_eq!(find_time_range("lunch with mom 12-2"), Some((t(12, 0), "2".into())));
        // explicit pm on both ends
        assert_eq!(find_time_range("meeting 2pm to 4pm"), Some((t(14, 0), "4pm".into())));
        // the reported "graduation party from 7-8" (bare hours, assume PM)
        assert_eq!(find_time_range("Akshat's graduation party from 7-8"), Some((t(19, 0), "8".into())));
        // "from X to Y", bare → assume PM
        assert_eq!(find_time_range("call from 3 to 5"), Some((t(15, 0), "5".into())));
        // overnight, end keeps its am marker for compute_end
        assert_eq!(find_time_range("sleepover 8pm to 8am"), Some((t(20, 0), "8am".into())));
        // minutes + en-dash
        assert_eq!(find_time_range("standup 3:30–5"), Some((t(15, 30), "5".into())));
        // a lone time is NOT a range
        assert_eq!(find_time_range("lunch at 1pm"), None);
        // an ISO date must not look like a "06-04" range
        assert_eq!(find_time_range("event on 2026-06-04 from 2-4pm"), Some((t(14, 0), "4pm".into())));
        assert_eq!(find_time_range("deadline 2026-06-04"), None);
    }

    #[test]
    fn backfill_recovers_dropped_end_time() {
        let now = NaiveDate::from_ymd_opt(2026, 6, 3).unwrap().and_hms_opt(9, 0, 0).unwrap();
        let dur = |ev: &ParsedEvent| {
            let (s, e) = resolve_event(now, ev).unwrap();
            (e - s).num_minutes()
        };

        // Model gave the start but dropped endTime → without backfill this is 60 min.
        let mut plan = ParsedPlan {
            events: vec![ev("friday", Some("12:00"), None)],
            ..Default::default()
        };
        backfill_event_fields(&mut plan, "lunch with mom friday 12-2", NaiveDate::from_ymd_opt(2026, 6, 8).unwrap());
        assert_eq!(plan.events[0].end_time.as_deref(), Some("2"));
        assert_eq!(dur(&plan.events[0]), 120); // recovered the 2-hour range

        // Model dropped BOTH times → recover start and end from the text.
        let mut plan = ParsedPlan {
            events: vec![ev("friday", None, None)],
            ..Default::default()
        };
        backfill_event_fields(&mut plan, "meeting friday 2pm to 4pm", NaiveDate::from_ymd_opt(2026, 6, 8).unwrap());
        assert_eq!(dur(&plan.events[0]), 120);

        // A correct endTime from the model is never overwritten.
        let mut plan = ParsedPlan {
            events: vec![ev("friday", Some("12:00"), Some("13:00"))],
            ..Default::default()
        };
        backfill_event_fields(&mut plan, "lunch friday 12-2", NaiveDate::from_ymd_opt(2026, 6, 8).unwrap());
        assert_eq!(plan.events[0].end_time.as_deref(), Some("13:00"));

        // No range in the text → nothing changes (still the 60-min default).
        let mut plan = ParsedPlan {
            events: vec![ev("friday", Some("12:00"), None)],
            ..Default::default()
        };
        backfill_event_fields(&mut plan, "lunch friday at noon", NaiveDate::from_ymd_opt(2026, 6, 8).unwrap());
        assert_eq!(plan.events[0].end_time, None);

        // Ambiguous: 2+ events in one message → don't guess which gets the range.
        let mut plan = ParsedPlan {
            events: vec![ev("friday", Some("12:00"), None), ev("friday", Some("15:00"), None)],
            ..Default::default()
        };
        backfill_event_fields(&mut plan, "lunch 12-2 and a call", NaiveDate::from_ymd_opt(2026, 6, 8).unwrap());
        assert_eq!(plan.events[0].end_time, None);
        assert_eq!(plan.events[1].end_time, None);
    }

    #[test]
    fn multi_event_ranges_assigned_positionally() {
        let now = NaiveDate::from_ymd_opt(2026, 6, 3).unwrap().and_hms_opt(9, 0, 0).unwrap();
        let dur = |e: &ParsedEvent| {
            let (s, en) = resolve_event(now, e).unwrap();
            (en - s).num_minutes()
        };
        let today = NaiveDate::from_ymd_opt(2026, 6, 8).unwrap();

        // Two events + two ranges, model dropped both end times → assign in reading order.
        let mut lunch = ev("friday", Some("12:00"), None);
        lunch.title = "Lunch with mom".into();
        let mut party = ev("friday", Some("18:00"), None);
        party.title = "Graduation party".into();
        let mut plan = ParsedPlan { events: vec![lunch, party], ..Default::default() };
        backfill_event_fields(&mut plan, "lunch with mom friday 12-2 and a graduation party from 6-10", today);
        assert_eq!(dur(&plan.events[0]), 120); // 12–2
        assert_eq!(dur(&plan.events[1]), 240); // 6–10

        // Count mismatch (two events, one range) → ambiguous, leave both alone.
        let mut a = ev("friday", Some("12:00"), None);
        a.title = "Lunch".into();
        let mut b = ev("friday", Some("15:00"), None);
        b.title = "Call".into();
        let mut plan = ParsedPlan { events: vec![a, b], ..Default::default() };
        backfill_event_fields(&mut plan, "lunch 12-2 and a call", today);
        assert_eq!(plan.events[0].end_time, None);
        assert_eq!(plan.events[1].end_time, None);
    }

    #[test]
    fn event_duration_handles_pm_less_end() {
        let now = NaiveDate::from_ymd_opt(2026, 6, 3).unwrap().and_hms_opt(9, 0, 0).unwrap();
        let dur = |st, et| {
            let (s, e) = resolve_event(now, &ev("friday", Some(st), et)).unwrap();
            (e - s).num_minutes()
        };
        assert_eq!(dur("12:00", Some("02:00")), 120); // "12 - 2": PM-less end → 14:00 (2h)
        assert_eq!(dur("18:00", Some("10:00")), 240); // "6pm-10pm": end "10:00" → 22:00 (4h)
        assert_eq!(dur("12:00", Some("14:00")), 120); // clean end stays correct
        assert_eq!(dur("18:00", Some("10pm")), 240); // "10pm" parsed directly
        assert_eq!(dur("12:00", None), 60); // no end → 60min default
    }

    #[test]
    fn finds_deadlines_in_text() {
        let today = NaiveDate::from_ymd_opt(2026, 6, 8).unwrap(); // a Monday
        let thu = NaiveDate::from_ymd_opt(2026, 6, 11).unwrap();
        assert_eq!(find_deadline_dates("review 4 chapters due thursday", today), vec![thu]);
        assert_eq!(find_deadline_dates("finish the slides before friday", today), vec![NaiveDate::from_ymd_opt(2026, 6, 12).unwrap()]);
        assert_eq!(find_deadline_dates("submit by 6/15", today), vec![NaiveDate::from_ymd_opt(2026, 6, 15).unwrap()]);
        assert_eq!(find_deadline_dates("launch in 3 weeks", today), vec![today + Duration::days(21)]);
        assert_eq!(find_deadline_dates("ship within 5 days", today), vec![today + Duration::days(5)]);
        // No deadline keyword / not a date → nothing.
        assert_eq!(find_deadline_dates("write three blog posts", today), Vec::<NaiveDate>::new());
        assert_eq!(find_deadline_dates("meet me by the door", today), Vec::<NaiveDate>::new());
        // Two distinct deadlines are surfaced (caller decides not to guess).
        assert_eq!(find_deadline_dates("X due tuesday and Y due friday", today).len(), 2);
    }

    #[test]
    fn backfill_applies_single_deadline_to_all_tasks() {
        let today = NaiveDate::from_ymd_opt(2026, 6, 8).unwrap();
        let mk = |titles: &[&str]| ParsedPlan {
            projects: vec![ParsedProject {
                name: "Exam".into(),
                tasks: titles.iter().map(|t| ParsedTask { title: (*t).into(), estimated_minutes: 60, priority: "medium".into(), ..Default::default() }).collect(),
            }],
            ..Default::default()
        };

        // One deadline phrase → every task inherits it (end of that day).
        let mut plan = mk(&["Review chapters", "Practice test"]);
        backfill_task_fields(&mut plan, "prep for the exam: review chapters and a practice test, all due friday", today);
        let fri = NaiveDate::from_ymd_opt(2026, 6, 12).unwrap().and_hms_opt(23, 59, 0).unwrap();
        for t in &plan.projects[0].tasks {
            assert_eq!(t.deadline.as_deref().and_then(parse_dt), Some(fri));
        }

        // A deadline the model already set correctly is left untouched.
        let mut plan = mk(&["A", "B"]);
        plan.projects[0].tasks[0].deadline = Some("2026-06-10T17:00:00".into());
        backfill_task_fields(&mut plan, "do these by friday", today);
        assert_eq!(plan.projects[0].tasks[0].deadline.as_deref(), Some("2026-06-10T17:00:00")); // kept
        assert!(plan.projects[0].tasks[1].deadline.is_some()); // filled

        // One deadline per task → paired in reading order (A→Tuesday, B→Friday).
        let mut plan = mk(&["A", "B"]);
        backfill_task_fields(&mut plan, "A due tuesday and B due friday", today);
        let tue = NaiveDate::from_ymd_opt(2026, 6, 9).unwrap().and_hms_opt(23, 59, 0).unwrap();
        let fri = NaiveDate::from_ymd_opt(2026, 6, 12).unwrap().and_hms_opt(23, 59, 0).unwrap();
        assert_eq!(plan.projects[0].tasks[0].deadline.as_deref().and_then(parse_dt), Some(tue));
        assert_eq!(plan.projects[0].tasks[1].deadline.as_deref().and_then(parse_dt), Some(fri));

        // Explicit per-task deadlines still pair even when the model ALSO double-emits a same-named
        // event (which used to disable the backfill via the dropped `task_only` gate).
        let mut plan = mk(&["Finish chapter 1", "Finish chapter 2"]);
        plan.events = vec![{ let mut e = ev("monday", None, None); e.title = "Finish chapter 1".into(); e }];
        backfill_task_fields(&mut plan, "finish chapter 1 by monday and chapter 2 by wednesday", today);
        let mon = NaiveDate::from_ymd_opt(2026, 6, 8).unwrap().and_hms_opt(23, 59, 0).unwrap();
        let wed = NaiveDate::from_ymd_opt(2026, 6, 10).unwrap().and_hms_opt(23, 59, 0).unwrap();
        assert_eq!(plan.projects[0].tasks[0].deadline.as_deref().and_then(parse_dt), Some(mon));
        assert_eq!(plan.projects[0].tasks[1].deadline.as_deref().and_then(parse_dt), Some(wed));

        // Deadline count ≠ task count → don't guess the pairing (leave them).
        let mut plan = mk(&["A", "B", "C"]);
        backfill_task_fields(&mut plan, "A due tuesday and B due friday", today);
        assert!(plan.projects[0].tasks.iter().all(|t| t.deadline.is_none()));

        // Keyword-less but task-only with a lone day word → that day is the deadline.
        let mut plan = mk(&["Review chapters", "Practice test"]);
        backfill_task_fields(&mut plan, "prep for my exam friday: review chapters and a practice test", today);
        let fri = NaiveDate::from_ymd_opt(2026, 6, 12).unwrap().and_hms_opt(23, 59, 0).unwrap();
        assert!(plan.projects[0].tasks.iter().all(|t| t.deadline.as_deref().and_then(parse_dt) == Some(fri)));

        // …but a day word alongside an EVENT is not a task deadline (it's the event's day).
        let mut plan = mk(&["Finish slides"]);
        plan.events.push(ev("friday", Some("14:00"), Some("15:00")));
        backfill_task_fields(&mut plan, "dentist friday at 2pm and finish the slides", today);
        assert!(plan.projects[0].tasks[0].deadline.is_none());
    }

    #[test]
    fn backfill_sets_single_task_estimate_from_text() {
        let today = NaiveDate::from_ymd_opt(2026, 6, 8).unwrap();
        let one = || ParsedPlan {
            projects: vec![ParsedProject {
                name: "Study".into(),
                tasks: vec![ParsedTask { title: "Study for exam".into(), estimated_minutes: 60, priority: "high".into(), ..Default::default() }],
            }],
            ..Default::default()
        };

        // Lone task + explicit length → trust the user's number over the model's 60-min default.
        let mut plan = one();
        backfill_task_fields(&mut plan, "study for the exam, about 3 hours", today);
        assert_eq!(plan.projects[0].tasks[0].estimated_minutes, 180);

        // Two tasks → ambiguous, don't reassign a single duration.
        let mut plan = ParsedPlan {
            projects: vec![ParsedProject {
                name: "P".into(),
                tasks: vec![
                    ParsedTask { title: "A".into(), estimated_minutes: 60, priority: "medium".into(), ..Default::default() },
                    ParsedTask { title: "B".into(), estimated_minutes: 60, priority: "medium".into(), ..Default::default() },
                ],
            }],
            ..Default::default()
        };
        backfill_task_fields(&mut plan, "spend 2 hours total on these", today);
        assert!(plan.projects[0].tasks.iter().all(|t| t.estimated_minutes == 60));

        // A competing event in the same message → leave the task estimate alone.
        let mut plan = one();
        plan.events.push(ev("today", Some("15:00"), Some("16:00")));
        backfill_task_fields(&mut plan, "meeting 3-4pm and study for 2 hours", today);
        assert_eq!(plan.projects[0].tasks[0].estimated_minutes, 60);
    }

    #[test]
    fn finds_durations_in_text() {
        assert_eq!(find_duration_minutes("make the meeting 2 hours instead of 1"), Some(120));
        assert_eq!(find_duration_minutes("shorten it to 30 min"), Some(30));
        assert_eq!(find_duration_minutes("a 1.5 hour block"), Some(90));
        assert_eq!(find_duration_minutes("2hrs please"), Some(120));
        assert_eq!(find_duration_minutes("45 minutes"), Some(45));
        // bare numbers without a unit are not durations
        assert_eq!(find_duration_minutes("instead of 1"), None);
        assert_eq!(find_duration_minutes("move it to friday"), None);
        // a clock time is not a duration
        assert_eq!(find_duration_minutes("at 2pm"), None);
    }

    #[test]
    fn duration_edit_resolves_in_one_turn() {
        // The reported loop: "change the meeting today to be 2 hours instead of 1".
        // The model routes it as an update but can't express the new length; the backfill
        // recovers 120 min, and merge_event keeps the existing 13:00 start.
        let now = NaiveDate::from_ymd_opt(2026, 6, 4).unwrap().and_hms_opt(9, 0, 0).unwrap();
        let meeting = crate::model::Event {
            id: 7,
            title: "Meeting with my friend".into(),
            start: "2026-06-04T13:00:00".into(),
            end: "2026-06-04T14:00:00".into(), // currently 1h
            kind: "fixed".into(),
            source: "manual".into(),
            created_at: String::new(),
            provider: None,
            external_id: None,
            account_id: None,
            etag: None,
        };

        let mut plan = ParsedPlan {
            update_events: vec![UpdateEvent {
                target: "Meeting".into(),
                title: None,
                day: None,
                start_time: None,
                end_time: None,
                duration_minutes: None,
                date: None,
                span_days: None,
                shift_minutes: None,
            }],
            ..Default::default()
        };
        backfill_event_fields(&mut plan, "Change the meeting I have today to be 2 hours instead of 1", NaiveDate::from_ymd_opt(2026, 6, 8).unwrap());
        assert_eq!(plan.update_events[0].duration_minutes, Some(120));

        let up = &plan.update_events[0];
        let (_t, s, en) = merge_event(&meeting, now, up.day.as_deref(), up.date.as_deref(), up.start_time.as_deref(), up.end_time.as_deref(), up.duration_minutes, up.span_days, up.title.as_deref());
        assert_eq!(s, "2026-06-04T13:00:00"); // start preserved
        assert_eq!((parse_dt(&en).unwrap() - parse_dt(&s).unwrap()).num_minutes(), 120); // now 2h
    }

    #[test]
    fn clarification_loop_is_broken() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let updated = s(&["Meeting with my friend"]);

        let nh: &[String] = &[];

        // The exact loop from the screenshots: we updated the meeting but the model still
        // asks for the start/end time. Those questions must be dropped.
        let out = filter_clarifications(&s(&["What is the start time of the updated meeting?"]), &[], &[], &updated, &[], nh);
        assert!(out.is_empty(), "redundant time question should be dropped, got {out:?}");

        let out = filter_clarifications(&s(&["What is the new end time for the meeting?"]), &[], &[], &updated, &[], nh);
        assert!(out.is_empty());

        // A question naming a different, untouched event still gets through.
        let out = filter_clarifications(&s(&["What time is the dentist appointment?"]), &[], &[], &updated, &[], nh);
        assert_eq!(out.len(), 1);

        // Our own "couldn't place it" question survives even alongside an edit.
        let out = filter_clarifications(&s(&[]), &s(&["What date and time is \"Yoga\"?"]), &[], &updated, &[], nh);
        assert_eq!(out.len(), 1);

        // Non-questions (restatements) are dropped.
        let out = filter_clarifications(&s(&["Updated the meeting."]), &[], &[], &updated, &[], nh);
        assert!(out.is_empty());
    }

    #[test]
    fn one_day_word_covers_every_event() {
        // "birthday lunch tomorrow from 12-2 and a graduation party from 6-10 as well":
        // the model wrongly put one event on "today" and left the other's day blank. Since
        // the user named exactly one day, both must land on tomorrow.
        let now = NaiveDate::from_ymd_opt(2026, 6, 4).unwrap().and_hms_opt(9, 0, 0).unwrap();
        let mut lunch = ev("today", Some("12:00"), Some("14:00")); // model's wrong guess
        lunch.title = "Birthday lunch".into();
        let mut party = ev("today", Some("18:00"), Some("22:00"));
        party.title = "Graduation party".into();
        party.day = None; // model left it blank
        let mut plan = ParsedPlan { events: vec![lunch, party], ..Default::default() };

        backfill_event_fields(&mut plan, "I have my birthday lunch tomorrow from 12 - 2 and a graduation party from 6 - 10 as well", NaiveDate::from_ymd_opt(2026, 6, 8).unwrap());

        let tomorrow = NaiveDate::from_ymd_opt(2026, 6, 5).unwrap();
        for e in &plan.events {
            assert_eq!(e.day.as_deref(), Some("tomorrow"));
            assert_eq!(resolve_event(now, e).unwrap().0.date(), tomorrow);
        }

        // Two distinct events on two days → leave each alone (NOT a single multi-day span).
        let mut lunch = ev("today", Some("12:00"), None);
        lunch.title = "Lunch".into();
        let mut dinner = ev("tomorrow", Some("18:00"), None);
        dinner.title = "Dinner".into();
        let mut plan = ParsedPlan { events: vec![lunch, dinner], ..Default::default() };
        backfill_event_fields(&mut plan, "lunch today and dinner tomorrow", NaiveDate::from_ymd_opt(2026, 6, 8).unwrap());
        assert_eq!(plan.events.len(), 2);
        assert_eq!(plan.events[0].day.as_deref(), Some("today"));
        assert_eq!(plan.events[1].day.as_deref(), Some("tomorrow"));
        assert_eq!(plan.events[0].span_days, None); // not collapsed into a span
    }

    #[test]
    fn day_confirmations_and_chatter_dropped() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let created = s(&["Birthday lunch", "Graduation party"]);
        let nh: &[String] = &[];

        // Rust already resolved "tomorrow" and placed the events — confirming is noise.
        let out = filter_clarifications(&s(&["Is 'tomorrow' the day after today?"]), &[], &created, &[], &[], nh);
        assert!(out.is_empty(), "got {out:?}");
        let out = filter_clarifications(&s(&["Is 'tomorrow' referring to June 5th?"]), &[], &created, &[], &[], nh);
        assert!(out.is_empty());

        // Generic "anything else" filler is dropped.
        let out = filter_clarifications(&s(&["Is there anything else you need me to add or change?"]), &[], &created, &[], &[], nh);
        assert!(out.is_empty());

        // The reported case: event placed, but the model still asks for its duration.
        let out = filter_clarifications(&s(&["What is the duration in minutes for this event?"]), &[], &created, &[], &[], nh);
        assert!(out.is_empty(), "duration question after placing is noise, got {out:?}");

        // But when nothing was placed, a genuine day/time question still comes through.
        let out = filter_clarifications(&s(&["What time on tuesday?"]), &[], &[], &[], &[], nh);
        assert_eq!(out.len(), 1, "no event placed → keep the question, got {out:?}");
    }

    #[test]
    fn recurrence_detects_cadence_and_synthesizes() {
        // Cadence detection.
        assert_eq!(recurrence("go to the gym every monday and wednesday"), Some(("weekly".into(), vec![1, 3], 1)));
        assert_eq!(recurrence("study every other day for 2 hours"), Some(("interval".into(), vec![], 2)));
        assert_eq!(recurrence("water the plants every 3 days"), Some(("interval".into(), vec![], 3)));
        assert_eq!(recurrence("stand-up every weekday at 9"), Some(("weekly".into(), vec![1, 2, 3, 4, 5], 1)));
        assert_eq!(recurrence("relax on weekends"), Some(("weekly".into(), vec![6, 7], 1)));
        assert_eq!(recurrence("exercise daily"), Some(("daily".into(), vec![], 1)));
        assert_eq!(recurrence("lunch on friday"), None); // one-off, not recurring
        // "except" inverts the named days against the full week.
        assert_eq!(recurrence("workout every day except sunday at 8pm"), Some(("weekly".into(), vec![1, 2, 3, 4, 5, 6], 1)));
        assert_eq!(recurrence("gym every day but not saturday and sunday"), Some(("weekly".into(), vec![1, 2, 3, 4, 5], 1)));

        // Synthesis strips lead-ins, recurrence words, and weekdays down to the activity.
        assert_eq!(synthesize_habit_name("Go to the gym every Monday and Wednesday").as_deref(), Some("Gym"));
        assert_eq!(synthesize_habit_name("meditate every other day").as_deref(), Some("Meditate"));

        // End to end: an empty plan + weekly recurrence → a synthesized weekly habit.
        let mut plan = ParsedPlan::default();
        route_recurring_to_habits(&mut plan, "Go to the gym every Monday and Wednesday.");
        assert_eq!(plan.habits.len(), 1);
        assert_eq!(plan.habits[0].name, "Gym");
        assert_eq!(plan.habits[0].cadence, "weekly");
        assert_eq!(plan.habits[0].days, vec![1, 3]);
    }

    #[test]
    fn sequential_then_chains_task_dependencies() {
        let mk = |t: &str| ParsedTask { title: t.into(), ..Default::default() };
        let mut plan = ParsedPlan {
            projects: vec![ParsedProject { name: "Launch".into(), tasks: vec![mk("Fix the login bug"), mk("Write tests"), mk("Deploy to prod")] }],
            ..Default::default()
        };
        backfill_task_dependencies(&mut plan, "fix the login bug, then write tests, then deploy to prod");
        let t = &plan.projects[0].tasks;
        assert!(t[0].depends_on.is_empty(), "first step has no prerequisite");
        assert_eq!(t[1].depends_on, vec!["Fix the login bug".to_string()]);
        assert_eq!(t[2].depends_on, vec!["Write tests".to_string()]);

        // No sequencing cue → unordered list, no chaining.
        let mut p2 = ParsedPlan {
            projects: vec![ParsedProject { name: "Errands".into(), tasks: vec![mk("Buy milk"), mk("Buy eggs")] }],
            ..Default::default()
        };
        backfill_task_dependencies(&mut p2, "buy milk and eggs");
        assert!(p2.projects[0].tasks[1].depends_on.is_empty());
    }

    #[test]
    fn double_emitted_recurring_routine_collapses_to_one_habit() {
        // The model emits the SAME routine as an event + a same-named update + a task. It must
        // collapse into ONE habit, keeping the stated duration and the detected interval cadence.
        let mut e = ev("today", Some("09:00"), None);
        e.title = "Study US History".into();
        let up = UpdateEvent {
            target: "Study US History".into(),
            title: None,
            day: None,
            start_time: None,
            end_time: None,
            duration_minutes: None,
            date: None,
            span_days: None,
            shift_minutes: None,
        };
        let mut plan = ParsedPlan {
            events: vec![e],
            update_events: vec![up],
            projects: vec![ParsedProject {
                name: "Study".into(),
                tasks: vec![ParsedTask { title: "Study US History".into(), estimated_minutes: 60, priority: "medium".into(), ..Default::default() }],
            }],
            ..Default::default()
        };
        route_recurring_to_habits(&mut plan, "I will study every other day this week for two hours at 9 am for US history.");
        assert_eq!(plan.habits.len(), 1, "one habit for the single routine");
        assert!(plan.habits[0].name.to_lowercase().contains("histor"));
        assert_eq!(plan.habits[0].duration_minutes, 120, "two hours preserved");
        assert_eq!(plan.habits[0].cadence, "interval");
        assert_eq!(plan.habits[0].interval_days, 2);
        assert!(plan.events.is_empty() && plan.update_events.is_empty(), "duplicates suppressed");
        assert_eq!(plan.projects.iter().map(|p| p.tasks.len()).sum::<usize>(), 0);
    }

    #[test]
    fn recurring_routines_become_habits() {
        // The model routed "every day" as a single fixed event → convert to a habit.
        let mut ev1 = ev("today", Some("16:00"), Some("17:00"));
        ev1.title = "Violin practice".into();
        let mut plan = ParsedPlan { events: vec![ev1], ..Default::default() };
        route_recurring_to_habits(&mut plan, "i want to practice violin every day from 4pm to 5pm");
        assert!(plan.events.is_empty(), "the event should be moved out");
        assert_eq!(plan.habits.len(), 1);
        assert_eq!(plan.habits[0].name, "Violin practice");
        assert_eq!(plan.habits[0].duration_minutes, 60); // from the 4–5pm range

        // A single recurring task is converted too, using its estimate when no range is given.
        let mut plan = ParsedPlan {
            projects: vec![ParsedProject {
                name: "Fitness".into(),
                tasks: vec![ParsedTask { title: "Exercise".into(), estimated_minutes: 45, priority: "high".into(), ..Default::default() }],
            }],
            ..Default::default()
        };
        route_recurring_to_habits(&mut plan, "exercise daily");
        assert_eq!(plan.habits.len(), 1);
        assert_eq!(plan.habits[0].name, "Exercise");
        assert_eq!(plan.habits[0].duration_minutes, 45);
        assert_eq!(plan.projects.iter().map(|p| p.tasks.len()).sum::<usize>(), 0);

        // No recurrence language → leave events/tasks alone.
        let mut plan = ParsedPlan { events: vec![ev("today", Some("16:00"), Some("17:00"))], ..Default::default() };
        route_recurring_to_habits(&mut plan, "violin at 4pm today");
        assert_eq!(plan.events.len(), 1);
        assert!(plan.habits.is_empty());

        // Recurrence questions are dropped once we've made a habit.
        let habits = vec!["Violin practice".to_string()];
        let out = filter_clarifications(&["did you mean 'every weekday'?".into()], &[], &[], &[], &[], &habits);
        assert!(out.is_empty());
    }

    #[test]
    fn recurrence_suppresses_the_duplicate_event_beside_the_habit() {
        // The model double-emits: a "Violin practice" habit AND a "Practice violin" event/update
        // for the same daily routine. The event/update (reordered words) must be suppressed.
        let mut ev1 = ev("today", Some("16:00"), Some("17:00"));
        ev1.title = "Practice violin".into();
        let mut plan = ParsedPlan {
            events: vec![ev1],
            update_events: vec![UpdateEvent {
                target: "Practice violin".into(),
                title: None,
                day: None,
                start_time: None,
                end_time: None,
                duration_minutes: None,
                date: None,
                span_days: None,
                shift_minutes: None,
            }],
            habits: vec![ParsedHabit { name: "Violin practice".into(), duration_minutes: 60, ..Default::default() }],
            ..Default::default()
        };
        route_recurring_to_habits(&mut plan, "i want to practice violin every day from 4 to 5pm");
        assert!(plan.events.is_empty(), "duplicate event should be suppressed");
        assert!(plan.update_events.is_empty(), "duplicate update should be suppressed");
        assert_eq!(plan.habits.len(), 1);
        assert_eq!(plan.habits[0].name, "Violin practice");
    }

    #[test]
    fn recurrence_synthesizes_a_habit_when_the_model_emits_nothing() {
        // "Exercise daily for 30 minutes" — the model returned only a "what time?" clarification.
        // We synthesize the habit from the text rather than dropping the routine.
        let mut plan = ParsedPlan::default();
        route_recurring_to_habits(&mut plan, "Exercise daily for 30 minutes");
        assert_eq!(plan.habits.len(), 1);
        assert_eq!(plan.habits[0].name, "Exercise");
        assert_eq!(plan.habits[0].duration_minutes, 30);

        // But genuinely empty/ambiguous recurrence doesn't invent a habit.
        let mut plan = ParsedPlan::default();
        route_recurring_to_habits(&mut plan, "every day");
        assert!(plan.habits.is_empty());
    }

    #[test]
    fn timed_event_without_a_day_is_not_dropped() {
        let now = NaiveDate::from_ymd_opt(2026, 6, 4).unwrap().and_hms_opt(9, 0, 0).unwrap();

        // "graduation party from 7-8" with no day → placed on today, never lost.
        let mut e = ev("today", Some("19:00"), Some("20:00"));
        e.day = None;
        let (st, en) = resolve_event(now, &e).expect("a timed event must never be dropped");
        assert_eq!(st.date(), now.date());
        assert_eq!((en - st).num_minutes(), 60);

        // A duration alone is enough of a time signal to place it.
        let mut dur_only = ev("today", None, None);
        dur_only.day = None;
        dur_only.duration_minutes = Some(90);
        let (st, en) = resolve_event(now, &dur_only).unwrap();
        assert_eq!(st.date(), now.date());
        assert_eq!((en - st).num_minutes(), 90);

        // No day AND no time → genuinely ambiguous, so we still ask (None).
        let mut bare = ev("today", None, None);
        bare.day = None;
        assert!(resolve_event(now, &bare).is_none());
    }

    #[test]
    fn absurd_duration_is_clamped() {
        let now = NaiveDate::from_ymd_opt(2026, 6, 4).unwrap().and_hms_opt(9, 0, 0).unwrap();
        let dur = |mins: i64| {
            let mut e = ev("today", Some("12:00"), None);
            e.duration_minutes = Some(mins);
            let (s, en) = resolve_event(now, &e).unwrap();
            (en - s).num_minutes()
        };
        assert_eq!(dur(90), 90); // sane → honored
        assert_eq!(dur(24 * 60), 24 * 60); // a full day is allowed
        assert_eq!(dur(100_000), 60); // runaway → ignored, 60-min fallback
        assert_eq!(dur(-30), 60); // negative → ignored
    }

    #[test]
    fn parses_multi_day_spans_and_dates() {
        assert_eq!(find_span_days("staying in vietnam for two weeks"), Some(14));
        assert_eq!(find_span_days("a 5 day trip"), Some(5));
        assert_eq!(find_span_days("here for a week"), Some(7));
        assert_eq!(find_span_days("lunch for an hour"), None); // hours aren't a day-span

        let today = NaiveDate::from_ymd_opt(2026, 6, 5).unwrap();
        assert_eq!(find_explicit_date("on 6/12 i go to vietnam", today), NaiveDate::from_ymd_opt(2026, 6, 12));
        assert_eq!(find_explicit_date("trip on 12/25/2026", today), NaiveDate::from_ymd_opt(2026, 12, 25));
        // a bare date already well past rolls to next year (a future trip)
        assert_eq!(find_explicit_date("party on 1/3", today), NaiveDate::from_ymd_opt(2027, 1, 3));
        assert_eq!(find_explicit_date("no date here", today), None);
    }

    #[test]
    fn parses_relative_time_shift() {
        assert_eq!(find_time_shift("push my dentist appointment back an hour"), Some(60));
        assert_eq!(find_time_shift("move it up 30 minutes"), Some(-30));
        assert_eq!(find_time_shift("can you push the meeting back half an hour"), Some(30));
        assert_eq!(find_time_shift("postpone it by 2 hours"), Some(120));
        assert_eq!(find_time_shift("make it earlier by 15 min"), Some(-15));
        // no direction, or no amount → not a shift
        assert_eq!(find_time_shift("move it to 3pm"), None);
        assert_eq!(find_time_shift("push it back"), None);

        // End to end: backfill stamps the shift; store_plan applies it off the existing start.
        let now = NaiveDate::from_ymd_opt(2026, 6, 8).unwrap().and_hms_opt(9, 0, 0).unwrap();
        let mut plan = ParsedPlan {
            update_events: vec![UpdateEvent {
                target: "Dentist".into(),
                title: None,
                day: None,
                start_time: None,
                end_time: None,
                duration_minutes: None,
                date: None,
                span_days: None,
                shift_minutes: None,
            }],
            ..Default::default()
        };
        backfill_event_fields(&mut plan, "push my dentist appointment back an hour", NaiveDate::from_ymd_opt(2026, 6, 8).unwrap());
        assert_eq!(plan.update_events[0].shift_minutes, Some(60));
        // Apply against a 14:00 event → 15:00, duration preserved.
        let dentist = crate::model::Event {
            id: 1,
            title: "Dentist".into(),
            start: "2026-06-09T14:00:00".into(),
            end: "2026-06-09T15:00:00".into(),
            kind: "fixed".into(),
            source: "manual".into(),
            created_at: String::new(),
            provider: None,
            external_id: None,
            account_id: None,
            etag: None,
        };
        let up = &plan.update_events[0];
        let shifted = up.shift_minutes.and_then(|d| parse_dt(&dentist.start).map(|s| (s + Duration::minutes(d)).format("%H:%M").to_string()));
        let (_t, s, en) = merge_event(&dentist, now, None, None, shifted.as_deref(), None, None, None, None);
        assert_eq!(s, "2026-06-09T15:00:00");
        assert_eq!((parse_dt(&en).unwrap() - parse_dt(&s).unwrap()).num_minutes(), 60);
    }

    #[test]
    fn parses_relative_date() {
        let today = NaiveDate::from_ymd_opt(2026, 6, 5).unwrap();
        assert_eq!(find_relative_date("lunch the day after tomorrow at 1pm", today), NaiveDate::from_ymd_opt(2026, 6, 7));
        assert_eq!(find_relative_date("review in two weeks at 2pm", today), NaiveDate::from_ymd_opt(2026, 6, 19));
        assert_eq!(find_relative_date("call in 3 days", today), NaiveDate::from_ymd_opt(2026, 6, 8));
        assert_eq!(find_relative_date("ship within a week", today), NaiveDate::from_ymd_opt(2026, 6, 12));
        // not a date: durations, "in N hours", a trip span, plain "in vietnam"
        assert_eq!(find_relative_date("in 2 hours", today), None);
        assert_eq!(find_relative_date("staying in vietnam for two weeks", today), None);

        // End to end: a relative-dated event lands on the right day, and the span is NOT applied
        // (it stays a point event at its time, not a 14-day all-day block).
        let now = today.and_hms_opt(9, 0, 0).unwrap();
        let mut e = ev("today", Some("14:00"), None);
        e.title = "Project review".into();
        e.day = None;
        let mut plan = ParsedPlan { events: vec![e], ..Default::default() };
        backfill_event_fields(&mut plan, "schedule a project review in two weeks at 2pm", today);
        assert_eq!(plan.events[0].span_days, None, "an 'in two weeks' date must not become a span");
        assert_eq!(plan.events[0].date.as_deref(), Some("2026-06-19"));
        let (s, en) = resolve_event(now, &plan.events[0]).unwrap();
        assert_eq!(s.date(), NaiveDate::from_ymd_opt(2026, 6, 19).unwrap());
        assert_eq!((en - s).num_minutes(), 60); // a normal-length event, not all-day
    }

    #[test]
    fn parses_ordinal_day_of_month() {
        let today = NaiveDate::from_ymd_opt(2026, 6, 5).unwrap();
        // "the 25th" is later this month.
        assert_eq!(find_day_of_month("renew my passport on the 25th at 10am", today), NaiveDate::from_ymd_opt(2026, 6, 25));
        // "the 3rd" already passed this month → next month.
        assert_eq!(find_day_of_month("dentist on the 3rd", today), NaiveDate::from_ymd_opt(2026, 7, 3));
        // today's own ordinal resolves to today.
        assert_eq!(find_day_of_month("the 5th", today), NaiveDate::from_ymd_opt(2026, 6, 5));
        // requires a preceding "the" — rank words are not dates.
        assert_eq!(find_day_of_month("my 1st meeting tomorrow", today), None);
        assert_eq!(find_day_of_month("no ordinal here", today), None);
        // the 31st skips 30-day months (June, Sept) to the next month that has it.
        assert_eq!(find_day_of_month("the 31st", today), NaiveDate::from_ymd_opt(2026, 7, 31));
        // folds into the event date path: an ordinal-dated event lands on that day.
        let now = today.and_hms_opt(9, 0, 0).unwrap();
        let mut e = ev("today", Some("10:00"), None);
        e.title = "Renew passport".into();
        e.day = None;
        let mut plan = ParsedPlan { events: vec![e], ..Default::default() };
        backfill_event_fields(&mut plan, "renew my passport on the 25th at 10am", today);
        assert_eq!(plan.events[0].date.as_deref(), Some("2026-06-25"));
        assert_eq!(resolve_event(now, &plan.events[0]).unwrap().0.date(), NaiveDate::from_ymd_opt(2026, 6, 25).unwrap());
    }

    #[test]
    fn vietnam_trip_becomes_an_all_day_multi_day_event() {
        // "on 6/12 ... staying for two weeks" → all-day event 6/12 → 6/26, not a 24h block.
        let now = NaiveDate::from_ymd_opt(2026, 6, 5).unwrap().and_hms_opt(9, 0, 0).unwrap();
        let mut trip = ev("today", None, None);
        trip.title = "Vietnam Trip".into();
        trip.day = None;
        let mut plan = ParsedPlan { events: vec![trip], ..Default::default() };
        backfill_event_fields(&mut plan, "from 6/12 i will be staying in vietnam for two weeks", NaiveDate::from_ymd_opt(2026, 6, 8).unwrap());

        assert_eq!(plan.events[0].date.as_deref(), Some("2026-06-12"));
        assert_eq!(plan.events[0].span_days, Some(14));
        let (s, e) = resolve_event(now, &plan.events[0]).unwrap();
        assert_eq!(s, NaiveDate::from_ymd_opt(2026, 6, 12).unwrap().and_hms_opt(0, 0, 0).unwrap());
        assert_eq!(e, NaiveDate::from_ymd_opt(2026, 6, 26).unwrap().and_hms_opt(0, 0, 0).unwrap());
        assert_eq!((e - s).num_days(), 14);
    }

    #[test]
    fn named_weekday_range_is_a_multi_day_span() {
        let mon = NaiveDate::from_ymd_opt(2026, 6, 8).unwrap(); // a Monday
        // "wednesday and thursday" → Wed 6/10 .. Thu 6/11 (2 days)
        assert_eq!(find_weekday_span("orientation wednesday and thursday", mon), Some((NaiveDate::from_ymd_opt(2026, 6, 10).unwrap(), 2)));
        // "monday to friday" → 5 days
        assert_eq!(find_weekday_span("class monday to friday", mon), Some((NaiveDate::from_ymd_opt(2026, 6, 8).unwrap(), 5)));
        // a single day is not a span
        assert_eq!(find_weekday_span("lunch on friday", mon), None);

        // End to end: the reported A&M case — one event spanning Wed+Thu becomes all-day.
        let mut e = ev("wednesday", None, None);
        e.title = "A&M Orientation".into();
        let mut plan = ParsedPlan { events: vec![e], ..Default::default() };
        backfill_event_fields(&mut plan, "im going to A&M orientation wednesday and thursday this week", mon);
        assert_eq!(plan.events[0].date.as_deref(), Some("2026-06-10"));
        assert_eq!(plan.events[0].span_days, Some(2));
        let (s, en) = resolve_event(mon.and_hms_opt(9, 0, 0).unwrap(), &plan.events[0]).unwrap();
        assert_eq!(s, NaiveDate::from_ymd_opt(2026, 6, 10).unwrap().and_hms_opt(0, 0, 0).unwrap());
        assert_eq!(en, NaiveDate::from_ymd_opt(2026, 6, 12).unwrap().and_hms_opt(0, 0, 0).unwrap()); // covers Wed+Thu
    }

    #[test]
    fn explicit_date_trip_collapses_create_plus_self_updates() {
        // The reported live failure: "from 6/12 … for two weeks" → the model emitted one create
        // plus several self-updates of the same event, defeating the single-event span logic.
        let now = NaiveDate::from_ymd_opt(2026, 6, 5).unwrap().and_hms_opt(9, 0, 0).unwrap();
        let mut trip = ev("today", None, None);
        trip.title = "Stay in Vietnam".into();
        trip.day = None;
        let upd = |t: &str| UpdateEvent {
            target: t.into(),
            title: None,
            day: None,
            start_time: None,
            end_time: None,
            duration_minutes: None,
            date: None,
            span_days: None,
            shift_minutes: None,
        };
        let mut plan = ParsedPlan {
            events: vec![trip],
            update_events: vec![upd("Stay in Vietnam"), upd("Stay in Vietnam"), upd("Stay in Vietnam")],
            ..Default::default()
        };
        backfill_event_fields(&mut plan, "from 6/12 i will be staying in vietnam for two weeks", NaiveDate::from_ymd_opt(2026, 6, 8).unwrap());

        assert_eq!(plan.events.len(), 1);
        assert!(plan.update_events.is_empty(), "redundant self-updates should be dropped");
        assert_eq!(plan.events[0].span_days, Some(14));
        assert_eq!(plan.events[0].date.as_deref(), Some("2026-06-12"));
        let (s, e) = resolve_event(now, &plan.events[0]).unwrap();
        assert_eq!((e - s).num_days(), 14); // all-day, two weeks
    }

    #[test]
    fn named_day_range_collapses_create_plus_update() {
        // The model split "wednesday to thursday" into a create (Wed) + a same-event update
        // (Thu). They must collapse into ONE multi-day event, dropping the redundant update.
        let mon = NaiveDate::from_ymd_opt(2026, 6, 8).unwrap();
        let mut create = ev("wednesday", None, None);
        create.title = "A&M orientation".into();
        let update = UpdateEvent {
            target: "A&M orientation".into(),
            title: None,
            day: Some("thursday".into()),
            start_time: None,
            end_time: None,
            duration_minutes: None,
            date: None,
            span_days: None,
            shift_minutes: None,
        };
        let mut plan = ParsedPlan { events: vec![create], update_events: vec![update], ..Default::default() };
        backfill_event_fields(&mut plan, "i have a&m orientation from wednesday to thursday this week", mon);
        assert_eq!(plan.events.len(), 1);
        assert!(plan.update_events.is_empty(), "redundant per-day update should be dropped");
        assert_eq!(plan.events[0].date.as_deref(), Some("2026-06-10"));
        assert_eq!(plan.events[0].span_days, Some(2));
    }

    #[test]
    fn full_day_marks_events_all_day() {
        assert!(find_all_day("they are full day"));
        assert!(find_all_day("make it an all-day thing"));
        assert!(!find_all_day("at 3pm"));

        // "They are full day" referring to two events → both become all-day (span 1).
        let up = |target: &str| UpdateEvent {
            target: target.into(),
            title: None,
            day: None,
            start_time: None,
            end_time: None,
            duration_minutes: None,
            date: None,
            span_days: None,
            shift_minutes: None,
        };
        let mut plan = ParsedPlan { update_events: vec![up("A&M - Wednesday"), up("A&M - Thursday")], ..Default::default() };
        backfill_event_fields(&mut plan, "they are full day", NaiveDate::from_ymd_opt(2026, 6, 8).unwrap());
        assert_eq!(plan.update_events[0].span_days, Some(1));
        assert_eq!(plan.update_events[1].span_days, Some(1));
    }

    #[test]
    fn unescapes_html_entities_in_titles() {
        assert_eq!(unescape_html("A&amp;M Orientation"), "A&M Orientation");
        assert_eq!(unescape_html("Tom &amp; Jerry &lt;3"), "Tom & Jerry <3");
        let mut e = ev("friday", Some("12:00"), None);
        e.title = "A&amp;M Orientation".into();
        let mut plan = ParsedPlan { events: vec![e], ..Default::default() };
        unescape_plan(&mut plan);
        assert_eq!(plan.events[0].title, "A&M Orientation");
    }

    #[test]
    fn find_day_phrases_dedupes_and_ignores_substrings() {
        assert_eq!(find_day_phrases("lunch tomorrow and a party as well"), vec!["tomorrow"]);
        assert_eq!(find_day_phrases("lunch today and dinner tomorrow"), vec!["today", "tomorrow"]);
        assert_eq!(find_day_phrases("move it on friday, friday works"), vec!["friday"]);
        assert!(find_day_phrases("make it 2 hours").is_empty());
    }
}
