//! Tauri command surface — the typed IPC the frontend calls. The DB mutex is never
//! held across an `.await` (so async commands stay `Send`).

use crate::booking::{self, BookingSlot};
use crate::calendar::google;
use crate::model::*;
use crate::model_manager::{self, ModelInfo};
use crate::parser::{self, PlanOutcome};
use crate::scheduler::{self, Interval};
use crate::{db, habits, hermes, llm};
use chrono::{Datelike, Duration, Local, NaiveDate, NaiveDateTime, Timelike};
use rusqlite::Connection;
use serde::Serialize;
use std::collections::HashSet;
use std::process::Child;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, State};

pub struct AppState {
    pub db: Mutex<Connection>,
    pub http: reqwest::Client,
    pub server: Mutex<Option<Child>>,
    /// The second llama-server, in embeddings mode, powering Hermes semantic recall.
    pub embed_server: Mutex<Option<Child>>,
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Everything the UI needs for an initial render, in one round-trip.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppData {
    settings: Settings,
    projects: Vec<Project>,
    tasks: Vec<Task>,
    events: Vec<Event>,
    blocks: Vec<Block>,
    event_types: Vec<EventType>,
    bookings: Vec<Booking>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmStatus {
    reachable: bool,
    base_url: String,
    model_present: bool,
    model_id: String,
    models: Vec<ModelInfo>,
}

// ---------- core scheduling ----------

/// Recompute the schedule from the current DB state and persist the new blocks.
fn reschedule_inner(conn: &mut Connection, settings: &Settings) -> anyhow::Result<ScheduleResult> {
    let tasks = db::list_tasks(conn)?;
    let events = db::list_events(conn)?;
    let blocks = db::list_blocks(conn)?;

    let fixed: Vec<Interval> = events
        .iter()
        .filter_map(|e| match (scheduler::parse_dt(&e.start), scheduler::parse_dt(&e.end)) {
            (Some(s), Some(en)) => Some(Interval { start: s, end: en }),
            _ => None,
        })
        .collect();

    let locked: Vec<(i64, Interval)> = blocks
        .iter()
        .filter(|b| b.locked)
        .filter_map(|b| match (scheduler::parse_dt(&b.start), scheduler::parse_dt(&b.end)) {
            (Some(s), Some(en)) => Some((b.task_id, Interval { start: s, end: en })),
            _ => None,
        })
        .collect();

    let now = Local::now().naive_local();
    let result = scheduler::schedule(now, settings, &tasks, &fixed, &locked);
    db::replace_unlocked_blocks(conn, &result.blocks)?;

    // Light status sync: tasks with any block become "scheduled" (unless done).
    let scheduled_ids: std::collections::HashSet<i64> =
        db::list_blocks(conn)?.iter().map(|b| b.task_id).collect();
    for t in &tasks {
        if t.status == "done" || t.status == "in_progress" {
            continue;
        }
        let new = if scheduled_ids.contains(&t.id) { "scheduled" } else { "todo" };
        if new != t.status {
            db::set_task_status(conn, t.id, new)?;
        }
    }
    Ok(result)
}

#[tauri::command]
pub fn load_all(state: State<AppState>) -> Result<AppData, String> {
    let conn = state.db.lock().unwrap();
    Ok(AppData {
        settings: db::get_settings(&conn).map_err(err)?,
        projects: db::list_projects(&conn).map_err(err)?,
        tasks: db::list_tasks(&conn).map_err(err)?,
        events: db::list_events(&conn).map_err(err)?,
        blocks: db::list_blocks(&conn).map_err(err)?,
        event_types: db::list_event_types(&conn).map_err(err)?,
        bookings: db::list_bookings(&conn).map_err(err)?,
    })
}

#[tauri::command]
pub fn reschedule(state: State<AppState>) -> Result<ScheduleResult, String> {
    let mut conn = state.db.lock().unwrap();
    let settings = db::get_settings(&conn).map_err(err)?;
    reschedule_inner(&mut conn, &settings).map_err(err)
}

#[tauri::command]
pub fn save_settings(state: State<AppState>, settings: Settings) -> Result<(), String> {
    let conn = state.db.lock().unwrap();
    db::save_settings(&conn, &settings).map_err(err)
}

/// Pick the saved notes worth injecting into the planner prompt: only when *semantic* recall ran
/// (embed server up), only strong matches (cosine ≥ 0.35), at most 2, each truncated. Small models
/// are prompt-sensitive (gotcha #1), so a weak/keyword/empty recall contributes nothing. Pure.
fn gate_recalled_memory(result: &RecallResult) -> Vec<String> {
    if result.mode != "semantic" {
        return Vec::new();
    }
    result
        .notes
        .iter()
        .filter(|n| n.score.unwrap_or(0.0) >= 0.35)
        .take(2)
        .map(|n| n.content.trim().chars().take(220).collect::<String>())
        .collect()
}

/// Map a model's 1-based citation indices to page ids, dropping out-of-range ones, sorted + deduped.
/// `note_ids` is the recalled notes in the order they were numbered in the prompt. Pure.
fn map_citation_indices(sources: &[i64], note_ids: &[i64]) -> Vec<i64> {
    let mut citations: Vec<i64> = sources
        .iter()
        .filter_map(|&n| note_ids.get((n - 1) as usize).copied())
        .collect();
    citations.sort_unstable();
    citations.dedup();
    citations
}

// ---------- AI planning ----------

#[tauri::command]
pub async fn plan_tasks(
    state: State<'_, AppState>,
    text: String,
    history: Option<Vec<parser::ChatTurn>>,
) -> Result<PlanOutcome, String> {
    let (settings, current_events) = {
        let conn = state.db.lock().unwrap();
        (db::get_settings(&conn).map_err(err)?, db::list_events(&conn).map_err(err)?)
    };
    // Auto-recall relevant saved notes to inform planning. Best-effort and conservative — see
    // `gate_recalled_memory` (semantic-only, strong-match, capped + truncated).
    let recalled: Vec<String> = match recall_notes(&state, &text, 3).await {
        Ok(r) => gate_recalled_memory(&r),
        Err(_) => Vec::new(),
    };
    // Network call — no DB lock held here.
    let parsed = parser::plan(&state.http, &settings, &current_events, &history.unwrap_or_default(), &text, &recalled)
        .await
        .map_err(err)?;

    let mut conn = state.db.lock().unwrap();
    let mut outcome = parser::store_plan(&conn, &settings, &parsed).map_err(err)?;
    reschedule_inner(&mut conn, &settings).map_err(err)?;
    outcome.recalled_notes = recalled;
    Ok(outcome)
}

/// Surface durable facts/preferences in a chat message so the UI can offer to remember them. The
/// user confirms before anything is saved (keeps the vault clean). Empty on a no-op / model failure.
#[tauri::command]
pub async fn extract_memories(state: State<'_, AppState>, text: String) -> Result<Vec<String>, String> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return Ok(vec![]);
    }
    let settings = {
        let conn = state.db.lock().unwrap();
        db::get_settings(&conn).map_err(err)?
    };
    Ok(parser::extract_memories(&state.http, &settings, &text).await.unwrap_or_default())
}

// ---------- tasks ----------

#[tauri::command]
pub fn create_task(
    state: State<AppState>,
    title: String,
    estimated_minutes: i64,
    deadline: Option<String>,
    priority: i64,
    project_id: Option<i64>,
) -> Result<ScheduleResult, String> {
    let mut conn = state.db.lock().unwrap();
    let settings = db::get_settings(&conn).map_err(err)?;
    let deadline = deadline.and_then(|d| scheduler::parse_dt(&d).map(scheduler::fmt_dt));
    db::insert_task(
        &conn,
        project_id,
        &title,
        "",
        estimated_minutes.max(15),
        deadline.as_deref(),
        priority.clamp(1, 4),
        settings.default_min_chunk,
        settings.default_max_chunk,
        &[],
    )
    .map_err(err)?;
    reschedule_inner(&mut conn, &settings).map_err(err)
}

#[tauri::command]
pub fn set_task_status(state: State<AppState>, id: i64, status: String) -> Result<ScheduleResult, String> {
    let mut conn = state.db.lock().unwrap();
    let settings = db::get_settings(&conn).map_err(err)?;
    db::set_task_status(&conn, id, &status).map_err(err)?;
    reschedule_inner(&mut conn, &settings).map_err(err)
}

#[tauri::command]
pub fn delete_task(state: State<AppState>, id: i64) -> Result<ScheduleResult, String> {
    let mut conn = state.db.lock().unwrap();
    let settings = db::get_settings(&conn).map_err(err)?;
    db::delete_task(&conn, id).map_err(err)?;
    reschedule_inner(&mut conn, &settings).map_err(err)
}

// ---------- projects ----------

#[tauri::command]
pub fn delete_project(state: State<AppState>, id: i64) -> Result<ScheduleResult, String> {
    let mut conn = state.db.lock().unwrap();
    let settings = db::get_settings(&conn).map_err(err)?;
    db::delete_project(&conn, id).map_err(err)?;
    reschedule_inner(&mut conn, &settings).map_err(err)
}

#[tauri::command]
pub fn set_project_archived(state: State<AppState>, id: i64, archived: bool) -> Result<ScheduleResult, String> {
    let mut conn = state.db.lock().unwrap();
    let settings = db::get_settings(&conn).map_err(err)?;
    db::set_project_archived(&conn, id, archived).map_err(err)?;
    reschedule_inner(&mut conn, &settings).map_err(err)
}

// ---------- events & blocks ----------

#[tauri::command]
pub fn add_event(
    state: State<AppState>,
    title: String,
    start: String,
    end: String,
    kind: String,
) -> Result<ScheduleResult, String> {
    let mut conn = state.db.lock().unwrap();
    let settings = db::get_settings(&conn).map_err(err)?;
    db::insert_event(&conn, &title, &start, &end, &kind).map_err(err)?;
    reschedule_inner(&mut conn, &settings).map_err(err)
}

#[tauri::command]
pub fn delete_event(state: State<AppState>, id: i64) -> Result<ScheduleResult, String> {
    let mut conn = state.db.lock().unwrap();
    let settings = db::get_settings(&conn).map_err(err)?;
    db::delete_event(&conn, id).map_err(err)?;
    reschedule_inner(&mut conn, &settings).map_err(err)
}

/// Pin/unpin a block (and optionally move it). Pinned blocks survive reschedules.
#[tauri::command]
pub fn lock_block(
    state: State<AppState>,
    id: i64,
    locked: bool,
    start: String,
    end: String,
) -> Result<ScheduleResult, String> {
    let mut conn = state.db.lock().unwrap();
    let settings = db::get_settings(&conn).map_err(err)?;
    db::set_block_locked(&conn, id, locked, &start, &end).map_err(err)?;
    reschedule_inner(&mut conn, &settings).map_err(err)
}

// ---------- booking seam ----------

#[tauri::command]
pub fn list_event_types(state: State<AppState>) -> Result<Vec<EventType>, String> {
    let conn = state.db.lock().unwrap();
    db::list_event_types(&conn).map_err(err)
}

#[tauri::command]
pub fn create_event_type(
    state: State<AppState>,
    name: String,
    duration_minutes: i64,
    buffer_minutes: i64,
    color: String,
) -> Result<i64, String> {
    let conn = state.db.lock().unwrap();
    db::insert_event_type(&conn, &name, duration_minutes, buffer_minutes, &color).map_err(err)
}

#[tauri::command]
pub fn delete_event_type(state: State<AppState>, id: i64) -> Result<(), String> {
    let conn = state.db.lock().unwrap();
    db::delete_event_type(&conn, id).map_err(err)
}

// ---------- habits ----------

/// Load every active habit with its derived streak/consistency metrics, plus how many days from
/// today forward it's currently dropped on the calendar (drives the "Add to calendar" toggle).
fn habit_stats(conn: &Connection) -> anyhow::Result<Vec<HabitStats>> {
    let today = Local::now().naive_local().date();
    // Future habit placements per habit name → distinct day count.
    let events = db::list_events(conn)?;
    let scheduled_days = |name: &str| -> i64 {
        let days: HashSet<NaiveDate> = events
            .iter()
            .filter(|e| e.kind == "habit" && e.title.eq_ignore_ascii_case(name))
            .filter_map(|e| scheduler::parse_dt(&e.start).map(|d| d.date()))
            .filter(|d| *d >= today)
            .collect();
        days.len() as i64
    };
    db::list_habits(conn)?
        .iter()
        .map(|h| {
            let done: HashSet<NaiveDate> = db::done_days_for_habit(conn, h.id)?
                .iter()
                .filter_map(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
                .collect();
            let mut stat = habits::compute_stats(h, &done, today);
            stat.scheduled_days = scheduled_days(&h.name);
            Ok(stat)
        })
        .collect()
}

#[tauri::command]
pub fn list_habits(state: State<AppState>) -> Result<Vec<HabitStats>, String> {
    let conn = state.db.lock().unwrap();
    habit_stats(&conn).map_err(err)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn create_habit(
    state: State<AppState>,
    name: String,
    color: String,
    cadence: String,
    days: Vec<u8>,
    interval_days: i64,
    duration_minutes: i64,
) -> Result<Vec<HabitStats>, String> {
    let conn = state.db.lock().unwrap();
    db::insert_habit(&conn, &name, &color, &cadence, &days, interval_days.max(1), duration_minutes.clamp(5, 24 * 60)).map_err(err)?;
    habit_stats(&conn).map_err(err)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn update_habit(
    state: State<AppState>,
    id: i64,
    name: String,
    color: String,
    cadence: String,
    days: Vec<u8>,
    interval_days: i64,
    duration_minutes: i64,
) -> Result<Vec<HabitStats>, String> {
    let conn = state.db.lock().unwrap();
    db::update_habit(&conn, id, &name, &color, &cadence, &days, interval_days.max(1), duration_minutes.clamp(5, 24 * 60)).map_err(err)?;
    habit_stats(&conn).map_err(err)
}

/// Awake window habits are slotted into.
const HABIT_DAY_START_H: u32 = 7;
const HABIT_DAY_END_H: u32 = 22;

/// Drop a habit into the best free gap on `day_date` as a `kind="habit"` event, so the task
/// scheduler plans around it. No-ops (returns false) if it's already on that day or the day is
/// too full to fit; returns true when it actually placed one. Does NOT re-plan — the caller
/// batches a single `reschedule_inner` after placing one or many days.
fn place_habit_on_day(conn: &Connection, habit: &Habit, day_date: NaiveDate, now: NaiveDateTime) -> anyhow::Result<bool> {
    let events = db::list_events(conn)?;
    if habits::habit_already_on_day(&events, &habit.name, day_date) {
        return Ok(false);
    }

    // Everything already on that day becomes "busy" so the habit slots around it.
    let day_lo = day_date.and_hms_opt(0, 0, 0).unwrap();
    let day_hi = day_date.and_hms_opt(23, 59, 59).unwrap();
    let mut busy: Vec<Interval> = Vec::new();
    let mut collect = |start: &str, end: &str| {
        if let (Some(s), Some(e)) = (scheduler::parse_dt(start), scheduler::parse_dt(end)) {
            if e > day_lo && s < day_hi {
                busy.push(Interval { start: s, end: e });
            }
        }
    };
    for ev in &events {
        collect(&ev.start, &ev.end);
    }
    for b in db::list_blocks(conn)? {
        collect(&b.start, &b.end);
    }
    drop(collect);

    // Awake window for the day; never place in the past when it's today.
    let mut window_start = day_date.and_hms_opt(HABIT_DAY_START_H, 0, 0).unwrap();
    let window_end = day_date.and_hms_opt(HABIT_DAY_END_H, 0, 0).unwrap();
    if day_date == now.date() {
        let rounded = ((now.hour() as i64 * 60 + now.minute() as i64) + 14) / 15 * 15;
        let candidate = day_lo + Duration::minutes(rounded);
        if candidate > window_start {
            window_start = candidate.min(window_end);
        }
    }

    match habits::find_habit_slot(&busy, window_start, window_end, habit.duration_minutes) {
        Some((s, e)) => {
            db::insert_event(conn, &habit.name, &scheduler::fmt_dt(s), &scheduler::fmt_dt(e), "habit")?;
            Ok(true)
        }
        None => Ok(false),
    }
}

fn find_habit(conn: &Connection, id: i64) -> Result<Habit, String> {
    db::list_habits(conn)
        .map_err(err)?
        .into_iter()
        .find(|h| h.id == id)
        .ok_or_else(|| "habit not found".to_string())
}

/// Drop a habit onto the calendar for a single day (default today), then re-plan.
#[tauri::command]
pub fn schedule_habit(state: State<AppState>, id: i64, day: Option<String>) -> Result<ScheduleResult, String> {
    let mut conn = state.db.lock().unwrap();
    let settings = db::get_settings(&conn).map_err(err)?;
    let now = Local::now().naive_local();
    let day_date = day
        .as_deref()
        .and_then(|s| NaiveDate::parse_from_str(s.get(..10).unwrap_or(s), "%Y-%m-%d").ok())
        .unwrap_or_else(|| now.date());

    let habit = find_habit(&conn, id)?;
    let already = habits::habit_already_on_day(&db::list_events(&conn).map_err(err)?, &habit.name, day_date);
    if !already && !place_habit_on_day(&conn, &habit, day_date, now).map_err(err)? {
        return Err("No room left in the day to schedule this habit — try another day.".into());
    }
    reschedule_inner(&mut conn, &settings).map_err(err)
}

/// Toggle a habit on/off the calendar across the whole planning period. On: place it in a free
/// gap on every day from today through the horizon (skipping days it's already on or that are
/// full). Off: remove its instances from today forward (past days stay as history). Re-plans once.
#[tauri::command]
pub fn set_habit_scheduled(state: State<AppState>, id: i64, scheduled: bool) -> Result<ScheduleResult, String> {
    let mut conn = state.db.lock().unwrap();
    let settings = db::get_settings(&conn).map_err(err)?;
    let now = Local::now().naive_local();
    let habit = find_habit(&conn, id)?;

    if scheduled {
        let horizon = settings.horizon_days.max(1);
        for d in 0..horizon {
            let day = now.date() + Duration::days(d);
            // Only drop the habit on the days its cadence calls for (daily/weekly/interval).
            if habits::is_due(&habit, day) {
                place_habit_on_day(&conn, &habit, day, now).map_err(err)?;
            }
        }
    } else {
        for ev in db::list_events(&conn).map_err(err)? {
            let future = scheduler::parse_dt(&ev.start).map(|s| s.date() >= now.date()).unwrap_or(false);
            if ev.kind == "habit" && ev.title.eq_ignore_ascii_case(&habit.name) && future {
                db::delete_event(&conn, ev.id).map_err(err)?;
            }
        }
    }
    reschedule_inner(&mut conn, &settings).map_err(err)
}

/// Toggle a habit's completion for a day ("YYYY-MM-DD"); defaults to today.
#[tauri::command]
pub fn toggle_habit(state: State<AppState>, id: i64, day: Option<String>) -> Result<Vec<HabitStats>, String> {
    let conn = state.db.lock().unwrap();
    let day = day.unwrap_or_else(|| Local::now().naive_local().date().format("%Y-%m-%d").to_string());
    db::toggle_habit_log(&conn, id, &day).map_err(err)?;
    habit_stats(&conn).map_err(err)
}

#[tauri::command]
pub fn delete_habit(state: State<AppState>, id: i64) -> Result<Vec<HabitStats>, String> {
    let conn = state.db.lock().unwrap();
    db::delete_habit(&conn, id).map_err(err)?;
    habit_stats(&conn).map_err(err)
}

#[tauri::command]
pub fn booking_slots(state: State<AppState>, event_type_id: i64, horizon_days: i64) -> Result<Vec<BookingSlot>, String> {
    let conn = state.db.lock().unwrap();
    let settings = db::get_settings(&conn).map_err(err)?;
    let et = db::list_event_types(&conn)
        .map_err(err)?
        .into_iter()
        .find(|e| e.id == event_type_id)
        .ok_or_else(|| "event type not found".to_string())?;
    booking::available_slots(&conn, &settings, &et, horizon_days.clamp(1, 60)).map_err(err)
}

#[tauri::command]
pub fn create_booking(
    state: State<AppState>,
    event_type_id: i64,
    name: String,
    email: String,
    start: String,
    end: String,
) -> Result<ScheduleResult, String> {
    let mut conn = state.db.lock().unwrap();
    let settings = db::get_settings(&conn).map_err(err)?;
    db::insert_booking(&mut conn, event_type_id, &name, &email, &start, &end).map_err(err)?;
    reschedule_inner(&mut conn, &settings).map_err(err)
}

// ---------- Google Calendar two-way sync ----------

/// Run the OAuth consent flow and store the account. Returns the connected email.
#[tauri::command]
pub async fn connect_google(app: AppHandle, state: State<'_, AppState>) -> Result<String, String> {
    let (client_id, client_secret) = {
        let conn = state.db.lock().unwrap();
        let s = db::get_settings(&conn).map_err(err)?;
        (s.google_client_id.clone(), s.google_client_secret.clone())
    };
    let connected = google::connect(&app, &state.http, &client_id, &client_secret).await.map_err(err)?;
    {
        let conn = state.db.lock().unwrap();
        db::save_google_account(
            &conn,
            &connected.email,
            &connected.calendar_id,
            &connected.access_token,
            &connected.refresh_token,
            &connected.token_expiry,
        )
        .map_err(err)?;
        let mut s = db::get_settings(&conn).map_err(err)?;
        s.google_connected = true;
        db::save_settings(&conn, &s).map_err(err)?;
    }
    Ok(connected.email)
}

#[tauri::command]
pub fn disconnect_google(state: State<AppState>) -> Result<(), String> {
    let conn = state.db.lock().unwrap();
    db::delete_google_account(&conn).map_err(err)?;
    let mut s = db::get_settings(&conn).map_err(err)?;
    s.google_connected = false;
    db::save_settings(&conn, &s).map_err(err)?;
    Ok(())
}

/// Two-way sync with Google Calendar, then re-plan around anything newly pulled in.
#[tauri::command]
pub async fn sync_google(state: State<'_, AppState>) -> Result<google::SyncSummary, String> {
    let summary = google::sync(&state.db, &state.http).await.map_err(err)?;
    {
        let mut conn = state.db.lock().unwrap();
        let settings = db::get_settings(&conn).map_err(err)?;
        reschedule_inner(&mut conn, &settings).map_err(err)?;
    }
    Ok(summary)
}


// ---------- inference / model management ----------

#[tauri::command]
pub async fn llm_status(app: AppHandle, state: State<'_, AppState>) -> Result<LlmStatus, String> {
    let (base_url, model_id) = {
        let conn = state.db.lock().unwrap();
        let s = db::get_settings(&conn).map_err(err)?;
        (s.llm_base_url.clone(), s.model_id.clone())
    };
    // True if the configured model (or any model) is already downloaded.
    let model_present = model_manager::is_model_present(&app, &model_id) || model_manager::first_present_model(&app).is_some();
    let reachable = llm::health(&state.http, &base_url).await;
    Ok(LlmStatus {
        reachable,
        base_url,
        model_present,
        model_id,
        models: model_manager::MODELS.to_vec(),
    })
}

#[tauri::command]
pub fn list_models() -> Vec<ModelInfo> {
    model_manager::MODELS.to_vec()
}

#[tauri::command]
pub fn model_present(app: AppHandle, id: String) -> bool {
    model_manager::is_model_present(&app, &id)
}

#[tauri::command]
pub async fn download_model(app: AppHandle, state: State<'_, AppState>, id: String, sha256: Option<String>) -> Result<String, String> {
    let client = state.http.clone();
    model_manager::download_model(app, client, id, sha256.unwrap_or_default())
        .await
        .map_err(err)
}

/// Try to make local inference available: detect a running server, else spawn one.
/// Returns a human-readable status string.
#[tauri::command]
pub async fn ensure_inference(app: AppHandle, state: State<'_, AppState>) -> Result<String, String> {
    let (base_url, model_id) = {
        let conn = state.db.lock().unwrap();
        let s = db::get_settings(&conn).map_err(err)?;
        (s.llm_base_url.clone(), s.model_id.clone())
    };

    if llm::health(&state.http, &base_url).await {
        return Ok("Connected to a running local inference server.".into());
    }

    // Use the configured model if downloaded, otherwise fall back to any downloaded model.
    let model_to_use = if model_manager::is_model_present(&app, &model_id) {
        model_id.clone()
    } else if let Some(id) = model_manager::first_present_model(&app) {
        id.to_string()
    } else {
        return Err("Download a model first, then start the AI.".into());
    };

    // Persist the choice so the UI + future starts agree on the active model.
    if model_to_use != model_id {
        let conn = state.db.lock().unwrap();
        let mut s = db::get_settings(&conn).map_err(err)?;
        s.model_id = model_to_use.clone();
        db::save_settings(&conn, &s).map_err(err)?;
    }

    // Make sure the engine binary exists (auto-downloads the prebuilt llama.cpp server).
    model_manager::ensure_server_binary(&app, &state.http)
        .await
        .map_err(err)?;

    let _ = app.emit("inference-status", "Starting the model…");
    match model_manager::spawn_server(&app, &model_to_use, &base_url) {
        Ok(child) => {
            *state.server.lock().unwrap() = Some(child);
            // The model can take a little while to load on first run.
            for _ in 0..60 {
                tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
                if llm::health(&state.http, &base_url).await {
                    let _ = app.emit("inference-status", "AI is ready.");
                    return Ok("AI is ready.".into());
                }
            }
            Err("The model is taking a while to load — give it a moment and try again.".into())
        }
        Err(e) => Err(format!("Couldn't start the inference server: {e}")),
    }
}

// ---------- Hermes (on-device memory layer) ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecallResult {
    /// "semantic" (embedding cosine) or "keyword" (fallback) — so the UI can show which ran.
    mode: String,
    notes: Vec<Note>,
}

#[tauri::command]
pub fn hermes_list_notes(state: State<AppState>) -> Result<Vec<Note>, String> {
    let conn = state.db.lock().unwrap();
    db::list_notes(&conn).map_err(err)
}

#[tauri::command]
pub fn hermes_delete_note(state: State<AppState>, id: i64) -> Result<Vec<Note>, String> {
    let conn = state.db.lock().unwrap();
    db::delete_note(&conn, id).map_err(err)?;
    db::list_notes(&conn).map_err(err)
}

/// Save a note, embedding it on-device (best effort) so it's available for semantic recall. If the
/// backend has no embeddings endpoint the note is stored unindexed and still found via keyword.
#[tauri::command]
pub async fn hermes_add_note(state: State<'_, AppState>, content: String) -> Result<Vec<Note>, String> {
    let content = content.trim().to_string();
    if content.is_empty() {
        return Err("Note is empty.".into());
    }
    // Short lock to read embedding config; dropped before the network call (gotcha #8).
    let base = model_manager::embed_base_url();
    let model = {
        let conn = state.db.lock().unwrap();
        db::get_settings(&conn).map_err(err)?.embed_model
    };
    let blob = if model.trim().is_empty() {
        None
    } else {
        hermes::embed_text(&state.http, &base, &model, &content).await.ok().map(|v| hermes::vec_to_blob(&v))
    };
    let conn = state.db.lock().unwrap();
    db::insert_note(&conn, &content, blob.as_deref(), blob.as_ref().map(|_| model.as_str())).map_err(err)?;
    db::list_notes(&conn).map_err(err)
}

/// Recall the notes most relevant to `query` (shared by the recall command, the planner's auto-recall,
/// and ask-your-vault). Semantic cosine when the embed server is up and notes are indexed, else
/// keyword. The DB lock is never held across the embed `.await` (gotcha #8).
async fn recall_notes(state: &State<'_, AppState>, query: &str, k: usize) -> Result<RecallResult, String> {
    let model = {
        let conn = state.db.lock().unwrap();
        db::get_settings(&conn).map_err(err)?.embed_model
    };
    let qvec = if model.trim().is_empty() {
        None
    } else {
        let base = model_manager::embed_base_url();
        hermes::embed_text(&state.http, &base, &model, query).await.ok()
    };
    let notes = {
        let conn = state.db.lock().unwrap();
        db::notes_for_recall(&conn).map_err(err)?
    };
    let (mode, ranked) = hermes::rank_notes(notes, qvec.as_deref(), query, k);
    Ok(RecallResult { mode: mode.into(), notes: ranked })
}

/// Recall the notes most relevant to `query`: semantic cosine when embeddings exist, else keyword.
#[tauri::command]
pub async fn hermes_recall(state: State<'_, AppState>, query: String, k: Option<i64>) -> Result<RecallResult, String> {
    let query = query.trim().to_string();
    if query.is_empty() {
        return Ok(RecallResult { mode: "keyword".into(), notes: vec![] });
    }
    let k = k.unwrap_or(5).clamp(1, 50) as usize;
    recall_notes(&state, &query, k).await
}

// ---------- Vault pages (Notion-style documents over the Hermes notes store) ----------

/// Embed `text` on-device, best-effort. Returns the (blob, model) to persist, or None when there's
/// no embedding backend / empty text — recall then falls back to keyword. Mirrors `hermes_add_note`.
/// The DB lock is taken only to read the model name and dropped before the network call (gotcha #8).
async fn embed_best_effort(state: &State<'_, AppState>, text: &str) -> (Option<Vec<u8>>, String) {
    let model = {
        let conn = state.db.lock().unwrap();
        db::get_settings(&conn).map(|s| s.embed_model).unwrap_or_default()
    };
    if text.trim().is_empty() || model.trim().is_empty() {
        return (None, model);
    }
    let base = model_manager::embed_base_url();
    let blob = hermes::embed_text(&state.http, &base, &model, text).await.ok().map(|v| hermes::vec_to_blob(&v));
    (blob, model)
}

/// The vault page tree (lightweight rows — no bodies).
#[tauri::command]
pub fn list_pages(state: State<AppState>) -> Result<Vec<Page>, String> {
    let conn = state.db.lock().unwrap();
    db::list_pages(&conn).map_err(err)
}

/// A single page with its full body (for the editor).
#[tauri::command]
pub fn get_page(state: State<AppState>, id: i64) -> Result<Page, String> {
    let conn = state.db.lock().unwrap();
    db::get_page(&conn, id).map_err(err)
}

/// Create a new (optionally child) page and return it. Blank pages aren't embedded until they have content.
#[tauri::command]
pub async fn create_page(state: State<'_, AppState>, title: String, parent_id: Option<i64>, content: Option<String>) -> Result<Page, String> {
    let title = title.trim().to_string();
    let content = content.unwrap_or_default();
    let (blob, model) = embed_best_effort(&state, &content).await;
    let conn = state.db.lock().unwrap();
    let id = db::insert_page(
        &conn,
        if title.is_empty() { "Untitled" } else { &title },
        parent_id,
        &content,
        None,
        blob.as_deref(),
        blob.as_ref().map(|_| model.as_str()),
    )
    .map_err(err)?;
    db::get_page(&conn, id).map_err(err)
}

/// Save a page's title/icon/body + outgoing wikilinks, re-embedding the body for semantic recall.
/// `content` is the rendered plaintext (recall + keyword index); `content_json` is the BlockNote
/// block array; `link_titles` are the titles this page links to (resolved to edges in `set_page_links`).
#[tauri::command]
pub async fn update_page(
    state: State<'_, AppState>,
    id: i64,
    title: String,
    icon: Option<String>,
    content: String,
    content_json: Option<String>,
    link_titles: Vec<String>,
) -> Result<Page, String> {
    let title = title.trim().to_string();
    let (blob, model) = embed_best_effort(&state, &content).await;
    let conn = state.db.lock().unwrap();
    db::update_page(
        &conn,
        id,
        if title.is_empty() { "Untitled" } else { &title },
        icon.as_deref(),
        &content,
        content_json.as_deref(),
        blob.as_deref(),
        blob.as_ref().map(|_| model.as_str()),
    )
    .map_err(err)?;
    db::set_page_links(&conn, id, &link_titles).map_err(err)?;
    db::get_page(&conn, id).map_err(err)
}

/// Delete a page (its outgoing/incoming links cascade; children are re-parented to the root via the
/// ON DELETE SET NULL FK). Returns the refreshed tree.
#[tauri::command]
pub fn delete_page(state: State<AppState>, id: i64) -> Result<Vec<Page>, String> {
    let conn = state.db.lock().unwrap();
    db::delete_note(&conn, id).map_err(err)?;
    db::list_pages(&conn).map_err(err)
}

/// Reparent / reorder a page in the tree. Returns the refreshed tree.
#[tauri::command]
pub fn move_page(state: State<AppState>, id: i64, parent_id: Option<i64>, sort_order: f64) -> Result<Vec<Page>, String> {
    let conn = state.db.lock().unwrap();
    db::move_page(&conn, id, parent_id, sort_order).map_err(err)?;
    db::list_pages(&conn).map_err(err)
}

/// Pages that link to this one ("Linked references").
#[tauri::command]
pub fn page_backlinks(state: State<AppState>, id: i64) -> Result<Vec<Page>, String> {
    let conn = state.db.lock().unwrap();
    db::page_backlinks(&conn, id).map_err(err)
}

/// Free-text search over page titles + bodies (for the link picker and command palette).
#[tauri::command]
pub fn search_pages(state: State<AppState>, query: String) -> Result<Vec<Page>, String> {
    let conn = state.db.lock().unwrap();
    db::search_pages(&conn, &query).map_err(err)
}

/// Pages that mention this page's title but don't link it ("unlinked mentions").
#[tauri::command]
pub fn unlinked_mentions(state: State<AppState>, id: i64) -> Result<Vec<Page>, String> {
    let conn = state.db.lock().unwrap();
    db::unlinked_mentions(&conn, id).map_err(err)
}

/// The whole vault connection graph (nodes + resolved link edges).
#[tauri::command]
pub fn page_graph(state: State<AppState>) -> Result<PageGraph, String> {
    let conn = state.db.lock().unwrap();
    db::page_graph(&conn).map_err(err)
}

/// One-box quick capture: save text to the Inbox (embedded best-effort) to sort later.
#[tauri::command]
pub async fn capture_note(state: State<'_, AppState>, text: String) -> Result<(), String> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err("Nothing to capture.".into());
    }
    let (blob, model) = embed_best_effort(&state, &text).await;
    let conn = state.db.lock().unwrap();
    db::capture(&conn, &text, blob.as_deref(), blob.as_ref().map(|_| model.as_str())).map_err(err)?;
    Ok(())
}

/// The unsorted Inbox.
#[tauri::command]
pub fn list_inbox(state: State<AppState>) -> Result<Vec<Page>, String> {
    let conn = state.db.lock().unwrap();
    db::list_inbox(&conn).map_err(err)
}

/// Keep an Inbox capture as a normal vault page (clears its inbox flag).
#[tauri::command]
pub fn keep_inbox_note(state: State<AppState>, id: i64) -> Result<(), String> {
    let conn = state.db.lock().unwrap();
    db::clear_inbox(&conn, id).map_err(err)
}

/// Open (creating on first access) the vault page for a calendar day. `date` is 'YYYY-MM-DD'; the
/// title is a friendly "Weekday, Month D, YYYY".
#[tauri::command]
pub fn daily_note(state: State<AppState>, date: String) -> Result<Page, String> {
    let title = NaiveDate::parse_from_str(&date, "%Y-%m-%d")
        .map(|d| format!("{}, {} {}, {}", d.format("%A"), d.format("%B"), d.day(), d.year()))
        .unwrap_or_else(|_| date.clone());
    let conn = state.db.lock().unwrap();
    db::get_or_create_daily(&conn, &date, &title).map_err(err)
}

/// Recursively read a folder's Markdown files for the vault importer (Obsidian/Markdown). Skips
/// hidden dirs (`.obsidian`, `.git`, …); title = first `# heading` or the filename stem. Caps at
/// 2000 files so a giant folder can't hang the UI. The frontend converts each to blocks + creates pages.
#[tauri::command]
pub fn read_markdown_dir(path: String) -> Result<Vec<ImportDoc>, String> {
    let root = std::path::PathBuf::from(&path);
    if !root.is_dir() {
        return Err("Not a folder.".into());
    }
    let mut docs = Vec::new();
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            if docs.len() >= 2000 {
                return Ok(docs);
            }
            let p = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue; // skip .obsidian, .git, .trash, etc.
            }
            if p.is_dir() {
                stack.push(p);
            } else if matches!(p.extension().and_then(|e| e.to_str()), Some("md") | Some("markdown")) {
                if let Ok(markdown) = std::fs::read_to_string(&p) {
                    let stem = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Untitled".into());
                    let title = markdown
                        .lines()
                        .find_map(|l| l.strip_prefix("# ").map(|h| h.trim().to_string()))
                        .filter(|h| !h.is_empty())
                        .unwrap_or(stem);
                    docs.push(ImportDoc { title, markdown });
                }
            }
        }
    }
    Ok(docs)
}

