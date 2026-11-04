// BLACKVAULT — the single IPC boundary.
// Every call into the Rust core goes through `invoke` and every push from the
// core goes through the `listen` helpers below. No other module talks to Tauri.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AuthResult,
  DeviceScanResult,
  EngineStatus,
  HelloStatus,
  LauncherApp,
  MicStatus,
  PatternResult,
  ProtectedApp,
  ProtectedItem,
  SecurityEvent,
  Settings,
  VoiceCommand,
  VoiceIdentity,
  VoiceStatus,
} from "./types";

/** True when running inside the Tauri webview (false in a plain browser). */
export const isTauri = (): boolean =>
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (!isTauri()) {
    throw new Error(`BLACKVAULT core unavailable: cannot invoke "${cmd}" outside the app.`);
  }
  return invoke<T>(cmd, args);
}

// ============================ Engine / status ============================

export const getStatus = () => call<EngineStatus>("get_status");
export const getSettings = () => call<Settings>("get_settings");
export const saveSettings = (settings: Settings) =>
  call<Settings>("save_settings", { settings });
export const getVoiceStatus = () => call<VoiceStatus>("get_voice_status");
export const getMicStatus = () => call<MicStatus>("get_mic_status");
export const getHelloStatus = () => call<HelloStatus>("get_hello_status");

// ============================ Voice engine ===============================

export const startVoice = () => call<VoiceStatus>("start_voice");
export const stopVoice = () => call<VoiceStatus>("stop_voice");
export const restartVoice = () => call<VoiceStatus>("restart_voice");
export const getVoiceIdentity = () => call<VoiceIdentity>("get_voice_identity");
export const enrollVoice = (samples: number[]) =>
  call<VoiceIdentity>("enroll_voice", { samples });
export const clearVoiceIdentity = () => call<VoiceIdentity>("clear_voice_identity");

/** Teach the recogniser a phrase from a recorded sample (Float32 → number[]). */
export const trainPhrase = (phrase: string, kind: string, samples: number[]) =>
  call<void>("train_phrase", { phrase, kind, samples });

export const listCommands = () => call<VoiceCommand[]>("list_commands");
export const upsertCommand = (command: Partial<VoiceCommand>) =>
  call<VoiceCommand>("upsert_command", { command });
export const deleteCommand = (id: number) => call<void>("delete_command", { id });
export const simulateCommand = (phrase: string) =>
  call<AuthResult>("simulate_command", { phrase });

// ============================ Lock / unlock ==============================

export const lockPc = (reason?: string) => call<EngineStatus>("lock_pc", { reason });
export const beginUnlock = () => call<EngineStatus>("begin_unlock");
/** The factors the lock screen should offer right now. */
export const unlockFactors = () => call<string[]>("unlock_factors");
export const submitPassphrase = (phrase: string) =>
  call<AuthResult>("submit_passphrase", { phrase });
export const submitPin = (pin: string) => call<AuthResult>("submit_pin", { pin });
export const submitPattern = (nodes: number[]) =>
  call<AuthResult>("submit_pattern", { nodes });
export const submitHello = () => call<AuthResult>("submit_hello");
export const submitVoice = (phrase: string, sample: number[]) =>
  call<AuthResult>("submit_voice", { phrase, sample });
export const cancelUnlock = () => call<EngineStatus>("cancel_unlock");
export const completeUnlock = (token: string) =>
  call<EngineStatus>("complete_unlock", { token });

export const setPassphrase = (phrase: string) => call<void>("set_passphrase", { phrase });
export const setPin = (pin: string) => call<void>("set_pin", { pin });
export const clearPin = () => call<void>("clear_pin");
export const setPattern = (nodes: number[]) => call<void>("set_pattern", { nodes });
export const clearPattern = () => call<void>("clear_pattern");
export const testPattern = (nodes: number[]) =>
  call<PatternResult>("test_pattern", { nodes });

// ============================ Launcher ==================================

export const listApps = () => call<LauncherApp[]>("list_apps");
export const scanInstalledApps = () => call<DeviceScanResult[]>("scan_installed_apps");
export const pickExecutable = () => call<string | null>("pick_executable");
export const upsertApp = (app: Partial<LauncherApp>) =>
  call<LauncherApp>("upsert_app", { app });
