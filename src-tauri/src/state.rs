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
use crate::platform::stt::SttWorker;
use crate::settings::Settings;
use crate::voice::router::Router;
use crate::voice::VoiceEngine;

/// The event topic every core push travels on. Mirrors `EVENTS.core` in the UI.
pub const EVENT_TOPIC: &str = "blackvault://event";

/// What an open authentication prompt is protecting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptTarget {
    App { rule_id: i64, process: String },
    Path { item_id: i64, path: String },
}

#[derive(Debug, Clone)]
pub struct PendingPrompt {
    pub token: String,
    pub target: PromptTarget,
    pub raised_at: Instant,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", content = "payload", rename_all = "camelCase")]
pub enum CoreEvent {
    Status(crate::models::EngineStatus),
    Voice(crate::models::VoiceStatus),
    Mic(crate::models::MicStatus),
    Mode { mode: String, reason: Option<String> },
    #[serde(rename_all = "camelCase")]
    Heard { text: String, is_final: bool },
    Command { phrase: String, action: String, ok: bool },
    Auth(crate::models::AuthResult),
    Event(crate::models::SecurityEvent),
    Challenge { factors: Vec<String> },
    /// A protected application is running without authorisation. The UI raises
    /// the authentication prompt; the guard will not kill the process until the
    /// grace period below has passed.
    #[serde(rename_all = "camelCase")]
    AppChallenge {
        rule_id: i64,
        process_name: String,
        display_name: String,
        /// Seconds left before the process is terminated.
        grace_secs: u64,
        /// Where the application was started from, when it is known. The prompt
        /// uses it to show which copy is asking.
        origin: Option<String>,
        /// The single-use token this prompt may be answered with.
        token: String,
    },
    /// A protected file or folder was opened. The prompt is raised in its own
    /// window so the user authenticates at the point of use, wherever the
    /// control centre happens to be.
    #[serde(rename_all = "camelCase")]
    PathChallenge {
        token: String,
        item_id: i64,
        path: String,
        display_name: String,
        kind: String,
        /// Where the user tried to open it from, when Windows reports it.
        origin: Option<String>,
    },
    /// The prompt is finished and its window should close.
    PromptClosed { token: String },
    Unlocked { token: String },
    /// The agent's own state changed (busy, phase, stage, conversation count).
    Agent(Box<crate::runner::AgentState>),
    /// One rendered line of an agent conversation.
    #[serde(rename_all = "camelCase")]
    AgentEvent {
        local_id: String,
        event: crate::cloud::AgentEvent,
    },
    /// The agent asked for a tool that needs the user's approval.
    #[serde(rename_all = "camelCase")]
    AgentToolChallenge {
        local_id: String,
        tool: String,
        summary: String,
        token: String,
    },
    /// The agent's socket dropped and is being re-joined.
    #[serde(rename_all = "camelCase")]
    AgentStatus { local_id: String, status: String },
    /// The agent asked BLACKVAULT to speak.
    AgentSpeak { text: String },
    /// The microphone heard words while dictation is on. The UI appends them to
    /// the message box instead of the core treating them as a command.
    #[serde(rename_all = "camelCase")]
    Dictated { text: String, is_final: bool },
    /// The whole transcript of a chat, straight from the service.
    ///
    /// Emitted when a turn ends. The live stream is the fast path, but a cold
    /// start sends the prompt as the conversation's own initial message, so a
    /// quick answer can be produced before the socket is open — the first reply
    /// then existed only on the server. This replaces the pane's transcript with
    /// the authoritative one, so nothing the user said is ever missing.
    #[serde(rename_all = "camelCase")]
    AgentTranscript {
        local_id: String,
        messages: Vec<crate::runner::AgentMessage>,
    },
    Settings(Box<Settings>),
    Toast { title: String, body: String, severity: String },
}

/// The state of one protected process that is running without authorisation.
pub struct PendingBlock {
    /// When it was first seen unauthorised — the grace window starts here.
    pub first_seen: Instant,
    /// When its prompt was last raised. `None` until the first prompt goes up,
    /// so the first tick raises the card immediately rather than after a repeat
    /// interval — which is what made the app look as though it never asked.
    pub last_ask: Option<Instant>,
    /// Whether a prompt has ever been raised for this sighting.
    pub asked: bool,
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
    /// Word recognition through the ZYRAXON STT endpoint. This is the path that
    /// works on every machine, including those with no SAPI recogniser.
    pub stt: SttWorker,
    /// Routing state (wake-phrase arming) that must persist between utterances.
    pub router: Mutex<Router>,
    pub authorizations: Mutex<Authorizations>,
    /// The last command that actually ran, when it ran, and which recogniser
    /// reported it. Two recognisers (offline SAPI and the STT endpoint) can hear
    /// the same words, and a command must never fire twice for one utterance —
    /// but the same phrase twice from one recogniser is a deliberate repeat.
    pub last_command: Mutex<Option<(String, Instant, crate::engine::Recogniser)>>,
    /// When each blocked process was first seen running unauthorised, and when
    /// its authentication prompt was last raised. Drives the grace period so a
    /// protected app is prompted for, not silently killed — and the prompt is
    /// raised on the *first* sighting, not one repeat-interval later.
    /// Process name -> state.
    pub pending_blocks: Mutex<std::collections::HashMap<String, PendingBlock>>,
    /// The authentication prompt that is open right now, if any. Its token is
    /// regenerated every time a prompt is raised, so a PIN entered for one
    /// protected application or file can never release a different one.
    pub pending_prompt: Mutex<Option<PendingPrompt>>,
    /// Last time each protected item was seen to change, for the prompt-on-open
    /// guard. A baseline is recorded on the first sighting, so adding an item
    /// never fires a prompt.
    pub file_seen: Mutex<std::collections::HashMap<String, std::time::SystemTime>>,
    /// True once a passphrase has been verified in the current unlock attempt,
    /// so the second factor can complete the unlock.
    pub partial_auth: AtomicBool,
    /// True while the mic button is dictating into the agent's message box.
    /// Utterances are then transcribed and sent to the UI as text rather than
    /// being matched against the command list.
    pub dictating: AtomicBool,
    /// One-shot token handed out when an unlock succeeds, consumed by the UI.
    pub unlock_token: Mutex<Option<String>>,
    /// Set while the app is shutting down cleanly, so the crash marker is removed.
    pub shutting_down: AtomicBool,
    /// Whether an offline speech recogniser is usable on this machine.
    pub speech_available: AtomicBool,
    pub last_mic_level: AtomicU64,
    pub app: Mutex<Option<AppHandle>>,
    pub hello_available: AtomicBool,
    /// The desktop agent: the cloud link, the conversation registry and the
    /// tool runner. Set once at start-up; `None` only before that.
    pub agent: Mutex<Option<Arc<crate::runner::AgentRuntime>>>,
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
            stt: SttWorker::new(),
            router: Mutex::new(Router::new()),
            authorizations: Mutex::new(Authorizations::new()),
            last_command: Mutex::new(None),
            pending_blocks: Mutex::new(std::collections::HashMap::new()),
            pending_prompt: Mutex::new(None),
            file_seen: Mutex::new(std::collections::HashMap::new()),
            partial_auth: AtomicBool::new(false),
            dictating: AtomicBool::new(false),
            unlock_token: Mutex::new(None),
            shutting_down: AtomicBool::new(false),
            speech_available: AtomicBool::new(false),
            last_mic_level: AtomicU64::new(0),
            app: Mutex::new(None),
            hello_available: AtomicBool::new(false),
            agent: Mutex::new(None),
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