/// Link / unlink a page to a task or event (`kind` = "task" | "event").
#[tauri::command]
pub fn link_page_entity(state: State<AppState>, page_id: i64, kind: String, entity_id: i64) -> Result<(), String> {
    let conn = state.db.lock().unwrap();
    db::link_entity(&conn, page_id, &kind, entity_id).map_err(err)
}

#[tauri::command]
pub fn unlink_page_entity(state: State<AppState>, page_id: i64, kind: String, entity_id: i64) -> Result<(), String> {
    let conn = state.db.lock().unwrap();
    db::unlink_entity(&conn, page_id, &kind, entity_id).map_err(err)
}

/// The tasks/events a page references (for the editor's "Linked tasks & events" strip).
#[tauri::command]
pub fn page_entities(state: State<AppState>, page_id: i64) -> Result<Vec<EntityRef>, String> {
    let conn = state.db.lock().unwrap();
    db::page_entities(&conn, page_id).map_err(err)
}

/// The pages that reference a given task/event (for a "Notes" affordance on it).
#[tauri::command]
pub fn entity_pages(state: State<AppState>, kind: String, entity_id: i64) -> Result<Vec<Page>, String> {
    let conn = state.db.lock().unwrap();
    db::entity_pages(&conn, &kind, entity_id).map_err(err)
}

