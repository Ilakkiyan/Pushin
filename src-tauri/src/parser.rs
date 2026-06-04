//! Natural-language → structured plan. The small LLM extracts events/tasks and a
//! **day phrase + time** (which it does well); Rust computes the actual calendar date
//! (which the model does badly). Dates are never trusted from the model.

use crate::llm;
use crate::model::{Event, Settings};
use crate::scheduler::{fmt_dt, parse_dt};
use anyhow::Result;
use chrono::{Datelike, Duration, Local, NaiveDate, NaiveDateTime, NaiveTime, Weekday};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;

/// One prior chat turn, passed in for conversational context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatTurn {
    pub role: String, // "user" | "assistant"
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedProject {
    pub name: String,
    #[serde(default)]
    pub tasks: Vec<ParsedTask>,
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
    pub clarifications: Vec<String>,
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
                        "title": { "type": "string", "maxLength": 100 },
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
                        "match": { "type": "string", "maxLength": 100 },
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
                "items": { "type": "string", "maxLength": 100 }
            },
            "projects": {
                "type": "array",
                "maxItems": 6,
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "name": { "type": "string", "maxLength": 80 },
                        "tasks": {
                            "type": "array",
                            "maxItems": 25,
                            "items": {
                                "type": "object",
                                "additionalProperties": false,
                                "properties": {
                                    "title": { "type": "string", "maxLength": 100 },
                                    "estimated_minutes": { "type": "integer" },
                                    "deadline": { "type": ["string", "null"] },
                                    "priority": { "type": "string", "enum": ["low", "medium", "high", "urgent"] },
                                    "depends_on": { "type": "array", "maxItems": 12, "items": { "type": "string", "maxLength": 100 } },
                                    "chunkable": { "type": "boolean" }
                                },
                                "required": ["title", "estimated_minutes", "priority"]
                            }
                        }
                    },
                    "required": ["name", "tasks"]
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

