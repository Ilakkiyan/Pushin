mod booking;
mod booking_server;
mod briefing;
mod calendar;
mod commands;
mod context;
// Public so the LLM-evaluation harness (`tests/llm_eval.rs`) can drive the real parsing
// pipeline against a live llama-server. No external consumers otherwise.
pub mod db;
mod habits;
mod hermes;
mod llm;
mod meeting;
pub mod model;
mod model_manager;
pub mod parser;
mod scheduler;
// Public so the model regression battery (`tests/model_battery.rs`) can run the same post-plan
// reschedule the app does, then project the resulting calendar. No external consumers otherwise.
pub mod schedule_service;
mod secrets;
mod sync;
mod vault;

use commands::AppState;
use std::sync::{Arc, Mutex};
use tauri::{Manager, RunEvent};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init());

    // In-app auto-update from GitHub Releases. Desktop-only: the updater swaps the installed app
    // bundle (user data in the app-data dir is untouched) and `process` provides relaunch() after
    // install. Mobile updates ship via the app stores, so the plugins aren't built there (Cargo.toml).
    // cfg-gated shadowing (not `mut`) so the mobile build stays warning-free.
    #[cfg(desktop)]
    let builder = builder
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init());

    builder
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
                db: Arc::new(Mutex::new(conn)),
                http: reqwest::Client::new(),
                server: Mutex::new(None),
                embed_server: Mutex::new(None),
                booking_server: Mutex::new(None),
                sync_engine: Mutex::new(None),
                vault_watcher: Mutex::new(None),
                vault_echo: Default::default(),
            });

            // If a two-way vault folder is already configured, start watching it (files → DB). No-op
            // when unset; never blocks startup.
            if let Some(state) = handle.try_state::<AppState>() {
                commands::start_vault_watch(handle, state.inner());
            }

            // If this device has already joined a sync network, bring the mesh engine up in the
            // background (best-effort — sync failing must never block app startup).
            if sync::identity::mesh_secret().is_some() {
                let handle = handle.clone();
                tauri::async_runtime::spawn(async move {
                    if let Some(state) = handle.try_state::<AppState>() {
                        if let Err(e) = commands::ensure_engine(handle.clone(), state.inner()).await {
                            eprintln!("sync: engine did not start: {e}");
                        }
                    }
                });
            }

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
            commands::vault_write,
            commands::vault_page_for_path,
            commands::vault_link_path,
            commands::vault_unlink_path,
            commands::vault_refresh_watch,
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
            commands::update_event_type,
            commands::regenerate_event_type_token,
            commands::delete_event_type,
            commands::booking_server_status,
            commands::start_booking_server,
            commands::stop_booking_server,
            commands::booking_slots,
            commands::create_booking,
            commands::cancel_booking,
            commands::list_habits,
            commands::create_habit,
            commands::update_habit,
            commands::toggle_habit,
            commands::delete_habit,
            commands::schedule_habit,
            commands::set_habit_scheduled,
            commands::move_habit,
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
            commands::assistant_chat,
            commands::route_intent,
            commands::list_people,
            commands::get_person,
            commands::create_person,
            commands::update_person,
            commands::delete_person,
            commands::daily_briefing,
            commands::meeting_brief,
            commands::extract_action_items,
            commands::start_focus,
            commands::stop_focus,
            commands::active_focus,
            commands::task_focus_minutes,
            commands::suggest_labels,
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
            commands::recommend_model,
            commands::download_model,
            commands::ensure_inference,
            commands::sync_status,
            commands::sync_create_invite,
            commands::sync_join,
            commands::sync_now,
            commands::sync_remove_peer,
            commands::sync_set_device_name,
            commands::sync_set_relay,
            commands::sync_leave,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            // Make sure we don't leave a llama-server orphaned on exit (chat + embeddings).
            if let RunEvent::Exit = event {
                if let Some(state) = app_handle.try_state::<AppState>() {
                    // Drop the sync engine (its Iroh endpoint closes on drop).
                    if let Ok(mut guard) = state.sync_engine.lock() {
                        guard.take();
                    }
                    // Stop the vault file watcher.
                    if let Ok(mut guard) = state.vault_watcher.lock() {
                        guard.take();
                    }
                    if let Ok(mut guard) = state.booking_server.lock() {
                        if let Some(server) = guard.take() {
                            server.stop();
                        }
                    }
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
