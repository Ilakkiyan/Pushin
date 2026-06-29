//! Tauri command surface — the typed IPC the frontend calls. The DB mutex is never
//! held across an `.await` (so async commands stay `Send`).

use crate::booking::{self, BookingSlot};
use crate::booking_server::{self, BookingServerHandle, BookingServerStatus};
use crate::calendar::google;
use crate::model::*;
use crate::model_manager::{self, ModelInfo};
use crate::parser::{self, PlanOutcome};
use crate::schedule_service::reschedule_inner;
use crate::scheduler::{self, Interval};
use crate::{db, habits, hermes, llm};
use chrono::{Datelike, Duration, Local, NaiveDate, NaiveDateTime, Timelike};
use rusqlite::Connection;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::process::Child;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, State};

pub struct AppState {
    pub db: Arc<Mutex<Connection>>,
    pub http: reqwest::Client,
    pub server: Mutex<Option<Child>>,
    /// The second llama-server, in embeddings mode, powering Hermes semantic recall.
    pub embed_server: Mutex<Option<Child>>,
    pub booking_server: Mutex<Option<BookingServerHandle>>,
    /// The device-sync engine (Iroh mesh). `None` until the device joins/creates a network.
    pub sync_engine: Mutex<Option<Arc<crate::sync::engine::SyncEngine>>>,
    /// The two-way markdown-vault file watcher (`None` until a vault folder is set). Dropping it stops
    /// watching, so changing/clearing the folder just replaces it.
    pub vault_watcher: Mutex<Option<crate::vault::VaultWatcher>>,
    /// Hashes of files Pushin itself just wrote, so the watcher doesn't echo our own saves back into
    /// the DB. Shared with `vault_write`.
    pub vault_echo: crate::vault::EchoGuard,
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

/// Mirror a vault page to `<vault_dir>/<rel_path>.md` (two-way markdown vault). No-op if the user
/// hasn't picked a vault folder. Records the page→file mapping + an echo hash so the watcher ignores
/// this write.
#[tauri::command]
pub fn vault_write(state: State<AppState>, page_id: i64, rel_path: String, markdown: String) -> Result<(), String> {
    {
        let conn = state.db.lock().unwrap();
        let Some(dir) = db::get_settings(&conn).map_err(err)?.vault_dir else {
            return Ok(());
        };
        // Record the echo hash *before* writing so the watcher (another thread) can't process the OS
        // event before we've registered it.
        if let Ok(mut echo) = state.vault_echo.lock() {
            echo.insert(rel_path.clone(), crate::vault::content_hash(&markdown));
        }
        crate::vault::write_file(&dir, &rel_path, &markdown).map_err(err)?;
        db::set_page_rel_path(&conn, page_id, Some(&rel_path)).map_err(err)?;
    }
    Ok(())
}

/// The page currently mapped to `rel_path`, if any (file→page lookup used by the watcher path).
#[tauri::command]
pub fn vault_page_for_path(state: State<AppState>, rel_path: String) -> Result<Option<i64>, String> {
    let conn = state.db.lock().unwrap();
    db::page_id_for_rel_path(&conn, &rel_path).map_err(err)
}

/// Map an externally-created file to a (just-created) page, without writing the file back.
#[tauri::command]
pub fn vault_link_path(state: State<AppState>, page_id: i64, rel_path: String) -> Result<(), String> {
    let conn = state.db.lock().unwrap();
    db::set_page_rel_path(&conn, page_id, Some(&rel_path)).map_err(err)
}

/// A file was deleted on disk: unlink the page→file mapping (the page itself survives — deleting it on
/// an external `rm` would be too destructive).
#[tauri::command]
pub fn vault_unlink_path(state: State<AppState>, rel_path: String) -> Result<(), String> {
    let conn = state.db.lock().unwrap();
    db::unlink_rel_path(&conn, &rel_path).map_err(err)
}

/// (Re)start the vault file watcher to match the current `vault_dir` setting — start it when a folder
/// is set, drop it (stop watching) when cleared. Called at boot and after the folder changes.
#[tauri::command]
pub fn vault_refresh_watch(state: State<AppState>, app: tauri::AppHandle) -> Result<(), String> {
    start_vault_watch(&app, &state);
    Ok(())
}

/// Sync the watcher to the `vault_dir` setting. Best-effort — a failed watch never breaks the app.
pub fn start_vault_watch(app: &tauri::AppHandle, state: &AppState) {
    let dir = {
        let conn = state.db.lock().unwrap();
        db::get_settings(&conn).ok().and_then(|s| s.vault_dir)
    };
    let next = dir.and_then(|d| crate::vault::start_watch(&d, app.clone(), state.vault_echo.clone()).ok());
    if let Ok(mut guard) = state.vault_watcher.lock() {
        *guard = next; // dropping the old watcher stops the previous folder
    }
}

/// The cosine floor for injecting a recalled note into the planner. bge-small's similarity for
/// *unrelated* short text measures ~0.59 (two random notes scored 0.587 in testing), so the floor
/// must clear that baseline with margin — better to recall nothing than feed the prompt-sensitive
/// small model an irrelevant note (gotcha #1). Tune against a real corpus.
const RECALL_FLOOR: f32 = 0.65;

/// Pick the recalled context worth injecting into the planner prompt: only when *semantic* recall
/// ran (embed server up), only strong matches (cosine ≥ `RECALL_FLOOR`), at most 2, each truncated.
/// Graph-neighbor and recency items (which carry no score) are excluded here by design. Pure.
fn gate_recalled_context(bundle: &crate::context::ContextBundle) -> Vec<String> {
    if bundle.mode != "semantic" {
        return Vec::new();
    }
    bundle
        .items
        .iter()
        .filter(|it| it.score.unwrap_or(0.0) >= RECALL_FLOOR)
        .take(2)
        .map(|it| it.text.trim().chars().take(220).collect::<String>())
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
    // Auto-recall relevant durable notes (pages) to inform planning. Pages only: the planner already
    // sees current events, and recalling tasks/events just adds noise. Best-effort and conservative —
    // see `gate_recalled_context` (semantic-only, strong-match, capped + truncated).
    let recalled: Vec<String> = match recall_context(&state, &text, &[EntityKind::Page], 5).await {
        Ok(b) => gate_recalled_context(&b),
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
pub fn update_event_type(
    state: State<AppState>,
    id: i64,
    name: String,
    duration_minutes: i64,
    buffer_minutes: i64,
    color: String,
    enabled: bool,
) -> Result<EventType, String> {
    let conn = state.db.lock().unwrap();
    db::update_event_type(&conn, id, &name, duration_minutes, buffer_minutes, &color, enabled).map_err(err)
}

#[tauri::command]
pub fn regenerate_event_type_token(state: State<AppState>, id: i64) -> Result<EventType, String> {
    let conn = state.db.lock().unwrap();
    db::regenerate_event_type_token(&conn, id).map_err(err)
}

#[tauri::command]
pub fn delete_event_type(state: State<AppState>, id: i64) -> Result<(), String> {
    let conn = state.db.lock().unwrap();
    db::delete_event_type(&conn, id).map_err(err)
}

#[tauri::command]
pub fn booking_server_status(state: State<AppState>) -> Result<BookingServerStatus, String> {
    let guard = state.booking_server.lock().unwrap();
    Ok(guard.as_ref().map(|s| s.status()).unwrap_or_else(booking_server::stopped_status))
}

#[tauri::command]
pub fn start_booking_server(state: State<AppState>, port: Option<u16>) -> Result<BookingServerStatus, String> {
    let mut guard = state.booking_server.lock().unwrap();
    if let Some(server) = guard.as_ref() {
        return Ok(server.status());
    }
    let server = booking_server::start(Arc::clone(&state.db), state.http.clone(), port).map_err(err)?;
    let status = server.status();
    *guard = Some(server);
    Ok(status)
}

#[tauri::command]
pub fn stop_booking_server(state: State<AppState>) -> Result<BookingServerStatus, String> {
    let mut guard = state.booking_server.lock().unwrap();
    if let Some(server) = guard.take() {
        server.stop();
    }
    Ok(booking_server::stopped_status())
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
    let et = db::get_event_type(&conn, event_type_id).map_err(err)?;
    booking::confirm_booking(&mut conn, &settings, &et, &name, &email, &start, &end).map_err(err)?;
    reschedule_inner(&mut conn, &settings).map_err(err)
}

#[tauri::command]
pub fn cancel_booking(state: State<AppState>, id: i64) -> Result<ScheduleResult, String> {
    let mut conn = state.db.lock().unwrap();
    let settings = db::get_settings(&conn).map_err(err)?;
    db::cancel_booking(&mut conn, id).map_err(err)?;
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
    let summary = google::sync(state.db.as_ref(), &state.http).await.map_err(err)?;
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

/// Save a durable fact as a vault note, embedding it on-device (best effort) so it's available for
/// semantic recall. If there's no embeddings backend the note is stored unindexed and still found via
/// keyword. (Used by the chat→memory "Remember this?" chip; the old standalone notes list/delete
/// commands were retired with `HermesPane`.)
#[tauri::command]
pub async fn hermes_add_note(state: State<'_, AppState>, content: String) -> Result<(), String> {
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
    Ok(())
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

/// Cross-entity recall — the Context Engine's read path. Embeds the query, ranks everything in
/// `entity_index` (semantic when the embed server is up + entities indexed, else keyword), expands
/// the top hits with 1-hop graph neighbors, appends a small recency tail, and trims to a budget.
/// The DB lock is never held across the embed `.await` (gotcha #8).
async fn recall_context(state: &State<'_, AppState>, query: &str, kinds: &[EntityKind], k: usize) -> Result<crate::context::ContextBundle, String> {
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
    // One lock: rank the index snapshot, then resolve neighbors + recency from the same connection.
    let (mode, top, neighbors, recency) = {
        let conn = state.db.lock().unwrap();
        let all = db::entity_index_for_recall(&conn, kinds).map_err(err)?;
        let text_map: HashMap<(EntityKind, i64), String> = all.iter().map(|it| ((it.kind, it.id), it.text.clone())).collect();
        let (mode, top) = hermes::rank_items(all, qvec.as_deref(), query, k);
        let mut neighbors = Vec::new();
        let mut seen: HashSet<(EntityKind, i64)> = HashSet::new();
        for hit in &top {
            if let Ok(refs) = db::entity_neighbors(&conn, hit.kind, hit.id) {
                for (nk, nid) in refs {
                    if seen.insert((nk, nid)) {
                        if let Some(text) = text_map.get(&(nk, nid)) {
                            neighbors.push(ContextItem { kind: nk, id: nid, text: text.clone(), score: None, embedding: None });
                        }
                    }
                }
            }
        }
        let recency = db::recent_entities(&conn, 4).unwrap_or_default();
        (mode.to_string(), top, neighbors, recency)
    };
    let budget = crate::context::Budget { max_items: 8, max_chars: 4000 };
    let items = crate::context::merge_and_trim(vec![top, neighbors, recency], &budget);
    Ok(crate::context::ContextBundle { mode, items })
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

/// Bring the cross-entity recall index (`entity_index`) up to date: (re)embed tasks/events/pages
/// whose projected text is new or changed, and prune rows for entities that no longer exist.
/// Best-effort and idempotent — unchanged rows are skipped via their `text_hash`, and when the embed
/// backend is down rows are still tracked (NULL vector, keyword-searchable) and retried next sweep.
/// Spawnable: takes owned handles and never holds the DB lock across the embed `.await` (gotcha #8).
/// Returns how many rows were embedded this pass.
pub async fn reindex_all(db: Arc<Mutex<Connection>>, http: reqwest::Client) -> usize {
    const BATCH: usize = 32;
    use std::collections::HashMap;

    // 1. Snapshot model + entities + existing index state under one short lock.
    let (model, items, existing) = {
        let conn = db.lock().unwrap();
        let model = db::get_settings(&conn).map(|s| s.embed_model).unwrap_or_default();
        let items = match db::entities_for_index(&conn) {
            Ok(v) => v,
            Err(_) => return 0,
        };
        let mut existing: HashMap<(EntityKind, i64), crate::context::IndexState> = HashMap::new();
        for kind in [EntityKind::Task, EntityKind::Event, EntityKind::Page] {
            if let Ok(map) = db::entity_index_meta(&conn, kind) {
                for (id, st) in map {
                    existing.insert((kind, id), st);
                }
            }
        }
        (model, items, existing)
    };

    // 2. Decide what needs (re)indexing and what to prune.
    let present: HashSet<(EntityKind, i64)> = items.iter().map(|it| (it.kind, it.id)).collect();
    let mut todo: Vec<(ContextItem, String)> = Vec::new();
    for it in items {
        let hash = crate::context::text_hash(&it.text);
        if crate::context::needs_index_work(existing.get(&(it.kind, it.id)), &hash, &model) {
            todo.push((it, hash));
        }
    }
    let to_prune: Vec<(EntityKind, i64)> = existing.keys().filter(|k| !present.contains(k)).cloned().collect();
    if todo.is_empty() && to_prune.is_empty() {
        return 0;
    }

    // 3. Embed changed rows in batches, outside the lock. On failure keep NULL (keyword fallback).
    let base = model_manager::embed_base_url();
    let prepared: Vec<(ContextItem, String, Option<Vec<u8>>)> = if model.trim().is_empty() {
        todo.into_iter().map(|(it, hash)| (it, hash, None)).collect()
    } else {
        let mut prepared = Vec::with_capacity(todo.len());
        for chunk in todo.chunks(BATCH) {
            let texts: Vec<&str> = chunk.iter().map(|(it, _)| it.text.as_str()).collect();
            let vecs = hermes::embed_batch(&http, &base, &model, &texts).await.ok();
            for (i, (it, hash)) in chunk.iter().enumerate() {
                let blob = vecs.as_ref().and_then(|v| v.get(i)).map(|fv| hermes::vec_to_blob(fv));
                prepared.push((it.clone(), hash.clone(), blob));
            }
        }
        prepared
    };

    // 4. Write under a short lock.
    let conn = db.lock().unwrap();
    let mut embedded = 0usize;
    for (it, hash, blob) in &prepared {
        let model_used = blob.as_ref().map(|_| model.as_str());
        if db::upsert_entity_index(&conn, it.kind, it.id, &it.text, hash, blob.as_deref(), model_used).is_ok() && blob.is_some() {
            embedded += 1;
        }
    }
    for (kind, id) in to_prune {
        let _ = db::delete_entity_index(&conn, kind, id);
    }
    embedded
}

/// Kick off a background reindex sweep (fire-and-forget) so recall stays current without blocking
/// the caller. Used after the embed engine comes up.
fn spawn_reindex(state: &State<'_, AppState>) {
    let db = Arc::clone(&state.db);
    let http = state.http.clone();
    tauri::async_runtime::spawn(async move {
        let _ = reindex_all(db, http).await;
    });
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

// ---------- Labels (cross-cutting taxonomy) ----------

/// All non-archived labels with usage counts.
#[tauri::command]
pub fn list_labels(state: State<AppState>) -> Result<Vec<Label>, String> {
    let conn = state.db.lock().unwrap();
    db::list_labels(&conn).map_err(err)
}

/// Create a label; returns the refreshed list.
#[tauri::command]
pub fn create_label(state: State<AppState>, input: LabelInput) -> Result<Vec<Label>, String> {
    let conn = state.db.lock().unwrap();
    db::create_label(&conn, &input).map_err(err)?;
    db::list_labels(&conn).map_err(err)
}

#[tauri::command]
pub fn update_label(state: State<AppState>, id: i64, input: LabelInput) -> Result<Vec<Label>, String> {
    let conn = state.db.lock().unwrap();
    db::update_label(&conn, id, &input).map_err(err)?;
    db::list_labels(&conn).map_err(err)
}

#[tauri::command]
pub fn delete_label(state: State<AppState>, id: i64) -> Result<Vec<Label>, String> {
    let conn = state.db.lock().unwrap();
    db::delete_label(&conn, id).map_err(err)?;
    db::list_labels(&conn).map_err(err)
}

#[tauri::command]
pub fn merge_labels(state: State<AppState>, from: i64, into: i64) -> Result<Vec<Label>, String> {
    let conn = state.db.lock().unwrap();
    db::merge_labels(&conn, from, into).map_err(err)?;
    db::list_labels(&conn).map_err(err)
}

/// Replace the full label set on an entity (`kind` = task|event|habit|page|project).
#[tauri::command]
pub fn set_entity_labels(state: State<AppState>, kind: String, entity_id: i64, label_ids: Vec<i64>) -> Result<(), String> {
    let conn = state.db.lock().unwrap();
    db::set_entity_labels(&conn, &kind, entity_id, &label_ids).map_err(err)
}

/// The labels on an entity.
#[tauri::command]
pub fn labels_for(state: State<AppState>, kind: String, entity_id: i64) -> Result<Vec<Label>, String> {
    let conn = state.db.lock().unwrap();
    db::labels_for(&conn, &kind, entity_id).map_err(err)
}

/// Labels for many entities of one kind in a single call, keyed by entity id.
#[tauri::command]
pub fn labels_for_entities(state: State<AppState>, kind: String, ids: Vec<i64>) -> Result<std::collections::BTreeMap<i64, Vec<Label>>, String> {
    let conn = state.db.lock().unwrap();
    db::labels_for_entities(&conn, &kind, &ids).map_err(err)
}

/// Quick "create on the fly" from the picker — find-or-create by name; returns the refreshed list.
#[tauri::command]
pub fn quick_label(state: State<AppState>, name: String, color: String) -> Result<Vec<Label>, String> {
    let conn = state.db.lock().unwrap();
    db::get_or_create_label(&conn, &name, &color).map_err(err)?;
    db::list_labels(&conn).map_err(err)
}

/// Every entity tagged with a label (for the cross-cutting filtered view).
#[tauri::command]
pub fn entities_for_label(state: State<AppState>, label_id: i64) -> Result<Vec<EntityRef>, String> {
    let conn = state.db.lock().unwrap();
    db::entities_for_label(&conn, label_id).map_err(err)
}

/// Ask-your-vault (local RAG): recall the most relevant pages, then have the on-device chat model
/// answer using ONLY those notes and cite which it used. Citations are page ids. Best for the 7B+.
#[tauri::command]
pub async fn vault_ask(state: State<'_, AppState>, question: String) -> Result<VaultAnswer, String> {
    let question = question.trim().to_string();
    if question.is_empty() {
        return Ok(VaultAnswer { answer: String::new(), citations: vec![] });
    }
    let recalled = recall_context(&state, &question, &[EntityKind::Task, EntityKind::Event, EntityKind::Page, EntityKind::Person], 6).await?;
    if recalled.items.is_empty() {
        return Ok(VaultAnswer { answer: "I don't have any notes about that yet.".into(), citations: vec![] });
    }
    // Number the items [1..] so the model cites by index. Tasks/events inform the answer, but only
    // pages are citable (clickable) — non-page slots map to 0 and are dropped from citations.
    let mut context = String::new();
    for (i, it) in recalled.items.iter().enumerate() {
        let snippet: String = it.text.trim().chars().take(500).collect();
        context.push_str(&format!("[{}] {}\n", i + 1, snippet.replace('\n', " ")));
    }
    let ids: Vec<i64> = recalled.items.iter().map(|it| if it.kind == EntityKind::Page { it.id } else { 0 }).collect();

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
    let mut citations = map_citation_indices(&sources, &ids);
    citations.retain(|&c| c > 0); // drop non-page (0) slots
    Ok(VaultAnswer { answer, citations })
}

/// A prior turn in the assistant conversation (for multi-turn continuity).
#[derive(serde::Deserialize)]
pub struct ChatTurn {
    pub role: String,
    pub content: String,
}

/// The "deharnessed" general assistant: a free-form, on-device chat grounded with RAG over the user's
/// vault/entities. The SAME 7B as the planner, just unconstrained (no json_schema) and warmer — for
/// thinking out loud, Q&A, and capturing/organizing thoughts. The planner (`plan_tasks`) still owns
/// scheduling; the frontend toggles between the two modes.
#[tauri::command]
pub async fn assistant_chat(state: State<'_, AppState>, message: String, history: Vec<ChatTurn>) -> Result<String, String> {
    let message = message.trim().to_string();
    if message.is_empty() {
        return Ok(String::new());
    }
    // Best-effort RAG: pull possibly-relevant context. The assistant still answers general questions
    // when nothing relevant is found (unlike `vault_ask`, which is notes-only).
    let context = match recall_context(&state, &message, &[EntityKind::Task, EntityKind::Event, EntityKind::Page, EntityKind::Person], 6).await {
        Ok(b) if !b.items.is_empty() => {
            let mut c = String::from("\n\nPossibly-relevant notes from the user's vault (use only if they help):\n");
            for it in b.items.iter() {
                let snip: String = it.text.trim().chars().take(400).collect();
                c.push_str(&format!("- {}\n", snip.replace('\n', " ")));
            }
            c
        }
        _ => String::new(),
    };
    let settings = {
        let conn = state.db.lock().unwrap();
        db::get_settings(&conn).map_err(err)?
    };
    let system = format!(
        "You are Pushin, a private, on-device assistant and \"second brain\" for the user. Be helpful, \
warm, and concise. Help them think things through, answer questions, and capture/organize their \
thoughts. Everything stays on their device. Never invent facts about their life — if you don't know, \
say so or ask.{context}"
    );
    let mut messages = vec![serde_json::json!({ "role": "system", "content": system })];
    // Keep the last ~10 turns for continuity without overrunning the context window.
    let start = history.len().saturating_sub(10);
    for turn in &history[start..] {
        let role = if turn.role == "user" { "user" } else { "assistant" };
        messages.push(serde_json::json!({ "role": role, "content": turn.content }));
    }
    messages.push(serde_json::json!({ "role": "user", "content": message }));
    llm::chat_text(&state.http, &settings.llm_base_url, &settings.model_id, serde_json::Value::Array(messages))
        .await
        .map_err(err)
}

/// Keyword auto-label suggestions for an entity: existing labels whose name appears in the entity's
/// text and aren't already applied. Surfaced as confirm-chips in the label picker.
#[tauri::command]
pub fn suggest_labels(state: State<AppState>, kind: String, entity_id: i64) -> Result<Vec<Label>, String> {
    let conn = state.db.lock().unwrap();
    let Some(text) = db::entity_text(&conn, &kind, entity_id).map_err(err)? else {
        return Ok(vec![]);
    };
    let labels = db::list_labels(&conn).map_err(err)?;
    let applied: Vec<i64> = db::labels_for(&conn, &kind, entity_id).map_err(err)?.iter().map(|l| l.id).collect();
    Ok(db::suggest_labels_from(&labels, &text, &applied))
}

// ---------- People (relationship layer / private CRM) ----------

#[tauri::command]
pub fn list_people(state: State<AppState>) -> Result<Vec<Person>, String> {
    let conn = state.db.lock().unwrap();
    db::list_people(&conn).map_err(err)
}

#[tauri::command]
pub fn get_person(state: State<AppState>, id: i64) -> Result<Person, String> {
    let conn = state.db.lock().unwrap();
    db::get_person(&conn, id).map_err(err)
}

#[tauri::command]
pub fn create_person(state: State<AppState>, name: String, email: Option<String>, notes: Option<String>) -> Result<Person, String> {
    let conn = state.db.lock().unwrap();
    let id = db::insert_person(&conn, &name, email.as_deref(), &notes.unwrap_or_default()).map_err(err)?;
    db::get_person(&conn, id).map_err(err)
}

#[tauri::command]
pub fn update_person(state: State<AppState>, id: i64, name: String, email: Option<String>, notes: String) -> Result<Person, String> {
    let conn = state.db.lock().unwrap();
    db::update_person(&conn, id, &name, email.as_deref(), &notes).map_err(err)
}

#[tauri::command]
pub fn delete_person(state: State<AppState>, id: i64) -> Result<(), String> {
    let conn = state.db.lock().unwrap();
    db::delete_person(&conn, id).map_err(err)
}

// ---------- Focus / time-tracking ----------

#[tauri::command]
pub fn start_focus(state: State<AppState>, task_id: i64) -> Result<FocusSession, String> {
    let conn = state.db.lock().unwrap();
    db::start_focus(&conn, task_id).map_err(err)
}

#[tauri::command]
pub fn stop_focus(state: State<AppState>, id: i64) -> Result<(), String> {
    let conn = state.db.lock().unwrap();
    db::stop_focus(&conn, id).map_err(err)
}

#[tauri::command]
pub fn active_focus(state: State<AppState>) -> Result<Option<FocusSession>, String> {
    let conn = state.db.lock().unwrap();
    db::active_focus(&conn).map_err(err)
}

#[tauri::command]
pub fn task_focus_minutes(state: State<AppState>, task_id: i64) -> Result<i64, String> {
    let conn = state.db.lock().unwrap();
    db::focus_minutes_for_task(&conn, task_id).map_err(err)
}

// ---------- Meeting Companion ----------

/// Clean the model's action-item array: trim, drop blanks, dedupe (case-insensitive), cap at 10.
/// Pure, so the fragile-input handling is unit-tested without a model.
fn clean_action_items(raw: &serde_json::Value) -> Vec<String> {
    let mut seen = HashSet::new();
    raw["items"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty() && seen.insert(s.to_lowercase()))
                .take(10)
                .collect()
        })
        .unwrap_or_default()
}

/// Extract concrete follow-up action items from meeting notes (LLM). Returns candidate task titles —
/// nothing is created; the UI shows them as confirm-chips the user opts into. Strict schema + caps
/// keep the small model bounded (gotcha #4). The DB lock is dropped before the network call.
#[tauri::command]
pub async fn extract_action_items(state: State<'_, AppState>, notes: String) -> Result<Vec<String>, String> {
    let notes = notes.trim().to_string();
    if notes.is_empty() {
        return Ok(vec![]);
    }
    let settings = {
        let conn = state.db.lock().unwrap();
        db::get_settings(&conn).map_err(err)?
    };
    let schema = serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "items": { "type": "array", "maxItems": 10, "items": { "type": "string", "maxLength": 120 } }
        },
        "required": ["items"]
    });
    let system = "Extract the concrete follow-up action items (to-dos) from the meeting notes below. \
Each item is a short, imperative task title. Only include real action items that appear in the notes — \
do NOT invent any. If there are none, return an empty list.";
    let messages = serde_json::json!([
        { "role": "system", "content": system },
        { "role": "user", "content": notes }
    ]);
    let raw = llm::chat_json(&state.http, &settings.llm_base_url, &settings.model_id, messages, schema)
        .await
        .map_err(err)?;
    Ok(clean_action_items(&raw))
}

/// The deterministic pre-meeting brief for an event: attendees (booked invitees → people) with their
/// relationship history, plus notes linked to the meeting.
#[tauri::command]
pub fn meeting_brief(state: State<AppState>, event_id: i64) -> Result<MeetingBrief, String> {
    let conn = state.db.lock().unwrap();
    let event = db::list_events(&conn)
        .map_err(err)?
        .into_iter()
        .find(|e| e.id == event_id)
        .ok_or_else(|| "Event not found".to_string())?;
    let bookings = db::list_bookings(&conn).map_err(err)?;
    let people = db::list_people(&conn).map_err(err)?;
    let linked = db::entity_pages(&conn, "event", event_id).map_err(err)?;
    Ok(crate::meeting::assemble(&event, &bookings, &people, linked))
}

// ---------- Planning rituals ----------

/// The morning Daily Briefing: today's events, due/overdue tasks, and scheduled focus time.
/// Deterministic — assembled from SQLite, no LLM. `date` defaults to today (`YYYY-MM-DD`).
#[tauri::command]
pub fn daily_briefing(state: State<AppState>, date: Option<String>) -> Result<crate::briefing::Briefing, String> {
    let conn = state.db.lock().unwrap();
    let today = date
        .as_deref()
        .and_then(|d| NaiveDate::parse_from_str(d.trim(), "%Y-%m-%d").ok())
        .unwrap_or_else(|| Local::now().naive_local().date());
    let events = db::list_events(&conn).map_err(err)?;
    let tasks = db::list_tasks(&conn).map_err(err)?;
    let blocks = db::list_blocks(&conn).map_err(err)?;
    Ok(crate::briefing::assemble(today, &events, &tasks, &blocks))
}

/// Make Hermes' semantic recall work with zero setup: ensure the small embedding model is
/// downloaded and the second (embeddings) llama-server is running. Idempotent and safe to call
/// blindly — callers treat failure as "stay on keyword recall". Downloads happen outside any lock.
#[tauri::command]
pub async fn ensure_embeddings(app: AppHandle, state: State<'_, AppState>) -> Result<String, String> {
    let base = model_manager::embed_base_url();
    if llm::health(&state.http, &base).await {
        spawn_reindex(&state);
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
            spawn_reindex(&state);
            return Ok("Memory engine ready.".into());
        }
    }
    Err("Memory engine is taking a while to start — give it a moment.".into())
}

// ============================ Device sync (private Iroh mesh) ============================
use crate::sync;
use crate::sync::engine::SyncEngine;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    /// A mesh secret exists — this device belongs to a network.
    enabled: bool,
    /// The engine is bound and serving.
    running: bool,
    node_id: String,
    device_name: String,
    use_relay: bool,
    peers: Vec<sync::state::Peer>,
}

