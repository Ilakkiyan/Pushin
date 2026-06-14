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
mod secrets;

use commands::AppState;
use std::sync::Mutex;
use tauri::{Manager, RunEvent};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
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

            // On macOS, native traffic-light controls are the reliable way out of fullscreen/
            // maximized states. A frameless custom titlebar can leave the window trapped.
            #[cfg(target_os = "macos")]
            if let Some(window) = app.get_webview_window("main") {
                window.set_decorations(true)?;
            }

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
            commands::hermes_add_note,
            commands::hermes_recall,
            commands::ensure_embeddings,
            commands::list_pages,
            commands::get_page,
            commands::create_page,
            commands::update_page,
            commands::delete_page,
            commands::move_page,
            commands::page_backlinks,
            commands::search_pages,
            commands::page_graph,
            commands::daily_note,
            commands::link_page_entity,
            commands::unlink_page_entity,
            commands::page_entities,
            commands::entity_pages,
            commands::list_labels,
            commands::create_label,
            commands::update_label,
            commands::delete_label,
            commands::merge_labels,
            commands::set_entity_labels,
            commands::labels_for,
            commands::labels_for_entities,
            commands::quick_label,
            commands::entities_for_label,
            commands::extract_memories,
            commands::unlinked_mentions,
            commands::vault_ask,
            commands::read_markdown_dir,
            commands::capture_note,
            commands::list_inbox,
            commands::keep_inbox_note,
            commands::connect_google,
            commands::disconnect_google,
            commands::sync_google,
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
