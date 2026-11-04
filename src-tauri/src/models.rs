//! BLACKVAULT — the data contracts shared with the frontend.
//! Field names are camelCase on the wire to match `src/lib/types.ts`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SecurityMode {
    Unlocked,
    Locking,
    Locked,
    Unlocking,
}

impl SecurityMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            SecurityMode::Unlocked => "unlocked",
            SecurityMode::Locking => "locking",
            SecurityMode::Locked => "locked",
            SecurityMode::Unlocking => "unlocking",
        }
    }
    pub fn parse(s: &str) -> SecurityMode {
        match s {
            "locked" => SecurityMode::Locked,
            "locking" => SecurityMode::Locking,
            "unlocking" => SecurityMode::Unlocking,
            _ => SecurityMode::Unlocked,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceStatus {
    pub running: bool,
    pub wake_phrase: Option<String>,
    pub listening: bool,
    pub last_heard: Option<String>,
    pub last_command: Option<String>,
    pub recovery_attempts: u32,
    pub model: Option<String>,
    pub backend: String,
}

impl Default for VoiceStatus {
    fn default() -> Self {
        VoiceStatus {
            running: false,
            wake_phrase: None,
            listening: false,
            last_heard: None,
            last_command: None,
            recovery_attempts: 0,
            model: None,
            backend: "none".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MicStatus {
    pub device_name: Option<String>,
    pub level: f32,
    pub permission: String,
    pub muted: bool,
}

impl Default for MicStatus {
    fn default() -> Self {
        MicStatus {
            device_name: None,
            level: 0.0,
            permission: "unknown".into(),
            muted: false,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HelloStatus {
    pub available: bool,
    pub enrolled: bool,
    pub method: Option<String>,
    pub last_verified: Option<String>,
}

impl Default for HelloStatus {
    fn default() -> Self {
        HelloStatus {
            available: false,
            enrolled: false,
            method: None,
            last_verified: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineStatus {
    pub mode: SecurityMode,
    pub voice: VoiceStatus,
    pub mic: MicStatus,
    pub hello: HelloStatus,
    pub pin_configured: bool,
    pub pattern_configured: bool,
    pub passphrase_configured: bool,
    pub service_running: bool,
    pub protected_apps: i64,
    pub protected_files: i64,
    pub protected_folders: i64,
    pub uptime_secs: u64,
    pub last_auth: Option<String>,
    /// Whether an offline speech recogniser is installed and usable. When false
    /// the UI must say so rather than implying voice commands will work.
    pub speech_available: bool,
    /// The word recogniser that goes through the ZYRAXON STT endpoint.
    pub stt: SttStatus,
}

/// Health of the STT bridge, shown on the dashboard.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SttStatus {
    pub enabled: bool,
    pub running: bool,
    pub busy: bool,
    pub endpoint: String,
    pub language: String,
    pub live: bool,
    /// Utterances handed to the recogniser since it started.
    pub recognised: u64,
    pub last_text: Option<String>,
    pub last_error: Option<String>,
}

impl Default for SttStatus {
    fn default() -> Self {
        SttStatus {
            enabled: false,
            running: false,
            busy: false,
            endpoint: crate::platform::stt::DEFAULT_ENDPOINT.into(),
            language: "en".into(),
            live: false,
            recognised: 0,
            last_text: None,
            last_error: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceCommand {
    pub id: i64,
    pub phrase: String,
    pub action: String,
    pub action_type: String,
    pub payload: Option<String>,
    pub enabled: bool,
    pub requires_auth: bool,
    pub builtin: bool,
    pub created_at: String,
    pub hit_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LauncherApp {
    #[serde(default)]
    pub id: i64,
    pub name: String,
    pub command: String,
    pub args: Option<String>,
    pub cwd: Option<String>,
    pub voice_phrase: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub icon_path: Option<String>,
    #[serde(default)]
    pub launch_count: i64,
    pub last_launched: Option<String>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtectedApp {
    pub id: i64,
    pub exe_path: String,
    pub display_name: String,
    pub process_name: String,
    pub require_auth: bool,
    pub auto_relock: bool,
    pub relock_after_secs: i64,
    pub locked: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtectedItem {
    pub id: i64,
    pub path: String,
    pub kind: String,
    pub display_name: String,
    pub encryption: String,
    pub protected: bool,
    pub last_access: Option<String>,
    pub created_at: String,
    pub size_bytes: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecurityEvent {
    pub id: i64,
    pub at: String,
    pub kind: String,
    pub severity: String,
    pub message: String,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceIdentity {
    pub enrolled: bool,
    pub sample_count: i64,
    pub threshold: f32,
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthResult {
    pub ok: bool,
    pub method: String,
    pub message: String,
    pub score: Option<f32>,
    pub locked: bool,
    pub remaining_attempts: Option<i64>,
}

impl AuthResult {
    pub fn fail(method: &str, message: impl Into<String>, remaining: Option<i64>) -> Self {
        AuthResult {
            ok: false,
            method: method.into(),
            message: message.into(),
            score: None,
            locked: true,
            remaining_attempts: remaining,
        }
    }
    pub fn ok(method: &str, message: impl Into<String>) -> Self {
        AuthResult {
            ok: true,
            method: method.into(),
            message: message.into(),
            score: None,
            locked: false,
            remaining_attempts: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PatternResult {
    pub ok: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceScanResult {
    pub exe_path: String,
    pub name: String,
    pub publisher: Option<String>,
    pub version: Option<String>,
}