fn build_status(state: &AppState) -> Result<SyncStatus, String> {
    let enabled = sync::identity::mesh_secret().is_some();
    let running = state.sync_engine.lock().map_err(err)?.is_some();
    let conn = state.db.lock().map_err(err)?;
    Ok(SyncStatus {
        enabled,
        running,
        node_id: sync::state::node_id(&conn).map_err(err)?.unwrap_or_default(),
        device_name: sync::state::device_name(&conn).map_err(err)?,
        use_relay: sync::state::use_relay(&conn),
        peers: sync::state::list_peers(&conn).map_err(err)?,
    })
}

/// Return the running engine, starting it if the device is paired but the engine is down.
pub(crate) async fn ensure_engine(app: AppHandle, state: &AppState) -> Result<Arc<SyncEngine>, String> {
    if let Some(e) = state.sync_engine.lock().map_err(err)?.clone() {
        return Ok(e);
    }
    if sync::identity::mesh_secret().is_none() {
        return Err("This device hasn't joined a sync network yet.".into());
    }
    let use_relay = { sync::state::use_relay(&*state.db.lock().map_err(err)?) };
    let engine = SyncEngine::start(state.db.clone(), app, use_relay).await.map_err(err)?;
    *state.sync_engine.lock().map_err(err)? = Some(engine.clone());
    Ok(engine)
}

