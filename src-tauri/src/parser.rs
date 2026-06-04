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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedProject {
    pub name: String,
    #[serde(default)]
    pub tasks: Vec<ParsedTask>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
                        "endTime": { "type": ["string", "null"] }
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
                        "endTime": { "type": ["string", "null"] }
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

fn system_prompt(events: &[Event]) -> String {
    let now = Local::now().naive_local();

    // Show the model what's already on the calendar so it can change/remove items.
    let calendar = if events.is_empty() {
        "(the calendar is currently empty)".to_string()
    } else {
        events
            .iter()
            .take(30)
            .map(|e| {
                let when = parse_dt(&e.start)
                    .map(|d| d.format("%a %H:%M").to_string())
                    .unwrap_or_else(|| e.start.clone());
                format!("- {} ({})", e.title, when)
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    format!(
        "Convert the user's message (use the whole conversation for context) into JSON with \
`events`, `updateEvents`, `removeEvents`, `projects`, and `clarifications`.\n\
Choose the right action:\n\
- CREATE a NEW event → add to `events`. EVENTS are things at a set time (lunch, dinner, meeting, \
appointment, call, party). Fields: `title`, `day`, `startTime`, `endTime` (24-hour \"HH:MM\").\n\
- CHANGE an existing event (new time/day/title) → add to `updateEvents` with `match` = the existing \
event's title, plus only the fields that change. Do NOT also create it.\n\
- DELETE/remove an existing event → add its title (or a word from it) to `removeEvents`. \
\"remove all sleepovers\" → removeEvents: [\"sleepover\"]. Do NOT create anything.\n\
- TASKS (work to do: write, design, build, study, plan) → `projects[].tasks` with `estimated_minutes`, \
`priority`, `depends_on`. NEVER put work as an event.\n\
Rules:\n\
- `day` is the EXACT word the user used (\"today\", \"tomorrow\", or a weekday). NEVER output a computed date.\n\
- Now is {now} ({weekday}). \"12 - 2\" → startTime 12:00, endTime 14:00; assume PM for ambiguous hours \
unless clearly morning. Overnight ranges are fine (\"8pm to 8am\").\n\
- If the user is editing/removing, use updateEvents/removeEvents — do NOT add a duplicate event.\n\
- `clarifications` only for genuinely missing info, each a question ending with \"?\". Never restate.\n\
- Never output the same item twice.\n\
Events already on the calendar (reference these to change or remove them):\n\
{calendar}\n\
Examples:\n\
user: lunch with mom friday 12-2 → {{\"events\":[{{\"title\":\"Lunch with mom\",\"day\":\"friday\",\"startTime\":\"12:00\",\"endTime\":\"14:00\"}}]}}\n\
user: remove all sleepovers → {{\"removeEvents\":[\"sleepover\"]}}\n\
user: make the sleepover 8pm to 8am → {{\"updateEvents\":[{{\"match\":\"sleepover\",\"startTime\":\"20:00\",\"endTime\":\"08:00\"}}]}}\n\
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
    Ok(parsed)
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

/// Resolve an event's (start, end). Returns None if no date/day at all.
fn resolve_event(now: NaiveDateTime, ev: &ParsedEvent) -> Option<(NaiveDateTime, NaiveDateTime)> {
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
        })?;

    let start_time = ev.start_time.as_deref().and_then(parse_hm).unwrap_or(NaiveTime::from_hms_opt(12, 0, 0).unwrap());
    let start = date.and_time(start_time);
    Some((start, compute_end(start, ev.end_time.as_deref())))
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
    let new_end = if end_time.and_then(parse_hm).is_some() {
        compute_end(new_start, end_time)
    } else {
        new_start + Duration::minutes(cur_dur)
    };
    let new_title = title
        .filter(|t| !t.trim().is_empty())
        .map(|t| t.to_string())
        .unwrap_or_else(|| existing.title.clone());
    (new_title, fmt_dt(new_start), fmt_dt(new_end))
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
            let (t, s, en) = merge_event(&e, now, up.day.as_deref(), up.start_time.as_deref(), up.end_time.as_deref(), up.title.as_deref());
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
        // Edit routed as a create: same title exists → merge the change in (keep unspecified fields).
        if let Some(existing) = current.iter_mut().find(|x| x.title.eq_ignore_ascii_case(&ev.title)) {
            let (t, s, e) = merge_event(existing, now, ev.day.as_deref(), ev.start_time.as_deref(), ev.end_time.as_deref(), None);
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

    // Clarifications: keep only real questions, drop ones about events we created, dedupe.
    let created_lc: Vec<String> = created_event_titles.iter().map(|t| t.to_lowercase()).collect();
    let mut clarifications: Vec<String> = Vec::new();
    for c in plan.clarifications.iter().chain(extra_clarifications.iter()) {
        let c = c.trim();
        if !c.contains('?') {
            continue; // skip restatements / chatter
        }
        let c_lc = c.to_lowercase();
        if created_lc.iter().any(|t| c_lc.contains(t)) {
            continue; // already handled — we created that event
        }
        if !clarifications.iter().any(|x| x.eq_ignore_ascii_case(c)) {
            clarifications.push(c.to_string());
        }
    }

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
        let (_t, s, en) = merge_event(&e, now, None, Some("21:00"), None, None);
        assert_eq!(s, "2026-06-06T21:00:00");
        assert_eq!(dur(&s, &en), 720);

        // Change end only → keep the original start.
        let (_t, s, en) = merge_event(&e, now, None, None, Some("07:00"), None);
        assert_eq!(s, "2026-06-06T20:00:00");
        assert_eq!(dur(&s, &en), 660);

        // Rename only → keep times.
        let (t, s, _en) = merge_event(&e, now, None, None, None, Some("Movie Night"));
        assert_eq!(t, "Movie Night");
        assert_eq!(s, "2026-06-06T20:00:00");
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
}
