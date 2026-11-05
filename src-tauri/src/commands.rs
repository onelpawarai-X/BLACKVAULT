//! BLACKVAULT — the Tauri command surface.
//!
//! Every function here corresponds one-to-one with a method in
//! `src/lib/bridge.ts`. Commands validate their input, do the work through the
//! core modules, update shared state, emit events and return the new state so
//! the UI never has to guess.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tauri::{Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;

use crate::auth;
use crate::error::{BvError, BvResult};
use crate::models::*;
use crate::platform;
use crate::settings::Settings;
use crate::state::{AppState, CoreEvent, EVENT_TOPIC, PendingPrompt, PromptTarget};
use crate::voice::matcher::{self, Template};

type St<'a> = State<'a, Arc<AppState>>;

// ============================================================ status & settings

#[tauri::command]
pub fn get_status(state: St<'_>) -> EngineStatus {
    build_status(&state)
}

pub fn build_status(state: &Arc<AppState>) -> EngineStatus {
    let (apps, files, folders) = state.db.counts().unwrap_or((0, 0, 0));
    EngineStatus {
        mode: state.mode(),
        voice: state.voice.status(),
        mic: mic_status(state),
        hello: hello_status(state),
        pin_configured: state.db.get_secret(auth::SECRET_PIN).ok().flatten().is_some(),
        pattern_configured: state.db.get_secret(auth::SECRET_PATTERN).ok().flatten().is_some(),
        passphrase_configured: state.db.get_secret(auth::SECRET_PASSPHRASE).ok().flatten().is_some(),
        service_running: true,
        protected_apps: apps,
        protected_files: files,
        protected_folders: folders,
        uptime_secs: state.uptime_secs(),
        last_auth: state.db.last_auth_time().ok().flatten(),
        speech_available: state.speech_available.load(Ordering::Relaxed),
        stt: stt_status(state),
    }
}

fn stt_status(state: &Arc<AppState>) -> crate::models::SttStatus {
    let settings = state.settings();
    let cfg = state.stt.config();
    crate::models::SttStatus {
        enabled: settings.stt_enabled,
        running: state.stt.is_running(),
        busy: state.stt.is_busy(),
        endpoint: cfg.endpoint,
        language: cfg.language,
        live: cfg.live,
        recognised: state.stt.seen(),
        last_text: state.stt.last_text(),
        last_error: state.stt.last_error(),
    }
}

#[tauri::command]
pub fn get_settings(state: St<'_>) -> Settings {
    state.settings()
}

/// Reachability check for the speech-to-text endpoint, driven from Settings so
/// the user can confirm the language and endpoint before trusting them.
#[tauri::command]
pub fn probe_stt(settings: Settings) -> BvResult<crate::platform::stt::Transcript> {
    let cfg = crate::platform::stt::SttConfig {
        endpoint: settings.stt_endpoint.clone(),
        language: settings.stt_language.clone(),
        live: settings.stt_live,
        min_confidence: settings.stt_min_confidence,
    };
    crate::platform::stt::probe(&cfg)
}

#[tauri::command]
pub fn save_settings(state: St<'_>, settings: Settings) -> BvResult<Settings> {
    let sanitized = settings.sanitize();
    state.db.save_settings(&sanitized)?;
    *state.settings.write() = sanitized.clone();

    // Apply the startup preference to the real Run key.
    let _ = platform::service::set_autostart(sanitized.start_with_windows);

    // Recognition templates depend on the wake phrase and the command set.
    crate::engine::rebuild_templates(&state);
    // The offline grammar listens for the wake phrase and every command.
    crate::engine::restart_speech(&state);
    // The STT worker needs the new endpoint / language / mode.
    crate::engine::sync_stt(&state);

    // Keep the engine in step with the always-on preference.
    if sanitized.always_on_voice && !state.voice.is_running() {
        let st = state.voice.start();
        state.emit(CoreEvent::Voice(st));
    } else if !sanitized.always_on_voice && state.voice.is_running() {
        let st = state.voice.stop();
        state.emit(CoreEvent::Voice(st));
    }

    state.log_event("setting_changed", "info", "Settings updated", None);
    state.emit(CoreEvent::Settings(Box::new(sanitized.clone())));
    state.emit(CoreEvent::Status(build_status(&state)));
    Ok(sanitized)
}

#[tauri::command]
pub fn get_voice_status(state: St<'_>) -> VoiceStatus {
    state.voice.status()
}

#[tauri::command]
pub fn get_mic_status(state: St<'_>) -> MicStatus {
    mic_status(&state)
}

fn mic_status(state: &Arc<AppState>) -> MicStatus {
    let device = default_input_name();
    MicStatus {
        device_name: device,
        level: state.voice.level(),
        permission: "granted".into(),
        muted: !state.voice.is_running(),
    }
}

fn default_input_name() -> Option<String> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    host.default_input_device().and_then(|d| d.name().ok())
}

#[tauri::command]
pub fn get_hello_status(state: St<'_>) -> HelloStatus {
    hello_status(&state)
}

fn hello_status(state: &Arc<AppState>) -> HelloStatus {
    let available = state.hello_available();
    let method = platform::hello::availability().method;
    let enrolled = state.db.get_secret("hello_enrolled").ok().flatten().is_some();
    HelloStatus {
        available,
        enrolled,
        method,
        last_verified: state.db.last_auth_time().ok().flatten(),
    }
}

// ============================================================== voice engine

#[tauri::command]
pub fn start_voice(state: St<'_>) -> VoiceStatus {
    let settings = state.settings();
    state.voice.set_wake_phrase(Some(settings.wake_phrase.clone()));
    let st = state.voice.start();
    state.log_event("engine_start", "info", "Voice engine started", None);
    state.emit(CoreEvent::Status(build_status(&state)));
    st
}

#[tauri::command]
pub fn stop_voice(state: St<'_>) -> VoiceStatus {
    let st = state.voice.stop();
    state.log_event("engine_stop", "info", "Voice engine stopped", None);
    state.emit(CoreEvent::Status(build_status(&state)));
    st
}

