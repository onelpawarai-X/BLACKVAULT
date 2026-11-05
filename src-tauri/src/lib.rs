//! BLACKVAULT — the security core.
//!
//! This module owns the running system: the database, the settings, the
//! security mode, the voice engine, the app-lock watchdog and every command the
//! frontend can call. It is the only place that decides anything security
//! relevant; the UI merely reflects what happens here.
//!
//! Layout:
//!   `state`     — shared, lockable application state
//!   `commands`  — the `#[tauri::command]` surface (1:1 with `src/lib/bridge.ts`)
//!   `engine`    — the background threads: voice routing and the app guard

mod agent;
mod agent_cmds;
mod auth;
mod bridge;
mod cloud;
mod crypto;
mod db;
mod dsp;
mod error;
mod models;
mod platform;
mod settings;
mod state;
mod voice;

mod commands;
mod engine;
mod runner;

use tauri::Manager;

use state::AppState;

/// Launch the application. Called by `main.rs` on desktop and by the mobile
/// entry point when Tauri targets Android/iOS (not supported here, but the
/// signature must exist).
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    tauri::Builder::default()
        .manage(AppState::new())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--tray"]),
        ))
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            // status & settings
            commands::get_status,
            commands::get_settings,
            commands::probe_stt,
            commands::save_settings,
            commands::get_voice_status,
            commands::get_mic_status,
            commands::get_hello_status,
            // voice engine
            commands::start_voice,
            commands::stop_voice,
            commands::restart_voice,
            commands::get_voice_identity,
            commands::enroll_voice,
            commands::clear_voice_identity,
            commands::train_phrase,
            // commands
            commands::list_commands,
            commands::upsert_command,
            commands::delete_command,
            commands::simulate_command,
            // lock / unlock
            commands::lock_pc,
            commands::unlock_factors,
            commands::begin_unlock,
            commands::submit_passphrase,
            commands::submit_pin,
            commands::submit_pattern,
            commands::submit_hello,
            commands::submit_voice,
            commands::cancel_unlock,
            commands::complete_unlock,
            // authentication prompt (its own window)
            commands::answer_prompt,
            commands::dismiss_prompt,
            commands::set_passphrase,
            commands::set_pin,
            commands::clear_pin,
            commands::set_pattern,
            commands::clear_pattern,
            commands::test_pattern,
            // launcher
            commands::list_apps,
            commands::scan_installed_apps,
            commands::pick_executable,
            commands::upsert_app,
            commands::delete_app,
            commands::launch_app,
            commands::close_app,
            // protected apps
            commands::list_protected_apps,
            commands::protect_app,
            commands::unprotect_app,
            commands::set_app_lock,
            commands::update_protected_app,
            // protected items
            commands::list_protected_items,
            commands::pick_path,
            commands::protect_path,
            commands::unprotect_path,
            commands::reveal_path,
            commands::reseal_path,
            // automation
            commands::run_automation,
            commands::press_keys,
            commands::click_mouse,
            commands::scroll_mouse,
            // service & events
            commands::list_events,
            commands::clear_events,
            commands::export_audit,
            commands::service_status,
            commands::start_service,
            commands::stop_service,
            commands::set_autostart,
            commands::open_log_folder,
            commands::factory_reset,
            // desktop agent
            agent_cmds::agent_status,
            agent_cmds::agent_set_key,
            agent_cmds::agent_clear_key,
            agent_cmds::agent_has_key,
            agent_cmds::agent_conversations,
            agent_cmds::agent_history,
            agent_cmds::agent_send,
            agent_cmds::agent_interrupt,
            agent_cmds::agent_forget,
            agent_cmds::agent_pending,
            agent_cmds::agent_answer,
            agent_cmds::agent_tools,
        ])
        .setup(|app| {
            state::on_setup(app);
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the control-centre window hides it to the tray instead of
            // ending the process, so the voice engine and the app guard survive.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                match window.label() {
                    "main" => {
                        api.prevent_close();
                        let _ = window.hide();
                    }
                    "authprompt" => {
                        // Closing the prompt is a refusal, and the protected item
                        // has to be forgotten so the guard can ask again.
                        let app = window.app_handle();
                        let state = app.state::<std::sync::Arc<state::AppState>>();
                        let token = state
                            .pending_prompt
                            .lock()
                            .as_ref()
                            .map(|p| p.token.clone());
                        commands::close_prompt(&state, token.as_deref());
                    }
                    _ => {}
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("BLACKVAULT failed to start");
}
