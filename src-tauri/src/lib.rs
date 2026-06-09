mod booking;
mod calendar;
mod commands;
// Public so the LLM-evaluation harness (`tests/llm_eval.rs`) can drive the real parsing
// pipeline against a live llama-server. No external consumers otherwise.
pub mod db;
mod habits;
mod hermes;
mod llm;
pub mod model;
mod model_manager;
pub mod parser;
mod scheduler;

use commands::AppState;
use std::sync::Mutex;
use tauri::{Manager, RunEvent};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let handle = app.handle();
            let data_dir = handle.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let conn = db::open(&data_dir.join("pushin.db"))?;

            // Seed a default booking offering on first run.
            if db::list_event_types(&conn)?.is_empty() {
                db::insert_event_type(&conn, "30-minute call", 30, 10, "#0ea5e9")?;
            }

            app.manage(AppState {
                db: Mutex::new(conn),
                http: reqwest::Client::new(),
                server: Mutex::new(None),
                embed_server: Mutex::new(None),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::load_all,
            commands::reschedule,
            commands::save_settings,
            commands::plan_tasks,
            commands::create_task,
            commands::set_task_status,
            commands::delete_task,
            commands::delete_project,
            commands::set_project_archived,
            commands::add_event,
            commands::delete_event,
            commands::lock_block,
            commands::list_event_types,
            commands::create_event_type,
            commands::delete_event_type,
            commands::booking_slots,
            commands::create_booking,
            commands::list_habits,
            commands::create_habit,
            commands::update_habit,
            commands::toggle_habit,
            commands::delete_habit,
            commands::schedule_habit,
            commands::set_habit_scheduled,
            commands::hermes_list_notes,
            commands::hermes_add_note,
            commands::hermes_delete_note,
            commands::hermes_recall,
            commands::ensure_embeddings,
            commands::connect_google,
            commands::disconnect_google,
            commands::sync_google,
            commands::sync_calendar,
            commands::llm_status,
            commands::list_models,
            commands::model_present,
            commands::download_model,
            commands::ensure_inference,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            // Make sure we don't leave a llama-server orphaned on exit (chat + embeddings).
            if let RunEvent::Exit = event {
                if let Some(state) = app_handle.try_state::<AppState>() {
                    for slot in [&state.server, &state.embed_server] {
                        if let Ok(mut guard) = slot.lock() {
                            if let Some(mut child) = guard.take() {
                                let _ = child.kill();
                            }
                        }
                    }
                }
            }
        });
}