#[tauri::command]
pub fn restart_voice(state: St<'_>) -> VoiceStatus {
    let settings = state.settings();
    state.voice.set_wake_phrase(Some(settings.wake_phrase.clone()));
    let st = state.voice.restart();
    crate::engine::rebuild_templates(&state);
    state.log_event("engine_recovery", "info", "Voice engine restarted", None);
    st
}

#[tauri::command]
pub fn get_voice_identity(state: St<'_>) -> VoiceIdentity {
    state.db.voice_identity().ok().flatten().unwrap_or(VoiceIdentity {
        enrolled: false,
        sample_count: 0,
        threshold: state.settings().voiceprint_threshold,
        updated_at: None,
    })
}

/// Enrol a voice identity. The frontend sends a flat sample buffer which we
/// split back into per-sample windows.
#[tauri::command]
pub fn enroll_voice(state: St<'_>, samples: Vec<f32>) -> BvResult<VoiceIdentity> {
    let windows: Vec<Vec<f32>> = samples.chunks(512).map(|c| c.to_vec()).collect();
    if windows.is_empty() {
        return Err(BvError::msg("no samples were captured"));
    }
    let (count, threshold) = auth::enrol_voice(&state.db, &windows)?;

    // The same samples are the user speaking their unlock phrase, so teach the
    // recogniser that phrase in the same pass. Without this the identity would
    // be enrolled but no phrase template would exist to match against.
    let mut template: Vec<Vec<f32>> = Vec::new();
    for w in &windows {
        let frames = crate::dsp::voiced_frames(w);
        template.extend(frames);
    }
    if !template.is_empty() {
        let phrase = matcher::normalise("unlock this pc");
        state.db.set_template(&phrase, "unlock", &template)?;
        crate::engine::rebuild_templates(&state);
    }

    state.toast("Voice identity enrolled", &format!("{count} samples stored"), "info");
    Ok(VoiceIdentity {
        enrolled: true,
        sample_count: count,
        threshold,
        updated_at: Some(chrono::Utc::now().to_rfc3339()),
    })
}

#[tauri::command]
pub fn clear_voice_identity(state: St<'_>) -> BvResult<VoiceIdentity> {
    state.db.clear_voice_identity()?;
    state.log_event("hello_enrolled", "warn", "Voice identity cleared", None);
    Ok(VoiceIdentity {
        enrolled: false,
        sample_count: 0,
        threshold: state.settings().voiceprint_threshold,
        updated_at: None,
    })
}

/// Teach the recogniser a phrase. The sample is the spoken phrase; we store its
/// MFCC template so later utterances can be matched against it.
#[tauri::command]
pub fn train_phrase(state: St<'_>, phrase: String, kind: Option<String>, samples: Vec<f32>) -> BvResult<()> {
    let phrase = matcher::normalise(&phrase);
    if phrase.is_empty() {
        return Err(BvError::msg("the phrase is empty"));
    }
    let mfcc = crate::dsp::voiced_frames(&samples);
    if mfcc.is_empty() {
        return Err(BvError::msg("the sample was too short to analyse"));
    }
    let kind = kind.unwrap_or_else(|| "command".into());
    state.db.set_template(&phrase, &kind, &mfcc)?;
    crate::engine::rebuild_templates(&state);
    state.log_event(
        "voice_command",
        "info",
        &format!("Phrase trained: \"{phrase}\""),
        Some(&format!("{kind}, {} frames", mfcc.len())),
    );
    Ok(())
}

// ================================================================= commands

#[tauri::command]
pub fn list_commands(state: St<'_>) -> BvResult<Vec<VoiceCommand>> {
    state.db.list_commands()
}

#[tauri::command]
pub fn upsert_command(state: St<'_>, command: VoiceCommand) -> BvResult<VoiceCommand> {
    let mut cmd = command;
    cmd.phrase = matcher::normalise(&cmd.phrase);
    if cmd.phrase.is_empty() {
        return Err(BvError::msg("the command phrase is empty"));
    }
    if cmd.action.is_empty() {
        cmd.action = cmd.action_type.clone();
    }
    let saved = state.db.upsert_command(&cmd)?;
    crate::engine::rebuild_templates(&state);
    crate::engine::restart_speech(&state);
    state.log_event(
        "setting_changed",
        "info",
        &format!("Voice command saved: \"{}\"", saved.phrase),
        Some(&saved.action_type),
    );
    state.emit(CoreEvent::Status(build_status(&state)));
    Ok(saved)
}

#[tauri::command]
pub fn delete_command(state: St<'_>, id: i64) -> BvResult<()> {
    let phrase = state
        .db
        .list_commands()?
        .into_iter()
        .find(|c| c.id == id)
        .map(|c| c.phrase);
    state.db.delete_command(id)?;
    if let Some(p) = phrase {
        let _ = state.db.delete_template(&p);
    }
    crate::engine::rebuild_templates(&state);
    crate::engine::restart_speech(&state);
    state.log_event("setting_changed", "info", "Voice command deleted", None);
    Ok(())
}

/// Dry-run a phrase against the command list, for the editor's Test button.
#[tauri::command]
pub fn simulate_command(state: St<'_>, phrase: String) -> BvResult<AuthResult> {
    let commands = state.db.list_commands()?;
    let phrases: Vec<String> = commands.iter().map(|c| c.phrase.clone()).collect();
    match matcher::match_text(&phrase, &phrases) {
        Some((matched, score)) => {
            let cmd = commands.iter().find(|c| c.phrase == matched);
            let action = cmd.map(|c| c.action_type.clone()).unwrap_or_default();
            let mut r = AuthResult::ok("simulate", format!("Matched \"{matched}\" → {action}"));
            r.score = Some(score);
            Ok(r)
        }
        None => Ok(AuthResult::fail("simulate", "No command matches that phrase.", None)),
    }
}

// ============================================================ lock / unlock

#[tauri::command]
pub fn lock_pc(state: St<'_>, reason: Option<String>) -> EngineStatus {
    perform_lock(&state, &reason.unwrap_or_else(|| "manual".into()));
    build_status(&state)
}

