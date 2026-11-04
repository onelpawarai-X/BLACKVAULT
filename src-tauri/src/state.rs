//! BLACKVAULT — shared application state.
//!
//! One `AppState` lives for the whole process and is handed to every command and
//! every background thread. It owns the database, the settings, the security
//! mode, the voice engine, the app-lock authorisations and the paths.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use parking_lot::{Mutex, RwLock};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::db::Db;
use crate::models::SecurityMode;
use crate::platform::applock::Authorizations;
use crate::platform::speech::SpeechEngine;
use crate::settings::Settings;
use crate::voice::router::Router;
use crate::voice::VoiceEngine;

/// The event topic every core push travels on. Mirrors `EVENTS.core` in the UI.
pub const EVENT_TOPIC: &str = "blackvault://event";

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", content = "payload", rename_all = "camelCase")]
pub enum CoreEvent {
    Status(crate::models::EngineStatus),
    Voice(crate::models::VoiceStatus),
    Mic(crate::models::MicStatus),
    Mode { mode: String, reason: Option<String> },
    Heard { text: String, is_final: bool },
    Command { phrase: String, action: String, ok: bool },
    Auth(crate::models::AuthResult),
    Event(crate::models::SecurityEvent),
    Challenge { factors: Vec<String> },
    Unlocked { token: String },
    Settings(Box<Settings>),
    Toast { title: String, body: String, severity: String },
}

pub struct AppState {
    pub db: Db,
    pub data_dir: PathBuf,
    pub log_dir: PathBuf,
    pub started: Instant,
    pub settings: RwLock<Settings>,
    pub mode: RwLock<SecurityMode>,
    pub voice: VoiceEngine,
    /// Offline word recognition (Windows SAPI). Separate from `voice`: that one
    /// proves *who* is speaking, this one works out *what* was said.
    pub speech: SpeechEngine,
    /// Routing state (wake-phrase arming) that must persist between utterances.
    pub router: Mutex<Router>,
    pub authorizations: Mutex<Authorizations>,
    /// True once a passphrase has been verified in the current unlock attempt,
    /// so the second factor can complete the unlock.
    pub partial_auth: AtomicBool,
    /// One-shot token handed out when an unlock succeeds, consumed by the UI.
    pub unlock_token: Mutex<Option<String>>,
    /// Set while the app is shutting down cleanly, so the crash marker is removed.
    pub shutting_down: AtomicBool,
    /// Whether an offline speech recogniser is usable on this machine.
    pub speech_available: AtomicBool,
    pub last_mic_level: AtomicU64,
    pub app: Mutex<Option<AppHandle>>,
    pub hello_available: AtomicBool,
}

impl AppState {
    pub fn new() -> Arc<Self> {
        let data_dir = resolve_data_dir();
        let log_dir = data_dir.join("logs");
        let db_path = data_dir.join("blackvault.db");
        let db = Db::open(&db_path).unwrap_or_else(|e| {
            log::error!("could not open database at {db_path:?}: {e}; falling back to in-memory");
            Db::open_in_memory().expect("in-memory database")
        });
        let settings = db.load_settings().unwrap_or_default();

        Arc::new(AppState {
            db,
            data_dir,
            log_dir,
            started: Instant::now(),
            settings: RwLock::new(settings),
            mode: RwLock::new(SecurityMode::Unlocked),
            voice: VoiceEngine::new(),
            speech: SpeechEngine::new(),
            router: Mutex::new(Router::new()),
            authorizations: Mutex::new(Authorizations::new()),
            partial_auth: AtomicBool::new(false),
            unlock_token: Mutex::new(None),
            shutting_down: AtomicBool::new(false),
            speech_available: AtomicBool::new(false),
            last_mic_level: AtomicU64::new(0),
            app: Mutex::new(None),
            hello_available: AtomicBool::new(false),
        })
    }

    pub fn settings(&self) -> Settings {
        self.settings.read().clone()
    }

    pub fn mode(&self) -> SecurityMode {
        *self.mode.read()
    }

    pub fn set_mode(&self, mode: SecurityMode) {
        *self.mode.write() = mode;
    }

    pub fn hello_available(&self) -> bool {
        self.hello_available.load(Ordering::Relaxed)
    }

    /// Push an event to the UI. No-op when the app handle is not yet stored.
    pub fn emit(&self, event: CoreEvent) {
        if let Some(app) = self.app.lock().as_ref() {
            if let Err(e) = app.emit(EVENT_TOPIC, event) {
                log::debug!("emit failed: {e}");
            }
        }
    }

    pub fn log_event(&self, kind: &str, severity: &str, message: &str, detail: Option<&str>) {
        match self.db.log_event(kind, severity, message, detail) {
            Ok(ev) => self.emit(CoreEvent::Event(ev)),
            Err(e) => log::warn!("could not write audit event: {e}"),
        }
    }

    pub fn toast(&self, title: &str, body: &str, severity: &str) {
        self.emit(CoreEvent::Toast {
            title: title.into(),
            body: body.into(),
            severity: severity.into(),
        });
    }

    pub fn uptime_secs(&self) -> u64 {
        self.started.elapsed().as_secs()
    }
}

/// Where BLACKVAULT keeps its database and logs: `%LOCALAPPDATA%\BLACKVAULT`,
/// or the equivalent on other platforms during development.
pub fn resolve_data_dir() -> PathBuf {
    #[cfg(windows)]
    {
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            if !local.is_empty() {
                return PathBuf::from(local).join("BLACKVAULT");
            }
        }
        if let Ok(profile) = std::env::var("USERPROFILE") {
            return PathBuf::from(profile).join("AppData").join("Local").join("BLACKVAULT");
        }
    }
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        return PathBuf::from(xdg).join("blackvault");
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".local").join("share").join("blackvault");
    }
    PathBuf::from(".").join("blackvault-data")
}

/// Called once from `lib.rs` after the Tauri app is constructed.
pub fn on_setup(app: &mut tauri::App) {
    let handle = app.handle().clone();
    let state: Arc<AppState> = app.state::<Arc<AppState>>().inner().clone();
    *state.app.lock() = Some(handle.clone());

    // Crash recovery marker.
    match crate::platform::service::mark_running(&state.data_dir) {
        Ok(true) => {
            state.log_event(
                "engine_recovery",
                "warn",
                "Recovered from an unclean shutdown",
                Some("the previous session did not exit cleanly"),
            );
        }
        Ok(false) => {}
        Err(e) => log::warn!("could not write crash marker: {e}"),
    }

    // Probe Windows Hello availability once, up front.
    let hello = crate::platform::hello::availability();
    state.hello_available.store(hello.available, Ordering::Relaxed);

    // Seed defaults on first run.
    if let Err(e) = state.db.seed_default_commands() {
        log::warn!("could not seed default commands: {e}");
    }
    let settings = state.settings();
    if let Err(e) = state.db.prune_events(settings.audit_log_retention_days) {
        log::warn!("could not prune events: {e}");
    }

    // Start the background engine (voice routing + app guard + watchdog).
    crate::engine::spawn(Arc::clone(&state));

    // The main window starts hidden so the splash is the first thing seen; show
    // it now that setup is complete.
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.set_focus();
    }

    state.log_event("engine_start", "info", "BLACKVAULT core started", None);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_dir_is_absolute_or_relative() {
        let d = resolve_data_dir();
        assert!(d.to_string_lossy().contains("blackvault") || d.to_string_lossy().contains("BLACKVAULT"));
    }

    #[test]
    fn state_starts_unlocked() {
        let s = AppState::new();
        assert_eq!(s.mode(), SecurityMode::Unlocked);
    }
}
