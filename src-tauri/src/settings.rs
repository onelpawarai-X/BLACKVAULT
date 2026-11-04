//! BLACKVAULT — application settings. Stored as a single JSON row in SQLite so
//! the schema can grow without migrations. Defaults are secure by default:
//! audio is never stored, telemetry is off and cannot be turned on.

use serde::{Deserialize, Serialize};

fn t() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub theme: String,
    #[serde(default)]
    pub setup_complete: bool,
    pub accent_glow: f32,
    pub start_with_windows: bool,
    pub start_minimized: bool,
    pub minimize_to_tray: bool,
    pub always_on_voice: bool,
    pub wake_phrase_enabled: bool,
    pub wake_phrase: String,
    pub wake_phrase_sensitivity: f32,
    pub require_wake_for_commands: bool,
    pub listen_while_locked: bool,
    pub voice_feedback: bool,
    pub lock_animation: String,
    pub unlock_animation: String,
    pub hide_cursor_when_locked: bool,
    pub multi_monitor_overlay: bool,
    pub auto_relock_secs: i64,
    pub auth_rate_limit_attempts: i64,
    pub auth_rate_limit_window_secs: i64,
    pub require_second_factor: bool,
    pub second_factor: String,
    pub voiceprint_threshold: f32,
    pub store_audio: bool,
    pub telemetry: bool,
    pub audit_log_retention_days: i64,
    pub lock_on_idle_secs: i64,
    pub lock_on_sleep: bool,
    pub lock_on_screen_saver: bool,
    pub block_task_switch_when_locked: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: "black".into(),
            setup_complete: false,
            accent_glow: 0.55,
            start_with_windows: true,
            start_minimized: false,
            minimize_to_tray: true,
            always_on_voice: true,
            wake_phrase_enabled: true,
            wake_phrase: "black vault".into(),
            wake_phrase_sensitivity: 0.6,
            require_wake_for_commands: true,
            listen_while_locked: true,
            voice_feedback: false,
            lock_animation: "iris".into(),
            unlock_animation: "bloom".into(),
            hide_cursor_when_locked: true,
            multi_monitor_overlay: true,
            auto_relock_secs: 0,
            auth_rate_limit_attempts: 5,
            auth_rate_limit_window_secs: 60,
            require_second_factor: true,
            second_factor: "pin".into(),
            voiceprint_threshold: 0.62,
            store_audio: false,
            telemetry: false,
            audit_log_retention_days: 90,
            lock_on_idle_secs: 0,
            lock_on_sleep: true,
            lock_on_screen_saver: true,
            block_task_switch_when_locked: false,
        }
    }
}

impl Settings {
    /// Normalise values that arrive from the UI so nothing out of range is kept.
    pub fn sanitize(mut self) -> Self {
        self.telemetry = false;
        self.store_audio = false;
        self.accent_glow = self.accent_glow.clamp(0.0, 1.0);
        self.wake_phrase_sensitivity = self.wake_phrase_sensitivity.clamp(0.1, 0.95);
        self.voiceprint_threshold = self.voiceprint_threshold.clamp(0.3, 0.95);
        self.auth_rate_limit_attempts = self.auth_rate_limit_attempts.clamp(1, 50);
        self.auth_rate_limit_window_secs = self.auth_rate_limit_window_secs.clamp(5, 3600);
        self.audit_log_retention_days = self.audit_log_retention_days.clamp(1, 3650);
        self.auto_relock_secs = self.auto_relock_secs.clamp(0, 86400);
        self.lock_on_idle_secs = self.lock_on_idle_secs.clamp(0, 86400);
        if self.wake_phrase.trim().is_empty() {
            self.wake_phrase = "black vault".into();
        }
        match self.second_factor.as_str() {
            "pin" | "pattern" | "hello" | "voiceprint" | "passphrase" => {}
            _ => self.second_factor = "pin".into(),
        }
        match self.lock_animation.as_str() {
            "iris" | "shutter" | "dissolve" => {}
            _ => self.lock_animation = "iris".into(),
        }
        match self.unlock_animation.as_str() {
            "bloom" | "rings" | "vortex" => {}
            _ => self.unlock_animation = "bloom".into(),
        }
        self
    }
}

// `t` keeps the helper available for future defaulted fields without warnings.
#[allow(dead_code)]
pub fn always_true() -> bool {
    t()
}