/// Move the whole system into Full-Screen Privacy Mode.
pub fn perform_lock(state: &Arc<AppState>, reason: &str) {
    if state.mode() == SecurityMode::Locked {
        return;
    }
    // Anti-lockout: without at least one configured factor the overlay would be
    // a one-way door, so refuse to lock and say why instead.
    if !has_unlock_factor(state) {
        state.toast(
            "Lock refused",
            "Set a PIN, pattern or secret passphrase first so you can unlock again.",
            "warn",
        );
        state.log_event(
            "lock_refused",
            "warn",
            "Lock refused: no unlock factor is configured",
            Some(reason),
        );
        return;
    }
    let settings = state.settings();
    state.set_mode(SecurityMode::Locked);
    state.partial_auth.store(false, Ordering::Relaxed);

    // 1. The primary window covers its monitor, borderless and top-most.
    if let Some(app) = state.app.lock().clone() {
        if let Some(win) = app.get_webview_window("main") {
            let _ = win.set_decorations(false);
            let _ = win.set_always_on_top(true);
            let _ = win.set_skip_taskbar(true);
            let _ = win.set_fullscreen(true);
            let _ = win.show();
            let _ = win.set_focus();
        }
    }

    // 2. Every other monitor gets its own passive overlay.
    if settings.multi_monitor_overlay {
        crate::engine::deploy_overlays(state);
    }

    // 3. Optional hardening while the overlay is up.
    //
    // The cursor is deliberately *not* hidden any more. The unlock screen needs
    // a pointer to click the PIN field or to draw the pattern, and a hidden
    // system cursor made the lock screen unusable.
    if settings.block_task_switch_when_locked {
        platform::lock::set_task_switch_blocked(true);
    }

    // 4. Re-seal every protected item.
    reseal_items(state);

    // 5. The voice engine must keep listening — this is the whole point.
    let _ = platform::service::mark_running(&state.data_dir);

    state.log_event("lock", "info", "Desktop locked", Some(reason));
    state.emit(CoreEvent::Mode {
        mode: "locked".into(),
        reason: Some(reason.into()),
    });
    state.emit(CoreEvent::Status(build_status(state)));
}

/// Is there at least one way back in? Checked before locking.
pub fn has_unlock_factor(state: &Arc<AppState>) -> bool {
    let has = |name: &str| state.db.get_secret(name).ok().flatten().is_some();
    has(crate::auth::SECRET_PASSPHRASE)
        || has(crate::auth::SECRET_PIN)
        || has(crate::auth::SECRET_PATTERN)
        || state.hello_available()
}

/// Bring the desktop back.
pub fn finish_unlock(state: &Arc<AppState>) {
    state.set_mode(SecurityMode::Unlocked);
    state.partial_auth.store(false, Ordering::Relaxed);

    // Undo any hardening the lock applied.
    platform::lock::show_cursor();
    platform::lock::set_task_switch_blocked(false);

    if let Some(app) = state.app.lock().clone() {
        if let Some(win) = app.get_webview_window("main") {
            let _ = win.set_fullscreen(false);
            let _ = win.set_always_on_top(false);
            let _ = win.set_skip_taskbar(false);
            let _ = win.set_decorations(true);
        }
    }
    crate::engine::retract_overlays(state);

    let token = crate::crypto::random_token();
    *state.unlock_token.lock() = Some(token.clone());

    state.log_event("unlock", "info", "Desktop unlocked", None);
    state.emit(CoreEvent::Mode {
        mode: "unlocked".into(),
        reason: None,
    });
    state.emit(CoreEvent::Unlocked { token });
    state.emit(CoreEvent::Status(build_status(state)));
}

/// The factors to offer on the lock screen right now. The UI calls this when
/// the lock screen appears so the passphrase prompt is present even when the
/// user unlocks by typing rather than by voice.
#[tauri::command]
pub fn unlock_factors(state: St<'_>) -> Vec<String> {
    let settings = state.settings();
    auth::first_step_factors(&state.db, &settings, state.hello_available())
}

#[tauri::command]
pub fn begin_unlock(state: St<'_>) -> EngineStatus {
    begin_unlock_now(&state)
}

/// The unlock prompt itself. Split out of the command so the supervisor thread
/// can raise it from a recognised utterance without going through Tauri.
pub fn begin_unlock_now(state: &Arc<AppState>) -> EngineStatus {
    state.set_mode(SecurityMode::Unlocking);
    state.partial_auth.store(false, Ordering::Relaxed);
    let settings = state.settings();
    let factors = auth::first_step_factors(&state.db, &settings, state.hello_available());
    state.emit(CoreEvent::Mode {
        mode: "unlocking".into(),
        reason: None,
    });
    state.emit(CoreEvent::Challenge { factors });
    build_status(state)
}

fn after_factor(state: &Arc<AppState>, result: AuthResult, method: &str) -> AuthResult {
    if !result.ok {
        state.emit(CoreEvent::Auth(result.clone()));
        return result;
    }
    let settings = state.settings();
    let first_done = state.partial_auth.load(Ordering::Relaxed);

    // Two-factor only means anything when a first factor (the passphrase) is
    // actually configured. With no passphrase the PIN, the pattern or Hello *is*
    // the only factor, and refusing a correct PIN with "enter your passphrase
    // first" is the false rejection that made a right credential look wrong.
    let passphrase_set = state
        .db
        .get_secret(crate::auth::SECRET_PASSPHRASE)
        .ok()
        .flatten()
        .is_some();
    let two_factor = settings.require_second_factor && passphrase_set;

    match auth::decide_step(two_factor, first_done, method) {
        auth::UnlockStep::OutOfOrder => {
            // A PIN on its own must not open the desktop, which is exactly the
            // two-factor policy the user configured.
            let refused = AuthResult::fail(
                method,
                "Enter your secret passphrase first, then the second factor.",
                None,
            );
            state.emit(CoreEvent::Auth(refused.clone()));
            refused
        }
        auth::UnlockStep::NeedSecondFactor => {
            // Ask for the second factor only if one actually exists. With the
            // default policy on but no PIN, pattern or Hello configured, there is
            // nothing to ask for, so the verified passphrase completes the unlock
            // — otherwise the screen would demand the passphrase a second time and
            // never let the user in.
            let factors = auth::second_step_factors(&state.db, &settings, state.hello_available());
            if factors.is_empty() {
                state.emit(CoreEvent::Auth(result.clone()));
                finish_unlock(state);
                return result;
            }
            state.partial_auth.store(true, Ordering::Relaxed);
            state.emit(CoreEvent::Auth(result.clone()));
            state.emit(CoreEvent::Challenge { factors });
            result
        }
        auth::UnlockStep::Complete => {
            state.emit(CoreEvent::Auth(result.clone()));
            finish_unlock(state);
            result
        }
    }
}