fn system_prompt(events: &[Event]) -> String {
    let now = Local::now().naive_local();

    // Show the model what's already on the calendar — with each event's day, time range,
    // and length — so it can recognize an event and change/remove it without re-asking.
    let calendar = if events.is_empty() {
        "(the calendar is currently empty)".to_string()
    } else {
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
Events already on the calendar (reference these to change or remove them):\n\
{calendar}\n\
Examples:\n\
user: lunch with mom friday 12-2 → {{\"events\":[{{\"title\":\"Lunch with mom\",\"day\":\"friday\",\"startTime\":\"12:00\",\"endTime\":\"14:00\"}}]}}\n\
user: remove all sleepovers → {{\"removeEvents\":[\"sleepover\"]}}\n\
user: make the sleepover 8pm to 8am → {{\"updateEvents\":[{{\"match\":\"sleepover\",\"startTime\":\"20:00\",\"endTime\":\"08:00\"}}]}}\n\
user: make the meeting today 2 hours instead of 1 → {{\"updateEvents\":[{{\"match\":\"Meeting\",\"durationMinutes\":120}}]}}\n\
user: plan a blog - pick platform, write posts → {{\"projects\":[{{\"name\":\"Blog\",\"tasks\":[{{\"title\":\"Pick platform\",\"estimated_minutes\":60,\"priority\":\"high\"}}]}}]}}",
        now = now.format("%Y-%m-%d %H:%M"),
        weekday = now.format("%A"),
        calendar = calendar,
    )
}

/// Call the model with conversation context. Resolves task deadlines (event dates are
/// resolved later, in `store_plan`). Holds no DB lock.
pub async fn plan(
    client: &reqwest::Client,
    settings: &Settings,
    current_events: &[Event],
    history: &[ChatTurn],
    user_text: &str,
) -> Result<ParsedPlan> {
    let mut messages: Vec<Value> = vec![json!({ "role": "system", "content": system_prompt(current_events) })];
    for turn in history.iter().rev().take(6).rev() {
        let role = if turn.role == "assistant" { "assistant" } else { "user" };
        messages.push(json!({ "role": role, "content": turn.content }));
    }
    messages.push(json!({ "role": "user", "content": user_text }));

    let raw = llm::chat_json(
        client,
        &settings.llm_base_url,
        &settings.model_id,
        Value::Array(messages),
        response_schema(),
    )
    .await?;

    let mut parsed: ParsedPlan = serde_json::from_value(raw)?;
    resolve_task_deadlines(&mut parsed);
    backfill_event_fields(&mut parsed, user_text);
    Ok(parsed)
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

/// The small model frequently drops or mis-assigns the optional fields it's worst at:
/// `endTime`/`durationMinutes` (→ events collapse to the 60-min default or duration edits
/// loop) and the day when one day covers several events ("birthday lunch tomorrow … and a
/// party … as well" → only one lands on tomorrow). Since the user literally typed these,
/// recover them deterministically from their text — the same way we never trust the model
/// for dates. Only fills/corrects what the model got wrong.
fn backfill_event_fields(plan: &mut ParsedPlan, user_text: &str) {
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

    let range = find_time_range(user_text);
    let duration = find_duration_minutes(user_text);
    if range.is_none() && duration.is_none() {
        return;
    }
    // A time string is "unset" if the model omitted it or it can't be parsed.
    let unset = |s: &Option<String>| s.as_deref().and_then(parse_hm).is_none();

    // Only act when there's a single, unambiguous target, so a range/length is never
    // mis-assigned across multiple events in one message.
    let single_create = plan.events.len() == 1 && plan.update_events.is_empty() && plan.remove_events.is_empty();
    let single_update = plan.update_events.len() == 1 && plan.events.is_empty() && plan.remove_events.is_empty();

    if single_create {
        let ev = &mut plan.events[0];
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
    }
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

/// Find a time RANGE ("12-2", "2pm to 4pm", "3:30–5") in free text.
/// Returns (start normalized to 24h, the verbatim end slice). The end is returned raw so
/// the existing `compute_end` PM-recovery still handles "12-2" → 14:00, overnight, etc.
fn find_time_range(text: &str) -> Option<(NaiveTime, String)> {
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
    for w in toks.windows(2) {
        let gap: String = chars[w[0].end..w[1].start].iter().collect();
        if is_range_gap(&gap) {
            return Some((w[0].norm, w[1].raw.clone()));
        }
    }
    None
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

/// True if the event carries any usable time signal (start, end, or a positive duration).
fn event_has_time(ev: &ParsedEvent) -> bool {
    ev.start_time.as_deref().and_then(parse_hm).is_some()
        || ev.end_time.as_deref().and_then(parse_hm).is_some()
        || ev.duration_minutes.map(|d| d > 0).unwrap_or(false)
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

    let start_time = ev.start_time.as_deref().and_then(parse_hm).unwrap_or(NaiveTime::from_hms_opt(12, 0, 0).unwrap());
    let start = date.and_time(start_time);
    // An explicit end wins; otherwise a stated duration; otherwise the 60-min default.
    let end = match ev.duration_minutes.and_then(sane_duration) {
        Some(d) if ev.end_time.as_deref().and_then(parse_hm).is_none() => start + Duration::minutes(d),
        _ => compute_end(start, ev.end_time.as_deref()),
    };
    Some((start, end_after(start, end)))
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
fn merge_event(
    existing: &Event,
    now: NaiveDateTime,
    day: Option<&str>,
    start_time: Option<&str>,
    end_time: Option<&str>,
    duration: Option<i64>,
    title: Option<&str>,
) -> (String, String, String) {
    let cur_start = parse_dt(&existing.start).unwrap_or(now);
    let cur_dur = parse_dt(&existing.end)
        .map(|e| (e - cur_start).num_minutes())
        .filter(|d| *d > 0)
        .unwrap_or(60);
    let date = day
        .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("null"))
        .and_then(|d| resolve_day(now.date(), d))
        .unwrap_or(cur_start.date());
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
    let new_title = title
        .filter(|t| !t.trim().is_empty())
        .map(|t| t.to_string())
        .unwrap_or_else(|| existing.title.clone());
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
) -> Vec<String> {
    // Common words that carry no identity — matching on these would over-suppress.
    const FILLER: &[&str] = &["with", "the", "and", "you", "your", "for", "that", "this", "from", "into", "about", "new"];
    let touched_words: Vec<String> = created
        .iter()
        .chain(updated.iter())
        .chain(removed.iter())
        .flat_map(|t| t.to_lowercase().split_whitespace().map(str::to_string).collect::<Vec<_>>())
        .filter(|w| w.len() >= 3 && !FILLER.contains(&w.as_str()))
        .collect();
    let placed_event = !created.is_empty() || !updated.is_empty();

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

    // Projects + tasks (dedupe identical task titles within a project).
    for proj in &plan.projects {
        if proj.tasks.is_empty() {
            continue;
        }
        let pid = crate::db::insert_project(conn, &proj.name, "#6366f1")?;
        project_names.push(proj.name.clone());
        let mut seen_titles: HashSet<String> = HashSet::new();
        for t in &proj.tasks {
            if !seen_titles.insert(t.title.to_lowercase()) {
                continue;
            }
            let min_chunk = if t.chunkable { settings.default_min_chunk } else { t.estimated_minutes.max(15) };
            let id = crate::db::insert_task(
                conn,
                Some(pid),
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
            let (t, s, en) = merge_event(&e, now, up.day.as_deref(), up.start_time.as_deref(), up.end_time.as_deref(), up.duration_minutes, up.title.as_deref());
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
        // Guardrail: never persist a blank-titled event (the schema allows ""), it'd show as
        // an empty block the user can't address.
        if ev.title.trim().is_empty() {
            continue;
        }
        // Edit routed as a create: same title exists → merge the change in (keep unspecified fields).
        if let Some(existing) = current.iter_mut().find(|x| x.title.eq_ignore_ascii_case(&ev.title)) {
            let (t, s, e) = merge_event(existing, now, ev.day.as_deref(), ev.start_time.as_deref(), ev.end_time.as_deref(), ev.duration_minutes, None);
            crate::db::update_event(conn, existing.id, &t, &s, &e)?;
            existing.start = s;
            existing.end = e;
            updated_event_titles.push(t);
            continue;
        }
        // Genuinely new event.
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
    );

    Ok(PlanOutcome {
        created_task_ids,
        project_names,
        created_event_titles,
        updated_event_titles,
        removed_event_titles,
        clarifications,
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

        // Move start only → keep the 12h duration.
        let (_t, s, en) = merge_event(&e, now, None, Some("21:00"), None, None, None);
        assert_eq!(s, "2026-06-06T21:00:00");
        assert_eq!(dur(&s, &en), 720);

        // Change end only → keep the original start.
        let (_t, s, en) = merge_event(&e, now, None, None, Some("07:00"), None, None);
        assert_eq!(s, "2026-06-06T20:00:00");
        assert_eq!(dur(&s, &en), 660);

        // Rename only → keep times.
        let (t, s, _en) = merge_event(&e, now, None, None, None, None, Some("Movie Night"));
        assert_eq!(t, "Movie Night");
        assert_eq!(s, "2026-06-06T20:00:00");

        // New duration only ("make it 2 hours") → keep start, set length, ignore old end.
        let (_t, s, en) = merge_event(&e, now, None, None, None, Some(120), None);
        assert_eq!(s, "2026-06-06T20:00:00");
        assert_eq!(dur(&s, &en), 120);

        // An explicit end still wins over a duration if both are somehow present.
        let (_t, s, en) = merge_event(&e, now, None, None, Some("23:00"), Some(120), None);
        assert_eq!(dur(&s, &en), 180);
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
        backfill_event_fields(&mut plan, "lunch with mom friday 12-2");
        assert_eq!(plan.events[0].end_time.as_deref(), Some("2"));
        assert_eq!(dur(&plan.events[0]), 120); // recovered the 2-hour range

        // Model dropped BOTH times → recover start and end from the text.
        let mut plan = ParsedPlan {
            events: vec![ev("friday", None, None)],
            ..Default::default()
        };
        backfill_event_fields(&mut plan, "meeting friday 2pm to 4pm");
        assert_eq!(dur(&plan.events[0]), 120);

        // A correct endTime from the model is never overwritten.
        let mut plan = ParsedPlan {
            events: vec![ev("friday", Some("12:00"), Some("13:00"))],
            ..Default::default()
        };
        backfill_event_fields(&mut plan, "lunch friday 12-2");
        assert_eq!(plan.events[0].end_time.as_deref(), Some("13:00"));

        // No range in the text → nothing changes (still the 60-min default).
        let mut plan = ParsedPlan {
            events: vec![ev("friday", Some("12:00"), None)],
            ..Default::default()
        };
        backfill_event_fields(&mut plan, "lunch friday at noon");
        assert_eq!(plan.events[0].end_time, None);

        // Ambiguous: 2+ events in one message → don't guess which gets the range.
        let mut plan = ParsedPlan {
            events: vec![ev("friday", Some("12:00"), None), ev("friday", Some("15:00"), None)],
            ..Default::default()
        };
        backfill_event_fields(&mut plan, "lunch 12-2 and a call");
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
            }],
            ..Default::default()
        };
        backfill_event_fields(&mut plan, "Change the meeting I have today to be 2 hours instead of 1");
        assert_eq!(plan.update_events[0].duration_minutes, Some(120));

        let up = &plan.update_events[0];
        let (_t, s, en) = merge_event(&meeting, now, up.day.as_deref(), up.start_time.as_deref(), up.end_time.as_deref(), up.duration_minutes, up.title.as_deref());
        assert_eq!(s, "2026-06-04T13:00:00"); // start preserved
        assert_eq!((parse_dt(&en).unwrap() - parse_dt(&s).unwrap()).num_minutes(), 120); // now 2h
    }

    #[test]
    fn clarification_loop_is_broken() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let updated = s(&["Meeting with my friend"]);

        // The exact loop from the screenshots: we updated the meeting but the model still
        // asks for the start/end time. Those questions must be dropped.
        let out = filter_clarifications(
            &s(&["What is the start time of the updated meeting?"]),
            &[],
            &[],
            &updated,
            &[],
        );
        assert!(out.is_empty(), "redundant time question should be dropped, got {out:?}");

        let out = filter_clarifications(&s(&["What is the new end time for the meeting?"]), &[], &[], &updated, &[]);
        assert!(out.is_empty());

        // A question naming a different, untouched event still gets through.
        let out = filter_clarifications(
            &s(&["What time is the dentist appointment?"]),
            &[],
            &[],
            &updated, // we touched the meeting, not the dentist
            &[],
        );
        assert_eq!(out.len(), 1);

        // Our own "couldn't place it" question survives even alongside an edit.
        let out = filter_clarifications(
            &s(&[]),
            &s(&["What date and time is \"Yoga\"?"]),
            &[],
            &updated,
            &[],
        );
        assert_eq!(out.len(), 1);

        // Non-questions (restatements) are dropped.
        let out = filter_clarifications(&s(&["Updated the meeting."]), &[], &[], &updated, &[]);
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

        backfill_event_fields(&mut plan, "I have my birthday lunch tomorrow from 12 - 2 and a graduation party from 6 - 10 as well");

        let tomorrow = NaiveDate::from_ymd_opt(2026, 6, 5).unwrap();
        for e in &plan.events {
            assert_eq!(e.day.as_deref(), Some("tomorrow"));
            assert_eq!(resolve_event(now, e).unwrap().0.date(), tomorrow);
        }

        // Two distinct days in the message → leave each event's day alone.
        let mut plan = ParsedPlan {
            events: vec![ev("today", Some("12:00"), None), ev("tomorrow", Some("18:00"), None)],
            ..Default::default()
        };
        backfill_event_fields(&mut plan, "lunch today and dinner tomorrow");
        assert_eq!(plan.events[0].day.as_deref(), Some("today"));
        assert_eq!(plan.events[1].day.as_deref(), Some("tomorrow"));
    }

    #[test]
    fn day_confirmations_and_chatter_dropped() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let created = s(&["Birthday lunch", "Graduation party"]);

        // Rust already resolved "tomorrow" and placed the events — confirming is noise.
        let out = filter_clarifications(&s(&["Is 'tomorrow' the day after today?"]), &[], &created, &[], &[]);
        assert!(out.is_empty(), "got {out:?}");
        let out = filter_clarifications(&s(&["Is 'tomorrow' referring to June 5th?"]), &[], &created, &[], &[]);
        assert!(out.is_empty());

        // Generic "anything else" filler is dropped.
        let out = filter_clarifications(&s(&["Is there anything else you need me to add or change?"]), &[], &created, &[], &[]);
        assert!(out.is_empty());

        // The reported case: event placed, but the model still asks for its duration.
        let out = filter_clarifications(&s(&["What is the duration in minutes for this event?"]), &[], &created, &[], &[]);
        assert!(out.is_empty(), "duration question after placing is noise, got {out:?}");

        // But when nothing was placed, a genuine day/time question still comes through.
        let out = filter_clarifications(&s(&["What time on tuesday?"]), &[], &[], &[], &[]);
        assert_eq!(out.len(), 1, "no event placed → keep the question, got {out:?}");
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
    fn find_day_phrases_dedupes_and_ignores_substrings() {
        assert_eq!(find_day_phrases("lunch tomorrow and a party as well"), vec!["tomorrow"]);
        assert_eq!(find_day_phrases("lunch today and dinner tomorrow"), vec!["today", "tomorrow"]);
        assert_eq!(find_day_phrases("move it on friday, friday works"), vec!["friday"]);
        assert!(find_day_phrases("make it 2 hours").is_empty());
    }
}
