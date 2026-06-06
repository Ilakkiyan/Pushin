//! Tauri command surface — the typed IPC the frontend calls. The DB mutex is never
//! held across an `.await` (so async commands stay `Send`).

use crate::booking::{self, BookingSlot};
use crate::calendar::{google, local::LocalProvider, CalendarProvider};
use crate::model::*;
use crate::model_manager::{self, ModelInfo};
use crate::parser::{self, PlanOutcome};
use crate::scheduler::{self, Interval};
use crate::{db, habits, llm};
use chrono::{Local, NaiveDate, Timelike};
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
    // Network call — no DB lock held here.
    let parsed = parser::plan(&state.http, &settings, &current_events, &history.unwrap_or_default(), &text)
        .await
        .map_err(err)?;

    let mut conn = state.db.lock().unwrap();
    let outcome = parser::store_plan(&conn, &settings, &parsed).map_err(err)?;
    reschedule_inner(&mut conn, &settings).map_err(err)?;
    Ok(outcome)
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

/// Load every active habit with its derived streak/consistency metrics.
fn habit_stats(conn: &Connection) -> anyhow::Result<Vec<HabitStats>> {
    let today = Local::now().naive_local().date();
    db::list_habits(conn)?
        .iter()
        .map(|h| {
            let done: HashSet<NaiveDate> = db::done_days_for_habit(conn, h.id)?
                .iter()
                .filter_map(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
                .collect();
            Ok(habits::compute_stats(h, &done, today))
        })
        .collect()
}

#[tauri::command]
pub fn list_habits(state: State<AppState>) -> Result<Vec<HabitStats>, String> {
    let conn = state.db.lock().unwrap();
    habit_stats(&conn).map_err(err)
}

#[tauri::command]
pub fn create_habit(
    state: State<AppState>,
    name: String,
    color: String,
    cadence: String,
    duration_minutes: i64,
) -> Result<Vec<HabitStats>, String> {
    let conn = state.db.lock().unwrap();
    db::insert_habit(&conn, &name, &color, &cadence, duration_minutes.clamp(5, 24 * 60)).map_err(err)?;
    habit_stats(&conn).map_err(err)
}

#[tauri::command]
pub fn update_habit(
    state: State<AppState>,
    id: i64,
    name: String,
    color: String,
    duration_minutes: i64,
) -> Result<Vec<HabitStats>, String> {
    let conn = state.db.lock().unwrap();
    db::update_habit(&conn, id, &name, &color, duration_minutes.clamp(5, 24 * 60)).map_err(err)?;
    habit_stats(&conn).map_err(err)
}

/// Drop a habit onto the calendar for a day (default today), slotting it into a free gap
/// near the end of the day. Creates a `kind = "habit"` event so the task scheduler plans
/// around it, then re-plans.
#[tauri::command]
pub fn schedule_habit(state: State<AppState>, id: i64, day: Option<String>) -> Result<ScheduleResult, String> {
    const DAY_START_H: u32 = 7;
    const DAY_END_H: u32 = 22;

    let mut conn = state.db.lock().unwrap();
    let settings = db::get_settings(&conn).map_err(err)?;
    let now = Local::now().naive_local();
    let day_date = day
        .as_deref()
        .and_then(|s| NaiveDate::parse_from_str(s.get(..10).unwrap_or(s), "%Y-%m-%d").ok())
        .unwrap_or_else(|| now.date());

    let habit = db::list_habits(&conn)
        .map_err(err)?
        .into_iter()
        .find(|h| h.id == id)
        .ok_or_else(|| "habit not found".to_string())?;

    let events = db::list_events(&conn).map_err(err)?;

    // Only add a habit to a given day once — re-plan and return without duplicating.
    if habits::habit_already_on_day(&events, &habit.name, day_date) {
        return reschedule_inner(&mut conn, &settings).map_err(err);
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
    for b in db::list_blocks(&conn).map_err(err)? {
        collect(&b.start, &b.end);
    }

    // Awake window for the day; never place in the past when it's today.
    let mut window_start = day_date.and_hms_opt(DAY_START_H, 0, 0).unwrap();
    let window_end = day_date.and_hms_opt(DAY_END_H, 0, 0).unwrap();
    if day_date == now.date() {
        let rounded = ((now.hour() as i64 * 60 + now.minute() as i64) + 14) / 15 * 15;
        let candidate = day_lo + chrono::Duration::minutes(rounded);
        if candidate > window_start {
            window_start = candidate.min(window_end);
        }
    }

    match habits::find_habit_slot(&busy, window_start, window_end, habit.duration_minutes) {
        Some((s, e)) => {
            db::insert_event(&conn, &habit.name, &scheduler::fmt_dt(s), &scheduler::fmt_dt(e), "habit").map_err(err)?;
        }
        None => return Err("No room left in the day to schedule this habit — try another day.".into()),
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

/// Local-provider read (the seam demo); unused by the Google path.
#[tauri::command]
pub fn sync_calendar(state: State<AppState>) -> Result<usize, String> {
    let conn = state.db.lock().unwrap();
    let settings = db::get_settings(&conn).map_err(err)?;
    let now = Local::now().naive_local();
    let start = scheduler::fmt_dt(now);
    let end = scheduler::fmt_dt(now + chrono::Duration::days(settings.horizon_days.max(1)));
    let events = LocalProvider.pull_events(&conn, &start, &end).map_err(err)?;
    Ok(events.len())
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