#[tauri::command]
pub fn submit_passphrase(state: St<'_>, phrase: String) -> BvResult<AuthResult> {
    let settings = state.settings();
    let r = auth::verify_passphrase(&state.db, &settings, &phrase)?;
    Ok(after_factor(&state, r, "passphrase"))
}

/// The spoken-passphrase path used by the lock screen. It runs the *same*
/// PBKDF2 check as the typed field; speech recognition is only the input
/// method, never the proof of identity.
pub fn spoken_passphrase(state: &Arc<AppState>, phrase: &str) -> AuthResult {
    let settings = state.settings();
    let r = match auth::verify_passphrase(&state.db, &settings, phrase) {
        Ok(r) => r,
        Err(e) => AuthResult::fail("passphrase", format!("Could not verify: {e}"), None),
    };
    after_factor(state, r, "passphrase")
}

#[tauri::command]
pub fn submit_pin(state: St<'_>, pin: String) -> BvResult<AuthResult> {
    let settings = state.settings();
    let r = auth::verify_pin(&state.db, &settings, &pin)?;
    Ok(after_factor(&state, r, "pin"))
}

#[tauri::command]
pub fn submit_pattern(state: St<'_>, nodes: Vec<i32>) -> BvResult<AuthResult> {
    let settings = state.settings();
    let r = auth::verify_pattern(&state.db, &settings, &nodes)?;
    Ok(after_factor(&state, r, "pattern"))
}

#[tauri::command]
pub fn submit_hello(state: St<'_>) -> BvResult<AuthResult> {
    let settings = state.settings();
    let r = auth::verify_hello(&state.db, &settings)?;
    if r.ok {
        let _ = state.db.set_secret("hello_enrolled", "1");
        state.hello_available.store(true, Ordering::Relaxed);
    }
    Ok(after_factor(&state, r, "hello"))
}

#[tauri::command]
pub fn submit_voice(state: St<'_>, phrase: String, sample: Vec<f32>) -> BvResult<AuthResult> {
    let settings = state.settings();
    let speaker = crate::dsp::speaker_vector(&sample);
    let r = auth::verify_voice(&state.db, &settings, &phrase, &speaker)?;
    Ok(after_factor(&state, r, "voice_full"))
}

#[tauri::command]
pub fn cancel_unlock(state: St<'_>) -> EngineStatus {
    // Cancelling an unlock attempt returns to the locked face, not the desktop.
    state.set_mode(SecurityMode::Locked);
    state.partial_auth.store(false, Ordering::Relaxed);
    state.emit(CoreEvent::Mode {
        mode: "locked".into(),
        reason: Some("unlock cancelled".into()),
    });
    build_status(&state)
}

#[tauri::command]
pub fn complete_unlock(state: St<'_>, token: String) -> EngineStatus {
    let expected = state.unlock_token.lock().clone();
    // Accept the issued token, or a re-assertion once already unlocked.
    if expected.as_deref() == Some(token.as_str()) || state.mode() == SecurityMode::Unlocked {
        *state.unlock_token.lock() = None;
        if state.mode() != SecurityMode::Unlocked {
            finish_unlock(&state);
        }
    }
    build_status(&state)
}

// ------------------------------------------------------------- factor config

#[tauri::command]
pub fn set_passphrase(state: St<'_>, phrase: String) -> BvResult<()> {
    auth::set_passphrase(&state.db, &phrase)?;
    state.emit(CoreEvent::Status(build_status(&state)));
    Ok(())
}

#[tauri::command]
pub fn set_pin(state: St<'_>, pin: String) -> BvResult<()> {
    auth::set_pin(&state.db, &pin)?;
    state.emit(CoreEvent::Status(build_status(&state)));
    Ok(())
}

#[tauri::command]
pub fn clear_pin(state: St<'_>) -> BvResult<()> {
    state.db.clear_secret(auth::SECRET_PIN)?;
    state.emit(CoreEvent::Status(build_status(&state)));
    Ok(())
}

#[tauri::command]
pub fn set_pattern(state: St<'_>, nodes: Vec<i32>) -> BvResult<()> {
    auth::set_pattern(&state.db, &nodes)?;
    state.emit(CoreEvent::Status(build_status(&state)));
    Ok(())
}

#[tauri::command]
pub fn clear_pattern(state: St<'_>) -> BvResult<()> {
    state.db.clear_secret(auth::SECRET_PATTERN)?;
    state.emit(CoreEvent::Status(build_status(&state)));
    Ok(())
}

#[tauri::command]
pub fn test_pattern(state: St<'_>, nodes: Vec<i32>) -> PatternResult {
    let settings = state.settings();
    match auth::verify_pattern(&state.db, &settings, &nodes) {
        Ok(r) => PatternResult {
            ok: r.ok,
            message: r.message,
        },
        Err(e) => PatternResult {
            ok: false,
            message: e.to_string(),
        },
    }
}

// ================================================================= launcher

#[tauri::command]
pub fn list_apps(state: St<'_>) -> BvResult<Vec<LauncherApp>> {
    state.db.list_apps()
}

#[tauri::command]
pub fn scan_installed_apps() -> Vec<DeviceScanResult> {
    platform::launcher::scan_installed()
}

#[tauri::command]
pub fn pick_executable(app: tauri::AppHandle) -> Option<String> {
    app.dialog()
        .file()
        .add_filter("Applications", &["exe", "lnk", "bat", "cmd"])
        .blocking_pick_file()
        .and_then(|p| p.into_path().ok())
        .map(|p| p.to_string_lossy().to_string())
}

