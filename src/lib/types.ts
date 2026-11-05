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
  stt: SttStatus;
}

/** Health of the ZYRAXON speech-to-text bridge. */
export interface SttStatus {
  enabled: boolean;
  running: boolean;
  busy: boolean;
  endpoint: string;
  language: string;
  live: boolean;
  recognised: number;
  lastText: string | null;
  lastError: string | null;
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
  ownerName: string;
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
  sttEnabled: boolean;
  sttEndpoint: string;
  sttLanguage: string;
  sttLive: boolean;
  sttMinConfidence: number;
  /** Opt-in Windows offline recogniser. Off by default; the endpoint does the work. */
  offlineRecognition: boolean;
  /** `prompt` asks for a password when the item is opened; `hard` denies reading it. */
  fileLockMode: "prompt" | "hard";
  /** Stop a protected application the moment it starts. */
  appGuardEnabled: boolean;
  /** Seconds an unauthorised application may run while the prompt is up. */
  appLockGraceSecs: number;
  /** Ask for a password when a protected item is opened. */
  fileGuardEnabled: boolean;
  /** Let the cloud agent drive this computer. */
  agentEnabled: boolean;
  /** Ask before a sensitive tool runs (a command, a file read, a release). */
  agentConfirmSensitive: boolean;
  /** The model the agent uses. `null` means the built-in default. */
  cloudModel: string | null;
  /** Keep recently used sandboxes awake for an instant next message. */
  agentKeepalive: boolean;
  /** Speak the agent's answers aloud. */
  agentSpeak: boolean;
  /** The owner's standing instructions for the agent, kept on this machine. */
  agentInstructions: string;
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

// ============================ Desktop agent ==============================

/** The agent's live state, mirrored from the Rust runner. */
export interface AgentState {
  busy: boolean;
  phase: "preparing" | "working" | "awaiting_tool" | "done" | string;
  stage: string;
  activeLocalId: string | null;
  activeRemoteId: string | null;
  conversations: number;
  warmSandboxes: number;
  lastError: string | null;
  model: string;
  keyPresent: boolean;
}

/** One rendered line of an agent conversation. */
export interface AgentMessage {
  id: string;
  localId: string;
  kind:
    | "message"
    | "action"
    | "observation"
    | "usage"
    | "status"
    | "error"
    | "delta"
    | "exec_status"
    | "other";
  role: string;
  text: string;
  thought: string;
  tool: string;
  summary: string;
  /** A tool call's arguments, as the service sent them. */
  arguments?: Record<string, unknown>;
  ok: boolean;
  at: string;
}

/** The same shape as an event pushed live, before it is stored as a message. */
export type AgentEvent = Omit<AgentMessage, "localId">;

/** A chat on this desktop and the cloud conversation behind it. */
export interface AgentConversation {
  localId: string;
  remoteId: string;
  title: string;
  sandboxId: string | null;
  lastSeen: number;
}

/** A tool the agent may use on this computer. */
export interface AgentTool {
  name: string;
  description: string;
  sensitive: boolean;
  params: { name: string; type: string; required: boolean }[];
}
