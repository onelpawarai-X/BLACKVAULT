// BLACKVAULT — shared type contracts between the frontend and the Rust core.
// Every type here mirrors a serde type in `src-tauri/src`.

export type SecurityMode = "unlocked" | "locked" | "locking" | "unlocking";

export interface EngineStatus {
  mode: SecurityMode;
  voice: VoiceStatus;
  mic: MicStatus;
  hello: HelloStatus;
  pinConfigured: boolean;
  patternConfigured: boolean;
  passphraseConfigured: boolean;
  serviceRunning: boolean;
  protectedApps: number;
  protectedFiles: number;
  protectedFolders: number;
  uptimeSecs: number;
  lastAuth: string | null;
  speechAvailable: boolean;
}

export interface VoiceStatus {
  running: boolean;
  wakePhrase: string | null;
  listening: boolean;
  lastHeard: string | null;
  lastCommand: string | null;
  recoveryAttempts: number;
  model: string | null;
  backend: "vosk" | "sherpa" | "windows" | "none";
}

export interface MicStatus {
  deviceName: string | null;
  level: number; // 0..1 live input level
  permission: "granted" | "denied" | "unknown";
  muted: boolean;
}

export interface HelloStatus {
  available: boolean;
  enrolled: boolean;
  method: string | null; // "fingerprint" | "pin" | "face" | ...
  lastVerified: string | null;
}

export interface VoiceCommand {
  id: number;
  phrase: string;
  action: string;
  actionType: CommandActionType;
  payload: string | null;
  enabled: boolean;
  requiresAuth: boolean;
  builtin: boolean;
  createdAt: string;
  hitCount: number;
}

export type CommandActionType =
  | "lock"
  | "unlock"
  | "launch_app"
  | "close_app"
  | "open_url"
  | "key_press"
  | "mouse_click"
  | "scroll"
  | "media_next"
  | "media_prev"
  | "media_play_pause"
  | "volume_up"
  | "volume_down"
  | "volume_mute"
  | "run_program"
  | "open_folder"
  | "minimize_all"
  | "show_desktop"
  | "screenshot"
  | "custom_script";

export interface LauncherApp {
  id: number;
  name: string;
  command: string;
  args: string | null;
  cwd: string | null;
  voicePhrase: string | null;
  enabled: boolean;
  iconPath: string | null;
  launchCount: number;
  lastLaunched: string | null;
}

export interface ProtectedApp {
  id: number;
  exePath: string;
  displayName: string;
  processName: string;
  requireAuth: boolean;
  autoRelock: boolean;
  relockAfterSecs: number;
  locked: boolean;
  createdAt: string;
}

export interface ProtectedItem {
  id: number;
  path: string;
  kind: "file" | "folder";
  displayName: string;
  encryption: "acl" | "efs" | "dpapi" | "acl+efs";
  protected: boolean;
  lastAccess: string | null;
  createdAt: string;
  sizeBytes: number | null;
}

export interface SecurityEvent {
  id: number;
  at: string;
  kind: SecurityEventKind;
  severity: "info" | "warn" | "critical";
  message: string;
  detail: string | null;
}

export type SecurityEventKind =
  | "lock"
  | "unlock"
  | "auth_success"
  | "auth_failure"
  | "voice_command"
  | "app_launch"
  | "app_blocked"
  | "file_protected"
  | "file_unprotected"
  | "setting_changed"
  | "engine_start"
  | "engine_stop"
  | "engine_recovery"
  | "mic_changed"
  | "pattern_set"
  | "pin_set"
  | "passphrase_set"
  | "hello_enrolled"
  | "rate_limited"
  | "error";

export interface Settings {
  theme: "black";
  setupComplete: boolean;
  accentGlow: number; // 0..1 intensity
  startWithWindows: boolean;
  startMinimized: boolean;
  minimizeToTray: boolean;
  alwaysOnVoice: boolean;
  wakePhraseEnabled: boolean;
  wakePhrase: string;
  wakePhraseSensitivity: number; // 0..1
  requireWakeForCommands: boolean;
  listenWhileLocked: boolean;
  voiceFeedback: boolean;
  lockAnimation: "iris" | "shutter" | "dissolve";
  unlockAnimation: "bloom" | "rings" | "vortex";
  hideCursorWhenLocked: boolean;
  multiMonitorOverlay: boolean;
  autoRelockSecs: number;
  authRateLimitAttempts: number;
  authRateLimitWindowSecs: number;
  requireSecondFactor: boolean;
  secondFactor: "pin" | "pattern" | "hello" | "voiceprint" | "passphrase";
  voiceprintThreshold: number;
  storeAudio: boolean;
  telemetry: false;
  auditLogRetentionDays: number;
  lockOnIdleSecs: number;
  lockOnSleep: boolean;
  lockOnScreenSaver: boolean;
  blockTaskSwitchWhenLocked: boolean;
}

export interface VoiceIdentity {
  enrolled: boolean;
  sampleCount: number;
  threshold: number;
  updatedAt: string | null;
}

export interface AuthResult {
  ok: boolean;
  method: string;
  message: string;
  score: number | null;
  locked: boolean;
  remainingAttempts: number | null;
}

export interface DeviceScanResult {
  exePath: string;
  name: string;
  publisher: string | null;
  version: string | null;
}

export interface AutomationRequest {
  action: CommandActionType;
  payload?: string | null;
}

export interface PatternResult {
  ok: boolean;
  message: string;
}