#[tauri::command]
pub fn upsert_app(state: St<'_>, app: LauncherApp) -> BvResult<LauncherApp> {
    if app.name.trim().is_empty() || app.command.trim().is_empty() {
        return Err(BvError::msg("a name and an executable are required"));
    }
    let saved = state.db.upsert_app(&app)?;
    crate::engine::rebuild_templates(&state);
    state.log_event(
        "setting_changed",
        "info",
        &format!("Launcher entry saved: {}", saved.name),
        Some(&saved.command),
    );
    Ok(saved)
}

#[tauri::command]
pub fn delete_app(state: St<'_>, id: i64) -> BvResult<()> {
    state.db.delete_app(id)?;
    crate::engine::rebuild_templates(&state);
    Ok(())
}

#[tauri::command]
pub fn launch_app(state: St<'_>, id: i64) -> BvResult<()> {
    let app = state
        .db
        .get_app(id)?
        .ok_or_else(|| BvError::msg("that application is not configured"))?;
    if !app.enabled {
        return Err(BvError::msg("that application is disabled"));
    }
    platform::launcher::launch(&app.command, app.args.as_deref(), app.cwd.as_deref())?;
    state.db.bump_app_launch(id)?;
    state.log_event("app_launch", "info", &format!("Launched {}", app.name), Some(&app.command));
    state.emit(CoreEvent::Status(build_status(&state)));
    Ok(())
}

#[tauri::command]
pub fn close_app(state: St<'_>, id: i64) -> BvResult<()> {
    let app = state
        .db
        .get_app(id)?
        .ok_or_else(|| BvError::msg("that application is not configured"))?;
    let process = std::path::Path::new(&app.command)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| app.name.clone());
    platform::launcher::close_process(&process)?;
    state.log_event("app_launch", "info", &format!("Closed {}", app.name), None);
    Ok(())
}

// =========================================================== protected apps

#[tauri::command]
pub fn list_protected_apps(state: St<'_>) -> BvResult<Vec<ProtectedApp>> {
    state.db.list_protected_apps()
}

#[tauri::command]
pub fn protect_app(state: St<'_>, exe_path: String, display_name: Option<String>) -> BvResult<ProtectedApp> {
    let path = std::path::Path::new(&exe_path);
    if !path.exists() {
        return Err(BvError::msg("that file does not exist"));
    }
    let process_name = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .ok_or_else(|| BvError::msg("could not read the executable name"))?;
    let display_name = display_name.unwrap_or_else(|| process_name.clone());
    let app = ProtectedApp {
        id: 0,
        exe_path: exe_path.clone(),
        display_name: display_name.clone(),
        process_name,
        require_auth: true,
        auto_relock: true,
        relock_after_secs: 30,
        locked: true,
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    let saved = state.db.insert_protected_app(&app)?;
    state.log_event("setting_changed", "info", &format!("Protected {display_name}"), Some(&exe_path));
    state.emit(CoreEvent::Status(build_status(&state)));
    Ok(saved)
}

#[tauri::command]
pub fn unprotect_app(state: St<'_>, id: i64) -> BvResult<()> {
    state.db.delete_protected_app(id)?;
    state.emit(CoreEvent::Status(build_status(&state)));
    Ok(())
}

#[tauri::command]
pub fn set_app_lock(state: St<'_>, id: i64, locked: bool) -> BvResult<ProtectedApp> {
    // Unlocking a protected app is an authentication-gated action.
    if !locked {
        let settings = state.settings();
        let r = auth::verify_hello(&state.db, &settings).unwrap_or_else(|_| {
            AuthResult::fail("hello", "Windows Hello unavailable", None)
        });
        if !r.ok {
            return Err(BvError::msg(
                "Authentication is required before a protected application can be released.",
            ));
        }
    }
    state.db.set_app_locked(id, locked)?;
    let rules = state.db.list_protected_apps()?;
    let rule = rules
        .into_iter()
        .find(|r| r.id == id)
        .ok_or(BvError::NotFound)?;
    if !locked {
        state
            .authorizations
            .lock()
            .grant(&rule.process_name, Duration::from_secs(300));
    } else {
        state.authorizations.lock().revoke(&rule.process_name);
    }
    state.log_event(
        "setting_changed",
        "info",
        &format!("{} {}", if locked { "Locked" } else { "Unlocked" }, rule.display_name),
        None,
    );
    state.emit(CoreEvent::Status(build_status(&state)));
    Ok(rule)
}

#[tauri::command]
pub fn update_protected_app(state: St<'_>, app: ProtectedApp) -> BvResult<ProtectedApp> {
    state.db.update_protected_app(&app)?;
    Ok(app)
}

// =========================================================== protected items

#[tauri::command]
pub fn list_protected_items(state: St<'_>) -> BvResult<Vec<ProtectedItem>> {
    state.db.list_protected_items()
}

#[tauri::command]
pub fn pick_path(app: tauri::AppHandle, kind: String) -> Option<String> {
    let dialog = app.dialog().file();
    if kind == "folder" {
        dialog.blocking_pick_folder()
    } else {
        dialog.blocking_pick_file()
    }
    .and_then(|p| p.into_path().ok())
    .map(|p| p.to_string_lossy().to_string())
}

#[tauri::command]
pub fn protect_path(state: St<'_>, path: String, encryption: String) -> BvResult<ProtectedItem> {
    let p = std::path::Path::new(&path);
    if !p.exists() {
        return Err(BvError::msg("that path does not exist"));
    }
    let snapshot = platform::fileprot::snapshot_acl(p).unwrap_or_default();
    let applied = platform::fileprot::seal_item(p, &state.settings().file_lock_mode)?;
    if !applied.acl && !applied.encrypted {
        return Err(BvError::msg(
            "protection could not be applied. Run BLACKVAULT as an administrator, or choose a location you own.",
        ));
    }
    let kind = if platform::fileprot::is_dir(p) { "folder" } else { "file" };
    let display_name = p
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| path.clone());
    let item = ProtectedItem {
        id: 0,
        path: path.clone(),
        kind: kind.into(),
        display_name: display_name.clone(),
        encryption: applied.label(),
        protected: true,
        last_access: None,
        created_at: chrono::Utc::now().to_rfc3339(),
        size_bytes: platform::fileprot::size_of(p),
    };
    let saved = state.db.insert_protected_item(&item, &snapshot)?;
    let _ = encryption; // the applied mechanism is authoritative
    state.log_event(
        "file_protected",
        "info",
        &format!("Protected {display_name}"),
        Some(&format!("{path} ({})", applied.label())),
    );
    state.emit(CoreEvent::Status(build_status(&state)));
    Ok(saved)
}