#[tauri::command]
pub async fn sync_status(state: State<'_, AppState>) -> Result<SyncStatus, String> {
    build_status(state.inner())
}

/// Start a network (if needed) and mint an invite ticket another device pastes to join.
#[tauri::command]
pub async fn sync_create_invite(app: AppHandle, state: State<'_, AppState>) -> Result<String, String> {
    sync::identity::ensure_mesh_secret();
    let engine = ensure_engine(app, state.inner()).await?;
    engine.create_invite().await.map_err(err)
}

/// Join a network from an invite ticket: adopt its mesh secret, then do an initial sync.
#[tauri::command]
pub async fn sync_join(app: AppHandle, state: State<'_, AppState>, ticket: String) -> Result<SyncStatus, String> {
    let (addr, mesh) = sync::transport::parse_ticket(&ticket).map_err(err)?;
    if !sync::identity::set_mesh_secret(&mesh) {
        return Err("Couldn't store the network key in the OS keychain.".into());
    }
    let engine = ensure_engine(app, state.inner()).await?;
    engine.sync_with(addr).await.map_err(err)?;
    build_status(state.inner())
}

#[tauri::command]
pub async fn sync_now(app: AppHandle, state: State<'_, AppState>) -> Result<usize, String> {
    let engine = ensure_engine(app, state.inner()).await?;
    Ok(engine.sync_all_peers().await)
}