/// Ask-your-vault (local RAG): recall the most relevant pages, then have the on-device chat model
/// answer using ONLY those notes and cite which it used. Citations are page ids. Best for the 7B+.
#[tauri::command]
pub async fn vault_ask(state: State<'_, AppState>, question: String) -> Result<VaultAnswer, String> {
    let question = question.trim().to_string();
    if question.is_empty() {
        return Ok(VaultAnswer { answer: String::new(), citations: vec![] });
    }
    let recalled = recall_notes(&state, &question, 5).await?;
    if recalled.notes.is_empty() {
        return Ok(VaultAnswer { answer: "I don't have any notes about that yet.".into(), citations: vec![] });
    }
    // Number the notes [1..] so the model cites by index; map indices back to page ids after.
    let mut context = String::new();
    for (i, n) in recalled.notes.iter().enumerate() {
        let snippet: String = n.content.trim().chars().take(500).collect();
        context.push_str(&format!("[{}] {}\n", i + 1, snippet.replace('\n', " ")));
    }
    let ids: Vec<i64> = recalled.notes.iter().map(|n| n.id).collect();

    let settings = {
        let conn = state.db.lock().unwrap();
        db::get_settings(&conn).map_err(err)?
    };
    let system = format!(
        "Answer the user's question using ONLY the notes below. Be concise. If the notes don't \
contain the answer, say you don't know — do not invent anything. In `sources`, list the bracketed \
numbers of the notes you actually used.\n\nNotes:\n{context}"
    );
    let schema = serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "answer": { "type": "string" },
            "sources": { "type": "array", "items": { "type": "integer" } }
        },
        "required": ["answer", "sources"]
    });
    let messages = serde_json::json!([
        { "role": "system", "content": system },
        { "role": "user", "content": question }
    ]);
    let raw = llm::chat_json(&state.http, &settings.llm_base_url, &settings.model_id, messages, schema)
        .await
        .map_err(err)?;
    let answer = raw["answer"].as_str().unwrap_or("").trim().to_string();
    let sources: Vec<i64> = raw["sources"].as_array().map(|a| a.iter().filter_map(|v| v.as_i64()).collect()).unwrap_or_default();
    let citations = map_citation_indices(&sources, &ids);
    Ok(VaultAnswer { answer, citations })
}