#[tauri::command]
pub fn unprotect_path(state: St<'_>, id: i64) -> BvResult<()> {
    let items = state.db.list_protected_items()?;
    let item = items.into_iter().find(|i| i.id == id).ok_or(BvError::NotFound)?;
    let snapshot = state.db.acl_snapshot(id)?;
    let p = std::path::Path::new(&item.path);
    if p.exists() {
        platform::fileprot::open(p, snapshot.as_deref())?;
    }
    state.db.delete_protected_item(id)?;
    state.log_event("file_unprotected", "info", &format!("Unprotected {}", item.display_name), None);
    state.emit(CoreEvent::Status(build_status(&state)));
    Ok(())
}

/// Release an item for the current session.
///
/// Authentication is mandatory: releasing a protected file is exactly the
/// moment the protection is meant to be doing something. The same check guards
/// it as guards unlocking a protected application. The item re-seals on the next
/// lock, and the guard notices within a few seconds if the permission is put
/// back by hand.
/// Ask for the password before a protected item is released.
///
/// This used to demand Windows Hello and refuse outright when the machine had
/// no Hello — which meant a protected file could not be released at all on such
/// a machine. It now raises the same prompt card the guard raises, so the PIN or
/// the secret passphrase works and Hello is an extra option rather than a
/// requirement.
#[tauri::command]
pub fn reveal_path(state: St<'_>, id: i64) -> BvResult<()> {
    let items = state.db.list_protected_items()?;
    let item = items.into_iter().find(|i| i.id == id).ok_or(BvError::NotFound)?;
    raise_prompt(
        &state,
        PromptTarget::Path {
            item_id: id,
            path: item.path.clone(),
        },
    );
    Ok(())
}

/// Put a released item back under protection immediately.
#[tauri::command]
pub fn reseal_path(state: St<'_>, id: i64) -> BvResult<()> {
    let items = state.db.list_protected_items()?;
    let item = items.into_iter().find(|i| i.id == id).ok_or(BvError::NotFound)?;
    let p = std::path::Path::new(&item.path);
    if !p.exists() {
        return Err(BvError::msg("the protected path no longer exists"));
    }
    let applied = platform::fileprot::seal_item(p, &state.settings().file_lock_mode)?;
    if !applied.acl && !applied.encrypted {
        return Err(BvError::msg("could not re-seal the item"));
    }
    state.db.set_item_protected(id, true)?;
    state.log_event("file_protected", "info", &format!("Re-sealed {}", item.display_name), None);
    Ok(())
}

fn reseal_items(state: &Arc<AppState>) {
    if let Ok(items) = state.db.list_protected_items() {
        for item in items {
            let p = std::path::Path::new(&item.path);
            if p.exists() {
                if let Err(e) = platform::fileprot::seal_item(p, &state.settings().file_lock_mode) {
                    log::warn!("could not re-seal {}: {e}", item.path);
                }
            }
            let _ = state.db.set_item_protected(item.id, true);
        }
    }
}

// ------------------------------------------------------- authentication prompt

/// How long a prompt token stays usable. Long enough to type a password or
/// speak a phrase; short enough that an abandoned prompt cannot be answered
/// hours later.
const PROMPT_TTL: Duration = Duration::from_secs(180);

/// Ask for authentication in a window of its own.
///
/// The prompt has to be its own window. Raising it inside the control centre
/// meant the card was drawn into a window that is hidden in the tray whenever
/// the user is doing real work, so opening a protected file or application
/// produced no password request at all — the complaint behind "the lock does
/// not ask for a password".
pub fn raise_prompt(state: &Arc<AppState>, target: PromptTarget) -> String {
    let token = crate::crypto::random_token();
    *state.pending_prompt.lock() = Some(PendingPrompt {
        token: token.clone(),
        target: target.clone(),
        raised_at: Instant::now(),
    });
    let app = state.app.lock().clone();
    let Some(app) = app else {
        return token;
    };
    let label = "authprompt";
    if let Some(w) = app.get_webview_window(label) {
        let _ = w.close();
    }
    let origin = crate::platform::lock::focused_process_path();
    let event = match &target {
        PromptTarget::App { rule_id, process } => CoreEvent::AppChallenge {
            rule_id: *rule_id,
            process_name: process.clone(),
            display_name: display_name_for(state, *rule_id, process),
            grace_secs: state.settings().app_lock_grace_secs,
            origin,
            token: token.clone(),
        },
        PromptTarget::Path { item_id, path } => {
            let (display_name, kind) = item_label(state, *item_id, path);
            CoreEvent::PathChallenge {
                token: token.clone(),
                item_id: *item_id,
                path: path.clone(),
                display_name,
                kind,
                origin,
            }
        }
    };
    let built = tauri::WebviewWindowBuilder::new(
        &app,
        label,
        tauri::WebviewUrl::App("index.html?prompt=1".into()),
    )
    .title("BLACKVAULT — authentication required")
    .inner_size(460.0, 540.0)
    .resizable(false)
    .always_on_top(true)
    .center()
    .decorations(true)
    .theme(Some(tauri::Theme::Dark))
    .background_color(tauri::window::Color(0, 0, 0, 255))
    .build();
    match built {
        Ok(window) => {
            let _ = window.set_focus();
            // The window is created empty and boots its own page. Emit now (in
            // case the page is already listening) and again shortly after, so
            // the card is never rendered before the listener exists.
            let _ = window.emit(EVENT_TOPIC, &event);
            let w = window.clone();
            let event2 = event.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(500));
                let _ = w.emit(EVENT_TOPIC, &event2);
            });
        }
        Err(e) => {
            log::warn!("could not open the authentication prompt: {e}");
            state.emit(event);
        }
    }
    token
}