    // Start the desktop agent. It is created even without a cloud key so the
    // pane can explain what is missing rather than showing nothing.
    let agent = crate::runner::AgentRuntime::new(Arc::clone(&state));
    *state.agent.lock() = Some(Arc::clone(&agent));
    if settings.agent_keepalive {
        crate::runner::spawn_keepalive(agent);
    }

    // The main window is created hidden. Whether it is revealed depends on the
    // two ways a user can ask for a quiet start: the "start minimised" setting,
    // or the `--tray` argument the autostart entry passes at sign-in. Without
    // this the window always popped up at login, which made both the setting and
    // the argument look like they did nothing.
    if !starts_in_tray(&settings) {
        if let Some(win) = app.get_webview_window("main") {
            let _ = win.show();
            let _ = win.set_focus();
        }
    } else {
        log::info!("starting in the tray; the control centre stays hidden");
    }

    state.log_event("engine_start", "info", "BLACKVAULT core started", None);
}

/// Did the process start with the `--tray` argument?
///
/// The autostart entry passes it so a sign-in launch never pops a window.
fn tray_requested() -> bool {
    std::env::args().any(|a| a == "--tray")
}

/// Should the control centre stay in the tray on this launch?
///
/// True when the user asked to start minimised, or when `--tray` was passed.
/// The argument wins, so a sign-in launch is always quiet even if the setting
/// was later changed in the UI.
fn starts_in_tray(settings: &crate::settings::Settings) -> bool {
    tray_requested() || settings.start_minimized
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

    /// Every field the UI reads must reach it in camelCase.
    ///
    /// `rename_all` on the enum renames the variants but *not* the fields of a
    /// struct variant — each one needs its own attribute. Without them
    /// `local_id` went out as `local_id` while the frontend read `localId`,
    /// which is `undefined`, so every live agent event was silently dropped and
    /// the pane showed nothing at all.
    #[test]
    fn core_event_fields_are_camel_case() {
        let event = CoreEvent::AgentEvent {
            local_id: "chat-1".into(),
            event: crate::cloud::AgentEvent::default(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"localId\""), "agent event: {json}");

        let heard = CoreEvent::Heard {
            text: "hi".into(),
            is_final: true,
        };
        let json = serde_json::to_string(&heard).unwrap();
        assert!(json.contains("\"isFinal\""), "heard: {json}");

        let challenge = CoreEvent::AppChallenge {
            rule_id: 1,
            process_name: "x.exe".into(),
            display_name: "X".into(),
            grace_secs: 5,
            origin: None,
            token: "t".into(),
        };
        let json = serde_json::to_string(&challenge).unwrap();
        assert!(json.contains("\"ruleId\""), "app challenge: {json}");
        assert!(json.contains("\"processName\""), "app challenge: {json}");

        let path = CoreEvent::PathChallenge {
            token: "t".into(),
            item_id: 2,
            path: "C:\\x".into(),
            display_name: "X".into(),
            kind: "file".into(),
            origin: None,
        };
        let json = serde_json::to_string(&path).unwrap();
        assert!(json.contains("\"itemId\""), "path challenge: {json}");
        assert!(json.contains("\"displayName\""), "path challenge: {json}");

        // The dictation and transcript events carry fields the frontend reads by
        // name, so the same trap applies to them.
        let dictated = CoreEvent::Dictated {
            text: "hello".into(),
            is_final: true,
        };
        let json = serde_json::to_string(&dictated).unwrap();
        assert!(json.contains("\"dictated\""), "dictated type: {json}");
        assert!(json.contains("\"isFinal\""), "dictated: {json}");

        let transcript = CoreEvent::AgentTranscript {
            local_id: "chat-1".into(),
            messages: Vec::new(),
        };
        let json = serde_json::to_string(&transcript).unwrap();
        assert!(json.contains("\"agentTranscript\""), "transcript type: {json}");
        assert!(json.contains("\"localId\""), "transcript: {json}");
    }
}
