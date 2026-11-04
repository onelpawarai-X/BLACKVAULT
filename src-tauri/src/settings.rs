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
    /// The name the user is addressed by. Purely cosmetic — it labels the voice
    /// profile and greets them on the lock screen. Never leaves the machine.
    #[serde(default)]
    pub owner_name: String,
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
    /// Recognise spoken words through the ZYRAXON STT endpoint. This is the path
    /// that works on machines with no offline recogniser installed.
    #[serde(default = "t")]
    pub stt_enabled: bool,
    #[serde(default = "default_stt_endpoint")]
    pub stt_endpoint: String,
    /// `en` or `bn`. The service does not detect the language, so it is explicit.
    #[serde(default = "default_stt_language")]
    pub stt_language: String,
    /// Stream the audio while the person is still speaking, instead of waiting
    /// for the utterance to end. Lower latency, one long request.
    #[serde(default)]
    pub stt_live: bool,
    /// Transcripts below this confidence are ignored. 0 accepts everything.
    #[serde(default)]
    pub stt_min_confidence: f32,

    /// Watch the process table for applications that have been put in the
    /// protected list and bring them to a halt when they start unauthorised.
    pub app_guard_enabled: bool,
    /// Seconds a protected application may run while its authentication prompt
    /// is up. The prompt appears as an overlay on the application itself, so
    /// the user authenticates where they are rather than in BLACKVAULT.
    pub app_lock_grace_secs: u64,
    /// Keep a protected file or folder unopenable, and offer the password at the
    /// moment it is opened rather than inside BLACKVAULT.
    pub file_guard_enabled: bool,

    /// Whether the offline Windows recogniser runs at all. Off by default:
    /// BLACKVAULT recognises speech through its own endpoint, and starting the
    /// Windows recogniser as well made the machine feel like it had stopped.
    pub offline_recognition: bool,

    /// How hard a protected file or folder is sealed.
    ///
    /// `prompt` (the default) denies write and delete but leaves read alone, so
    /// opening the item is what raises the password prompt. `hard` denies read
    /// as well, which is the strongest seal Windows offers but leaves the user
    /// with nothing but "access is denied" and no way to be asked.
    pub file_lock_mode: String,
}

fn default_stt_endpoint() -> String {
    crate::platform::stt::DEFAULT_ENDPOINT.into()
}

fn default_stt_language() -> String {
    "en".into()
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: "black".into(),
            setup_complete: false,
            owner_name: String::new(),
            accent_glow: 0.55,
            start_with_windows: true,
            start_minimized: false,
            minimize_to_tray: true,
            always_on_voice: true,
            wake_phrase_enabled: true,
            wake_phrase: "black vault".into(),
            wake_phrase_sensitivity: 0.6,
            // Off by default: requiring the wake phrase before *every* command
            // is the main reason a first-time user thinks voice control is
            // broken. It stays available as an opt-in in Settings.
            require_wake_for_commands: false,
            listen_while_locked: true,
            voice_feedback: false,
            lock_animation: "iris".into(),
            unlock_animation: "bloom".into(),
            // The overlay needs a pointer for the PIN field and the pattern
            // grid; hiding it made the lock screen unusable.
            hide_cursor_when_locked: false,
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
            stt_enabled: true,
            stt_endpoint: default_stt_endpoint(),
            stt_language: default_stt_language(),
            stt_live: false,
            stt_min_confidence: 0.0,
            app_guard_enabled: true,
            app_lock_grace_secs: 12,
            file_guard_enabled: true,
            offline_recognition: false,
            file_lock_mode: "prompt".into(),
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
        if self.stt_endpoint.trim().is_empty() {
            self.stt_endpoint = default_stt_endpoint();
        }
        self.app_lock_grace_secs = self.app_lock_grace_secs.clamp(5, 300);
        if self.file_lock_mode != "hard" {
            self.file_lock_mode = "prompt".into();
        }
        match self.stt_language.trim().to_ascii_lowercase().as_str() {
            "bn" | "bn-bd" | "bengali" => self.stt_language = "bn".into(),
            "both" | "auto" => self.stt_language = "both".into(),
            _ => self.stt_language = "en".into(),
        }
        self.stt_min_confidence = self.stt_min_confidence.clamp(0.0, 1.0);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_leave_the_wake_phrase_optional() {
        // Requiring the wake phrase before every command is the usual reason a
        // first-time user concludes voice control is broken.
        let d = Settings::default();
        assert!(!d.require_wake_for_commands);
        assert!(d.wake_phrase_enabled);
    }

    #[test]
    fn defaults_never_hide_the_cursor() {
        // The overlay carries the PIN field and the pattern grid.
        assert!(!Settings::default().hide_cursor_when_locked);
    }

    #[test]
    fn sanitize_pins_second_factor() {
        let mut s = Settings::default();
        s.second_factor = "  magic  ".into();
        assert_eq!(s.sanitize().second_factor, "pin");
        let mut ok = Settings::default();
        ok.second_factor = "pattern".into();
        assert_eq!(ok.sanitize().second_factor, "pattern");
    }

    #[test]
    fn sanitize_restores_an_empty_wake_phrase() {
        let mut s = Settings::default();
        s.wake_phrase = "   ".into();
        assert_eq!(s.sanitize().wake_phrase, "black vault");
    }

    #[test]
    fn sanitize_keeps_privacy_off() {
        let mut s = Settings::default();
        s.telemetry = true;
        s.store_audio = true;
        let s = s.sanitize();
        assert!(!s.telemetry);
        assert!(!s.store_audio);
    }

    #[test]
    fn sanitize_clamps_rate_limits() {
        let mut s = Settings::default();
        s.auth_rate_limit_attempts = 0;
        s.auth_rate_limit_window_secs = 1;
        let s = s.sanitize();
        assert_eq!(s.auth_rate_limit_attempts, 1);
        assert_eq!(s.auth_rate_limit_window_secs, 5);
    }
}

// `t` keeps the helper available for future defaulted fields without warnings.
#[allow(dead_code)]
pub fn always_true() -> bool {
    t()
}