/// Close the prompt window and forget its token. `None` closes whatever is open.
pub fn close_prompt(state: &Arc<AppState>, token: Option<&str>) {
    let mut guard = state.pending_prompt.lock();
    let matches = match (guard.as_ref(), token) {
        (Some(p), Some(t)) => p.token == t,
        (Some(_), None) => true,
        (None, _) => false,
    };
    if !matches {
        return;
    }
    let closed = guard.take();
    drop(guard);
    if let Some(app) = state.app.lock().clone() {
        if let Some(w) = app.get_webview_window("authprompt") {
            let _ = w.close();
        }
    }
    if let Some(p) = closed {
        state.emit(CoreEvent::PromptClosed { token: p.token });
    }
}

/// Close the prompt window if it is asking about this application. Called when
/// the application is released by any route, so the window never lingers.
pub fn close_prompt_for_app(state: &Arc<AppState>, rule_id: i64) {
    let open = {
        let guard = state.pending_prompt.lock();
        matches!(
            guard.as_ref().map(|p| &p.target),
            Some(PromptTarget::App { rule_id: id, .. }) if *id == rule_id
        )
    };
    if open {
        close_prompt(state, None);
    }
}

fn display_name_for(state: &Arc<AppState>, rule_id: i64, process: &str) -> String {
    state
        .db
        .list_protected_apps()
        .ok()
        .and_then(|rules| rules.into_iter().find(|r| r.id == rule_id))
        .map(|r| r.display_name)
        .unwrap_or_else(|| process.to_string())
}

fn item_label(state: &Arc<AppState>, item_id: i64, path: &str) -> (String, String) {
    state
        .db
        .list_protected_items()
        .ok()
        .and_then(|items| items.into_iter().find(|i| i.id == item_id))
        .map(|i| (i.display_name, i.kind))
        .unwrap_or_else(|| (path.to_string(), "file".into()))
}

/// The live, unexpired prompt for this token, if there is one.
fn live_prompt(state: &Arc<AppState>, token: &str) -> Option<PendingPrompt> {
    let guard = state.pending_prompt.lock();
    let p = guard.as_ref()?;
    if p.token != token || p.raised_at.elapsed() > PROMPT_TTL {
        return None;
    }
    Some(p.clone())
}

/// Verify a factor on behalf of a prompt, without the unlock state machine.
///
/// `after_factor` is deliberately *not* used here: it drives the desktop unlock
/// ceremony and the partial-auth flag, and a prompt for one file or one
/// application must never touch either.
fn verify_for_prompt(state: &Arc<AppState>, pin: Option<&str>, phrase: Option<&str>) -> AuthResult {
    let settings = state.settings();
    if let Some(p) = pin.map(str::trim).filter(|p| !p.is_empty()) {
        if let Ok(r) = auth::verify_pin(&state.db, &settings, p) {
            if r.ok {
                return r;
            }
        }
    }
    if let Some(p) = phrase.map(str::trim).filter(|p| !p.is_empty()) {
        if let Ok(r) = auth::verify_passphrase(&state.db, &settings, p) {
            if r.ok {
                return r;
            }
        }
    }
    // Windows Hello, when the machine has it, is offered on top.
    if let Ok(r) = auth::verify_hello(&state.db, &settings) {
        if r.ok {
            return r;
        }
    }
    AuthResult::fail("pin", "That password was not accepted.", Some(0))
}

/// Answer the open prompt. `pin` and `phrase` are both optional so the user can
/// type whichever factor they configured.
#[tauri::command]
pub fn answer_prompt(
    state: St<'_>,
    token: String,
    pin: Option<String>,
    phrase: Option<String>,
) -> BvResult<AuthResult> {
    let Some(prompt) = live_prompt(&state, &token) else {
        return Ok(AuthResult::fail(
            "prompt",
            "This prompt has expired. Open the item again.",
            None,
        ));
    };
    let result = verify_for_prompt(&state, pin.as_deref(), phrase.as_deref());
    if !result.ok {
        state.log_event("prompt_denied", "warn", "Authentication refused at a prompt", None);
        return Ok(result);
    }
    // A correct factor releases exactly what this prompt was raised for, and
    // nothing else.
    match prompt.target.clone() {
        PromptTarget::App { rule_id, process } => {
            let _ = state.db.set_app_locked(rule_id, false);
            let name = platform::applock::normalise_name(&process);
            state
                .authorizations
                .lock()
                .grant(&name, Duration::from_secs(300));
            state.pending_blocks.lock().remove(&name);
            // The unauthorised copy was terminated, so put the application back
            // the way the user asked for it.
            if !platform::applock::is_running(&name) {
                if let Some(rule) = state
                    .db
                    .list_protected_apps()
                    .ok()
                    .and_then(|rules| rules.into_iter().find(|r| r.id == rule_id))
                {
                    let exe = rule.exe_path.clone();
                    let spawned = platform::launcher::launch(&exe, None, None);
                    if let Err(e) = spawned {
                        log::warn!("could not relaunch {exe}: {e}");
                    }
                }
            }
            state.log_event(
                "app_authorized",
                "info",
                "Protected application released at its prompt",
                Some(&process),
            );
        }
        PromptTarget::Path { item_id, path } => {
            let snapshot = state.db.acl_snapshot(item_id).ok().flatten();
            let p = std::path::Path::new(&path);
            if p.exists() {
                if let Err(e) = platform::fileprot::open(p, snapshot.as_deref()) {
                    log::warn!("could not release {path}: {e}");
                }
            }
            let _ = state.db.set_item_protected(item_id, false);
            state.log_event(
                "file_revealed",
                "warn",
                "Protected item released at its prompt",
                Some("re-seals on the next lock"),
            );
            // Open it here rather than waiting for the window to ask. The token
            // is consumed a few lines below, and the open that raised the prompt
            // cannot be resumed, so this is the only chance to hand the item to
            // its application. It opens behind the prompt window, which is
            // playing its release animation on top.
            if p.exists() {
                if let Err(e) = platform::launcher::open_with_default(p) {
                    log::warn!("could not open {path}: {e}");
                }
            }
        }
    }
    close_prompt(&state, Some(&token));
    state.emit(CoreEvent::Status(build_status(&state)));
    Ok(result)
}