/// Make Hermes' semantic recall work with zero setup: ensure the small embedding model is
/// downloaded and the second (embeddings) llama-server is running. Idempotent and safe to call
/// blindly — callers treat failure as "stay on keyword recall". Downloads happen outside any lock.
#[tauri::command]
pub async fn ensure_embeddings(app: AppHandle, state: State<'_, AppState>) -> Result<String, String> {
    let base = model_manager::embed_base_url();
    if llm::health(&state.http, &base).await {
        return Ok("Memory engine ready.".into());
    }
    // Reuse the same engine binary as the chat server; fetch the (tiny) embedding model if missing.
    model_manager::ensure_server_binary(&app, &state.http).await.map_err(err)?;
    if !model_manager::is_model_present(&app, model_manager::EMBED_MODEL.id) {
        let _ = app.emit("inference-status", "Setting up memory (one-time ~37 MB)…");
        model_manager::download_model(app.clone(), state.http.clone(), model_manager::EMBED_MODEL.id.to_string(), String::new())
            .await
            .map_err(err)?;
    }
    // Spawn the embeddings server if it isn't already running (guard dropped before the await loop).
    {
        let mut guard = state.embed_server.lock().unwrap();
        if guard.is_none() {
            let child = model_manager::spawn_embed_server(&app).map_err(|e| format!("couldn't start the memory engine: {e}"))?;
            *guard = Some(child);
        }
    }
    for _ in 0..30 {
        tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
        if llm::health(&state.http, &base).await {
            let _ = app.emit("inference-status", "Memory engine ready.");
            return Ok("Memory engine ready.".into());
        }
    }
    Err("Memory engine is taking a while to start — give it a moment.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(id: i64, content: &str, score: Option<f32>) -> Note {
        Note {
            id,
            content: content.into(),
            created_at: "2026-01-01T00:00:00".into(),
            updated_at: "2026-01-01T00:00:00".into(),
            indexed: score.is_some(),
            score,
        }
    }

    #[test]
    fn gate_memory_only_injects_strong_semantic_matches_capped() {
        // Keyword mode → nothing, even with high scores.
        let kw = RecallResult { mode: "keyword".into(), notes: vec![note(1, "a", Some(0.9))] };
        assert!(gate_recalled_memory(&kw).is_empty());

        // Semantic: drop sub-threshold, cap at 2, keep order.
        let sem = RecallResult {
            mode: "semantic".into(),
            notes: vec![
                note(1, "strong one", Some(0.80)),
                note(2, "weak", Some(0.20)),
                note(3, "strong two", Some(0.50)),
                note(4, "strong three", Some(0.45)),
            ],
        };
        let got = gate_recalled_memory(&sem);
        assert_eq!(got, vec!["strong one".to_string(), "strong two".to_string()]);
    }

    #[test]
    fn gate_memory_truncates_long_notes() {
        let long = "x".repeat(500);
        let sem = RecallResult { mode: "semantic".into(), notes: vec![note(1, &long, Some(0.9))] };
        assert_eq!(gate_recalled_memory(&sem)[0].chars().count(), 220);
    }

    #[test]
    fn citation_indices_map_dedup_and_drop_out_of_range() {
        let ids = vec![10, 20, 30];
        // 1-based: 1→10, 3→30; duplicate 1 collapses; 0 and 9 are out of range.
        assert_eq!(map_citation_indices(&[1, 3, 1, 0, 9], &ids), vec![10, 30]);
        assert!(map_citation_indices(&[], &ids).is_empty());
    }
}