export const deleteApp = (id: number) => call<void>("delete_app", { id });
export const launchApp = (id: number) => call<void>("launch_app", { id });
export const closeApp = (id: number) => call<void>("close_app", { id });

// ============================ Protected apps ============================

export const listProtectedApps = () => call<ProtectedApp[]>("list_protected_apps");
export const protectApp = (exePath: string, displayName?: string) =>
  call<ProtectedApp>("protect_app", { exePath, displayName: displayName ?? null });
export const unprotectApp = (id: number) => call<void>("unprotect_app", { id });
export const setAppLock = (id: number, locked: boolean) =>
  call<ProtectedApp>("set_app_lock", { id, locked });
export const updateProtectedApp = (app: ProtectedApp) =>
  call<ProtectedApp>("update_protected_app", { app });

// ============================ File / folder protection ==================

export const listProtectedItems = () => call<ProtectedItem[]>("list_protected_items");
export const pickPath = (kind: "file" | "folder") =>
  call<string | null>("pick_path", { kind });
export const protectPath = (path: string, encryption: ProtectedItem["encryption"]) =>
  call<ProtectedItem>("protect_path", { path, encryption });
export const unprotectPath = (id: number) => call<void>("unprotect_path", { id });
export const revealPath = (id: number) => call<void>("reveal_path", { id });
export const resealPath = (id: number) => call<void>("reseal_path", { id });
/** Confirm a protected application may keep running (the grace prompt). */
export const authorizeApp = (id: number) => call<unknown>("set_app_lock", { id, locked: false });
/** Ask the STT endpoint whether the configured language/endpoint works. */
export const probeStt = (settings: Settings) =>
  call<{ text: string; locale: string; confidence: number; engine: string }>("probe_stt", {
    settings,
  });

// ============================ Automation ================================

export const runAutomation = (action: string, payload?: string | null) =>
  call<void>("run_automation", { action, payload: payload ?? null });
export const pressKeys = (combo: string) => call<void>("press_keys", { combo });
export const clickMouse = (x?: number, y?: number, button?: string) =>
  call<void>("click_mouse", { x: x ?? null, y: y ?? null, button: button ?? "left" });
export const scrollMouse = (dx: number, dy: number) =>
  call<void>("scroll_mouse", { dx, dy });

// ============================ Service / events ==========================

export const listEvents = (limit?: number) =>
  call<SecurityEvent[]>("list_events", { limit: limit ?? 100 });
export const clearEvents = () => call<void>("clear_events");
export const exportAudit = () => call<string>("export_audit");
export const serviceStatus = () => call<boolean>("service_status");
export const startService = () => call<boolean>("start_service");
export const stopService = () => call<boolean>("stop_service");
export const setAutostart = (enabled: boolean) => call<boolean>("set_autostart", { enabled });
export const openLogFolder = () => call<void>("open_log_folder");
export const factoryReset = () => call<void>("factory_reset");

// ============================ Event bus =================================

export type CoreEvent =
  | { type: "status"; payload: EngineStatus }
  | { type: "voice"; payload: VoiceStatus }
  | { type: "mic"; payload: MicStatus }
  | { type: "mode"; payload: { mode: string; reason?: string } }
  | { type: "heard"; payload: { text: string; isFinal: boolean } }
  | { type: "command"; payload: { phrase: string; action: string; ok: boolean } }
  | { type: "auth"; payload: AuthResult }
  | { type: "event"; payload: SecurityEvent }
  | { type: "challenge"; payload: { factors: string[] } }
  | {
      type: "appChallenge";
      payload: { ruleId: number; processName: string; displayName: string; graceSecs: number };
    }
  | { type: "unlocked"; payload: { token: string } }
  | { type: "settings"; payload: Settings }
  | { type: "toast"; payload: { title: string; body: string; severity: string } };

const TOPIC = "blackvault://event";

export function onCoreEvent(handler: (e: CoreEvent) => void): () => void {
  if (!isTauri()) return () => {};
  let un: UnlistenFn | null = null;
  let disposed = false;
  listen<CoreEvent>(TOPIC, (ev) => handler(ev.payload)).then((fn) => {
    if (disposed) fn();
    else un = fn;
  });
  return () => {
    disposed = true;
    if (un) un();
  };
}

export const EVENTS = {
  core: TOPIC,
} as const;