/// Abandon the open prompt. The protected item stays sealed.
#[tauri::command]
pub fn dismiss_prompt(state: St<'_>, token: String) -> BvResult<()> {
    close_prompt(&state, Some(&token));
    Ok(())
}


// =============================================================== automation

#[tauri::command]
pub fn run_automation(state: St<'_>, action: String, payload: Option<String>) -> BvResult<()> {
    crate::engine::run_action(&state, &action, payload.as_deref())?;
    state.log_event("voice_command", "info", &format!("Automation: {action}"), payload.as_deref());
    Ok(())
}

/// Push-to-talk: the user pressed the microphone button, so what is said next is
/// taken as a command even though the wake phrase was not spoken.
///
/// This is not a new listening path. It arms the router for one utterance — the
/// engine's own capture and the word recogniser do the work — and it runs a
/// command only when a phrase actually matched. Unrecognised speech is dropped
/// exactly as it is when listening passively, so the button cannot be used to
/// brute-force a command, and it can never unlock: unlocking still goes through
/// the passphrase check and the second factor.
#[tauri::command]
pub fn listen_once(state: St<'_>) -> VoiceStatus {
    // The mic button must always open the microphone. If the always-on engine
    // is off — because the user stopped it, or it never started — arming the
    // router alone would leave the button doing nothing, which reads as "the
    // mic is broken". Bring the capture and the word recogniser up first, then
    // arm. Starting the engine here does not widen what can run: an utterance
    // still has to match a command, and unlocking still needs the passphrase
    // and the second factor.
    if !state.voice.is_running() {
        let settings = state.settings();
        state.voice.set_wake_phrase(Some(settings.wake_phrase.clone()));
        let st = state.voice.start();
        state.emit(CoreEvent::Voice(st));
        state.log_event("engine_start", "info", "Voice engine started from mic button", None);
    }
    crate::engine::sync_stt(&state);

    state.router.lock().arm_once(Duration::from_secs(12));
    state.emit(CoreEvent::Voice(state.voice.status()));
    state.voice.status()
}

#[tauri::command]
pub fn press_keys(combo: String) -> BvResult<()> {
    platform::automation::press_keys(&combo)
}

#[tauri::command]
pub fn click_mouse(x: Option<i32>, y: Option<i32>, button: Option<String>) -> BvResult<()> {
    platform::automation::click(x, y, &button.unwrap_or_else(|| "left".into()))
}

#[tauri::command]
pub fn scroll_mouse(dx: i32, dy: i32) -> BvResult<()> {
    platform::automation::scroll(dx, dy)
}

// ============================================================ service & events

#[tauri::command]
pub fn list_events(state: St<'_>, limit: Option<i64>) -> BvResult<Vec<SecurityEvent>> {
    state.db.list_events(limit.unwrap_or(200).clamp(1, 2000))
}

#[tauri::command]
pub fn clear_events(state: St<'_>) -> BvResult<()> {
    state.db.clear_events()
}

#[tauri::command]
pub fn export_audit(state: St<'_>) -> BvResult<String> {
    let events = state.db.list_events(100_000)?;
    let path = state.log_dir.join(format!(
        "blackvault-audit-{}.json",
        chrono::Local::now().format("%Y%m%d-%H%M%S")
    ));
    std::fs::create_dir_all(&state.log_dir)?;
    let json = serde_json::to_string_pretty(&events)?;
    std::fs::write(&path, json)?;
    Ok(path.to_string_lossy().to_string())
}

#[tauri::command]
pub fn service_status(state: St<'_>) -> bool {
    state.voice.is_running() || state.settings().always_on_voice
}

#[tauri::command]
pub fn start_service(state: St<'_>) -> bool {
    let st = state.voice.start();
    state.emit(CoreEvent::Voice(st));
    true
}

#[tauri::command]
pub fn stop_service(state: St<'_>) -> bool {
    let st = state.voice.stop();
    state.emit(CoreEvent::Voice(st));
    false
}

#[tauri::command]
pub fn set_autostart(enabled: bool) -> BvResult<bool> {
    platform::service::set_autostart(enabled)?;
    Ok(enabled)
}

#[tauri::command]
pub fn open_log_folder(state: St<'_>) -> BvResult<()> {
    std::fs::create_dir_all(&state.log_dir)?;
    platform::launcher::launch(
        "explorer.exe",
        Some(&state.log_dir.to_string_lossy()),
        None,
    )
}

#[tauri::command]
pub fn factory_reset(state: St<'_>) -> BvResult<()> {
    // Release every protected item before wiping the rules, so nothing is left
    // sealed with a rule that no longer exists.
    if let Ok(items) = state.db.list_protected_items() {
        for item in items {
            let snapshot = state.db.acl_snapshot(item.id).ok().flatten();
            let p = std::path::Path::new(&item.path);
            if p.exists() {
                let _ = platform::fileprot::open(p, snapshot.as_deref());
            }
        }
    }
    state.db.factory_reset()?;
    state.voice.set_templates(Vec::new());
    *state.settings.write() = Settings::default();
    state.log_event("setting_changed", "critical", "Factory reset performed", None);
    Ok(())
}

// A small helper used by tests to build a template list from the database.
#[allow(dead_code)]
pub fn templates_from_db(state: &Arc<AppState>) -> Vec<Template> {
    state
        .db
        .templates()
        .unwrap_or_default()
        .into_iter()
        .map(|(phrase, kind, mfcc)| Template { phrase, kind, mfcc })
        .collect()
}

// Ensure the `Manager` import is used on every build target.
#[allow(dead_code)]
fn _use_manager(app: &tauri::AppHandle) -> Option<tauri::WebviewWindow> {
    app.get_webview_window("main")
}