#[tauri::command]
pub async fn sync_remove_peer(state: State<'_, AppState>, node_id: String) -> Result<(), String> {
    let conn = state.db.lock().map_err(err)?;
    sync::state::remove_peer(&conn, &node_id).map_err(err)
}

#[tauri::command]
pub async fn sync_set_device_name(state: State<'_, AppState>, name: String) -> Result<(), String> {
    let conn = state.db.lock().map_err(err)?;
    sync::state::set_device_name(&conn, &name).map_err(err)
}

/// Toggle relay use (LAN/direct-only when off). Restarts the engine to rebind.
#[tauri::command]
pub async fn sync_set_relay(app: AppHandle, state: State<'_, AppState>, use_relay: bool) -> Result<(), String> {
    {
        let conn = state.db.lock().map_err(err)?;
        sync::state::set_use_relay(&conn, use_relay).map_err(err)?;
    }
    // Take the engine out (dropping the lock guard) before awaiting its shutdown.
    let old = state.sync_engine.lock().map_err(err)?.take();
    if let Some(e) = old {
        e.shutdown().await;
    }
    if sync::identity::mesh_secret().is_some() {
        ensure_engine(app, state.inner()).await?;
    }
    Ok(())
}

/// Leave the network: stop the engine, forget the mesh key + peers (the device keeps its identity).
#[tauri::command]
pub async fn sync_leave(state: State<'_, AppState>) -> Result<(), String> {
    let old = state.sync_engine.lock().map_err(err)?.take();
    if let Some(e) = old {
        e.shutdown().await;
    }
    sync::identity::forget_mesh();
    let conn = state.db.lock().map_err(err)?;
    for p in sync::state::list_peers(&conn).unwrap_or_default() {
        let _ = sync::state::remove_peer(&conn, &p.node_id);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn citem(id: i64, text: &str, score: Option<f32>) -> ContextItem {
        ContextItem { kind: EntityKind::Page, id, text: text.into(), score, embedding: None }
    }

    #[test]
    fn clean_action_items_trims_dedupes_and_caps() {
        let raw = serde_json::json!({
            "items": ["  Email Ava  ", "Email ava", "", "Book the room", "Book the room"]
        });
        assert_eq!(clean_action_items(&raw), vec!["Email Ava".to_string(), "Book the room".to_string()]);
        // Missing/!array field → empty, not a panic.
        assert!(clean_action_items(&serde_json::json!({})).is_empty());
        // Cap at 10.
        let many = serde_json::json!({ "items": (0..20).map(|i| format!("t{i}")).collect::<Vec<_>>() });
        assert_eq!(clean_action_items(&many).len(), 10);
    }

    #[test]
    fn gate_context_only_injects_strong_semantic_matches_capped() {
        // Keyword mode → nothing, even with high scores.
        let kw = crate::context::ContextBundle { mode: "keyword".into(), items: vec![citem(1, "a", Some(0.9))] };
        assert!(gate_recalled_context(&kw).is_empty());

        // Semantic: drop sub-threshold (< RECALL_FLOOR), cap at 2, keep order, drop unscored neighbors.
        let sem = crate::context::ContextBundle {
            mode: "semantic".into(),
            items: vec![
                citem(1, "strong one", Some(0.85)),
                citem(2, "weak", Some(0.58)), // near bge-small's unrelated baseline → excluded
                citem(3, "strong two", Some(0.70)),
                citem(4, "strong three", Some(0.66)),
                citem(5, "neighbor", None),
            ],
        };
        let got = gate_recalled_context(&sem);
        assert_eq!(got, vec!["strong one".to_string(), "strong two".to_string()]);
    }

    #[test]
    fn gate_context_truncates_long_items() {
        let long = "x".repeat(500);
        let sem = crate::context::ContextBundle { mode: "semantic".into(), items: vec![citem(1, &long, Some(0.9))] };
        assert_eq!(gate_recalled_context(&sem)[0].chars().count(), 220);
    }

    #[test]
    fn citation_indices_map_dedup_and_drop_out_of_range() {
        let ids = vec![10, 20, 30];
        // 1-based: 1→10, 3→30; duplicate 1 collapses; 0 and 9 are out of range.
        assert_eq!(map_citation_indices(&[1, 3, 1, 0, 9], &ids), vec![10, 30]);
        assert!(map_citation_indices(&[], &ids).is_empty());
    }
}
