//! BLACKVAULT — the background engine.
//!
//! Three long-lived concerns live here, all driven from one supervisor thread:
//!
//!   * **Voice routing** — the voice engine's utterances are matched against the
//!     enrolled templates and routed (`voice::router`) into a wake, a command or
//!     an unlock attempt. Command actions are executed here, never in the UI.
//!   * **Application guard** — a fast loop over the process table that enforces
//!     the app-lock rules and re-locks apps when they exit.
//!   * **Watchdog** — restarts the voice engine if it stops unexpectedly, and
//!     applies the idle / auto-relock timers.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tauri::Manager;

use crate::models::{SecurityMode, VoiceStatus};
use crate::platform::applock::{self, GuardDecision};
use crate::state::{AppState, CoreEvent};
use crate::voice::matcher::Template;
use crate::voice::router::RouteKind;
use crate::voice::{Utterance, VoiceEngine};

/// How long a protected application may keep running while the authentication
/// prompt is up. Long enough to type a PIN, short enough that a locked app does
/// not stay open indefinitely.
const BLOCK_GRACE: Duration = Duration::from_secs(20);

/// Start every background concern. Idempotent per process.
pub fn spawn(state: Arc<AppState>) {
    // 1. Wire the voice engine's sinks.
    let s = Arc::clone(&state);
    state.voice.set_sink(Arc::new(move |u: Utterance| on_utterance(&s, u)));

    let s = Arc::clone(&state);
    state.voice.set_status_sink(Arc::new(move |st: VoiceStatus| {
        s.emit(CoreEvent::Voice(st));
    }));

    // 1b. Offline word recognition. This is what actually understands the
    // user's phrases; the DTW engine above is kept for the voiceprint check.
    let s = Arc::clone(&state);
    state
        .speech
        .set_sink(Arc::new(move |text: String| on_text(&s, Recogniser::Sapi, &text)));
    let speech_ok = crate::platform::speech::SpeechEngine::available();
    state
        .speech_available
        .store(speech_ok, Ordering::Relaxed);
    if !speech_ok {
        log::warn!(
            "no offline speech recogniser is installed; commands will only work \
             after a phrase has been taught by voice"
        );
    }

    // 2. Rebuild the recogniser templates from the database.
    rebuild_templates(&state);

    // 3. Start listening if the user asked for always-on voice.
    let settings = state.settings();
    if settings.always_on_voice {
        let st = state.voice.start();
        state.voice.set_wake_phrase(Some(settings.wake_phrase.clone()));
        state.emit(CoreEvent::Voice(st));
    }
    restart_speech(&state);

    // 3b. Word recognition through the ZYRAXON STT endpoint. This runs
    // alongside the offline recogniser rather than instead of it: whichever
    // hears the command first wins, and the dedup guard above stops a double
    // fire. It is the path that works when SAPI is missing or has no grammar
    // for the user's phrases.
    apply_stt_settings(&state);
    if state.settings().stt_enabled {
        let s = Arc::clone(&state);
        state
            .stt
            .start(Arc::new(move |text: String| on_text(&s, Recogniser::Sapi, &text)));
    }

    // 4. Supervisor thread: app guard + watchdog + timers.
    let sup = Arc::clone(&state);
    std::thread::Builder::new()
        .name("blackvault-supervisor".into())
        .spawn(move || supervisor(sup))
        .ok();

    // 5. Tray icon.
    build_tray(&state);

    // 6. Emergency release hotkey (Ctrl+Alt+B). This is the guarantee that the
    // overlay can never become a one-way door.
    let hot = Arc::clone(&state);
    crate::platform::lock::install_emergency_hotkey(Box::new(move || {
        crate::commands::finish_unlock(&hot);
    }));
}

/// Load every phrase template and hand them to the matcher.
pub fn rebuild_templates(state: &Arc<AppState>) {
    let settings = state.settings();
    let mut templates: Vec<Template> = Vec::new();

    // The wake phrase.
    if settings.wake_phrase_enabled {
        if let Some(mfcc) = template_for(state, &settings.wake_phrase) {
            templates.push(Template {
                phrase: settings.wake_phrase.clone(),
                kind: "wake".into(),
                mfcc,
            });
        }
    }

    // The unlock passphrase.
    if let Ok(Some(_)) = state.db.get_secret(crate::auth::SECRET_PASSPHRASE) {
        // The passphrase's spoken form is the enrolled "unlock" phrase. We use
        // the phrase the user trained for "unlock this pc" if present.
        if let Some(mfcc) = template_for(state, "unlock this pc") {
            templates.push(Template {
                phrase: "unlock this pc".into(),
                kind: "unlock".into(),
                mfcc,
            });
        }
    }

    // Every enabled command that has been taught.
    if let Ok(commands) = state.db.list_commands() {
        for c in commands.iter().filter(|c| c.enabled) {
            if c.action_type == "lock" || c.action_type == "unlock" {
                continue; // handled explicitly above
            }
            if let Some(mfcc) = template_for(state, &c.phrase) {
                templates.push(Template {
                    phrase: c.phrase.clone(),
                    kind: "command".into(),
                    mfcc,
                });
            }
        }
    }

    log::info!("voice recogniser armed with {} template(s)", templates.len());
    state.voice.set_templates(templates);
}

fn template_for(state: &Arc<AppState>, phrase: &str) -> Option<Vec<Vec<f32>>> {
    state
        .db
        .templates()
        .ok()?
        .into_iter()
        .find(|(p, _, _)| p == phrase)
        .map(|(_, _, m)| m)
        .filter(|m| !m.is_empty())
}

// ------------------------------------------------- offline word recognition

/// Every phrase the offline recogniser should listen for right now: the wake
/// phrase, the unlock phrase and each enabled command.
pub fn speech_phrases(state: &Arc<AppState>) -> Vec<String> {
    let settings = state.settings();
    let mut out: Vec<String> = Vec::new();
    if settings.wake_phrase_enabled && !settings.wake_phrase.trim().is_empty() {
        out.push(settings.wake_phrase.clone());
    }
    out.push("unlock this pc".into());
    if let Ok(commands) = state.db.list_commands() {
        for c in commands.iter().filter(|c| c.enabled) {
            if !c.phrase.trim().is_empty() && !out.contains(&c.phrase) {
                out.push(c.phrase.clone());
            }
        }
    }
    out
}

/// (Re)load the offline recogniser's grammar. Safe to call at any time.
///
/// While the overlay is up and the passphrase factor is configured, a dictation
/// grammar is armed too, so the user can speak the passphrase to unlock.
pub fn restart_speech(state: &Arc<AppState>) {
    if !state.speech_available.load(Ordering::Relaxed) {
        return;
    }
    let phrases = speech_phrases(state);
    let settings = state.settings();
    let dictation = state.mode() != SecurityMode::Unlocked
        && settings.listen_while_locked
        && state
            .db
            .get_secret(crate::auth::SECRET_PASSPHRASE)
            .ok()
            .flatten()
            .is_some();
    state.speech.start(phrases, dictation);
}

/// Re-seal any protected item whose deny ACE was removed or edited away.
///
/// A protection that quietly stops being enforced is worse than no protection,
/// so the guard checks the DACL itself rather than trusting that the seal from
/// when the item was added is still in place.
pub fn enforce_file_protection(state: &Arc<AppState>) {
    let Ok(items) = state.db.list_protected_items() else {
        return;
    };
    for item in items {
        if !item.protected {
            continue;
        }
        let p = std::path::Path::new(&item.path);
        if !p.exists() {
            continue;
        }
        if crate::platform::fileprot::is_sealed(p) {
            continue;
        }
        match crate::platform::fileprot::seal_item(p) {
            Ok(applied) if applied.acl || applied.encrypted => {
                log::warn!("re-sealed {} after the protection was removed", item.path);
                state.log_event(
                    "file_protected",
                    "warn",
                    &format!("Re-sealed {}", item.display_name),
                    Some("the deny permission had been removed"),
                );
            }
            Ok(_) => {
                state.log_event(
                    "file_protected",
                    "warn",
                    &format!("Could not re-seal {}", item.display_name),
                    Some("run BLACKVAULT as an administrator"),
                );
            }
            Err(e) => log::warn!("re-seal failed for {}: {e}", item.path),
        }
    }
}

/// Which recogniser produced a transcript. It is the dedup key: the two
/// recognisers hear one utterance, so the same phrase from two sources inside a
/// short window is a duplicate — but the same phrase twice from one source is
/// the user deliberately repeating a command such as "next".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recogniser {
    Sapi,
    Endpoint,
    /// A taught phrase matched by the offline voiceprint engine.
    Template,
}

/// Push the STT-related settings into the worker.
pub fn apply_stt_settings(state: &Arc<AppState>) {
    let settings = state.settings();
    state.stt.set_config(crate::platform::stt::SttConfig {
        endpoint: settings.stt_endpoint.clone(),
        language: settings.stt_language.clone(),
        live: settings.stt_live,
        min_confidence: settings.stt_min_confidence,
    });
}

/// Start or stop the STT worker to match the setting.
pub fn sync_stt(state: &Arc<AppState>) {
    apply_stt_settings(state);
    let settings = state.settings();
    if settings.stt_enabled && !state.stt.is_running() {
        let s = Arc::clone(state);
        state
            .stt
            .start(Arc::new(move |text: String| on_text(&s, Recogniser::Endpoint, &text)));
    } else if !settings.stt_enabled && state.stt.is_running() {
        state.stt.stop();
    }
}

/// A phrase was recognised as *words* by the offline engine.
///
/// This is the path that makes the default commands work: the words are matched
/// against the command list by text, so a command runs the moment it is typed in
/// the editor — no recording step required.
fn on_text(state: &Arc<AppState>, source: Recogniser, text: &str) {
    let heard = crate::voice::matcher::normalise(text);
    if heard.is_empty() {
        return;
    }
    let settings = state.settings();
    let locked = state.mode() != SecurityMode::Unlocked;
    let wake = crate::voice::matcher::normalise(&settings.wake_phrase);

    state.voice.note_heard(&heard);
    state.emit(CoreEvent::Heard {
        text: heard.clone(),
        is_final: true,
    });

    // The wake phrase arms commands and never runs anything itself.
    if settings.wake_phrase_enabled && !wake.is_empty() && heard == wake {
        state
            .router
            .lock()
            .route(false, &match_for(&heard, "wake"), true, Instant::now());
        state.toast("Listening", "Say a command.", "info");
        return;
    }

    if locked {
        // On the lock screen the words can *open* the unlock prompt, and the
        // spoken passphrase is accepted as the first factor — exactly like the
        // typed field. Identity is never proven by words alone: the passphrase
        // check is the same PBKDF2 comparison, and the second factor still
        // stands between the words and the desktop.
        if heard == "unlock this pc" || heard == wake {
            let status = crate::commands::begin_unlock_now(state);
            state.emit(CoreEvent::Status(status));
            return;
        }
        if state
            .db
            .get_secret(crate::auth::SECRET_PASSPHRASE)
            .ok()
            .flatten()
            .is_some()
            && !state.partial_auth.load(Ordering::Relaxed)
        {
            let r = crate::commands::spoken_passphrase(state, &heard);
            if r.ok {
                state.toast("Voice phrase accepted", &r.message, "info");
            } else if r.remaining_attempts != Some(0) {
                state.toast("Unlock refused", &r.message, "warn");
            }
        }
        return;
    }

    if settings.require_wake_for_commands && !state.router.lock().armed(Instant::now()) {
        state.toast("Wake phrase required", "Say the wake phrase first.", "warn");
        return;
    }

    let commands = match state.db.list_commands() {
        Ok(c) => c,
        Err(_) => return,
    };
    let phrases: Vec<String> = commands
        .iter()
        .filter(|c| c.enabled)
        .map(|c| c.phrase.clone())
        .collect();
    let Some((phrase, _score)) = crate::voice::matcher::match_text(&heard, &phrases) else {
        state.toast(
            "Not recognised",
            &format!("\u{201c}{heard}\u{201d} matches no command."),
            "warn",
        );
        // A phrase that is not a command may still be the secret passphrase.
        // Verifying it is the same PBKDF2 comparison the lock screen runs, so
        // speech is only the input method here — never the proof.
        if !heard.starts_with("open ")
            && state
                .db
                .get_secret(crate::auth::SECRET_PASSPHRASE)
                .ok()
                .flatten()
                .is_some()
        {
            let r = crate::commands::spoken_passphrase(state, &heard);
            if r.ok {
                state.toast("Voice phrase accepted", &r.message, "info");
            }
        }
        return;
    };

    state.router.lock().disarm();
    state.voice.note_command(&phrase);
    state.emit(CoreEvent::Heard {
        text: phrase.clone(),
        is_final: true,
    });
    execute_command(state, &phrase, source);
}

fn match_for(phrase: &str, kind: &str) -> crate::voice::matcher::Match {
    crate::voice::matcher::Match {
        phrase: phrase.into(),
        kind: kind.into(),
        score: 1.0,
        distance: 0.0,
    }
}

// ------------------------------------------------------------- voice routing

fn on_utterance(state: &Arc<AppState>, utterance: Utterance) {
    if utterance.mfcc.is_empty() {
        return;
    }
    let settings = state.settings();

    // Hand the audio to the word recogniser. This is what makes commands work;
    // the DTW match below is the voiceprint check, which is a different job.
    // The worker drops the utterance when it is still busy, so capture is never
    // stalled by a slow response.
    if settings.stt_enabled && !utterance.pcm16.is_empty() {
        state.stt.submit(utterance.pcm16.clone());
    }

    let locked = state.mode() == SecurityMode::Locked;

    // The recogniser hands back templates; score the utterance against them.
    let templates = state
        .db
        .templates()
        .unwrap_or_default()
        .into_iter()
        .map(|(phrase, kind, mfcc)| Template { phrase, kind, mfcc })
        .collect::<Vec<_>>();
    if templates.is_empty() {
        return;
    }

    let Some(best) = crate::voice::matcher::best_match(&utterance.mfcc, &templates, 0.9, 6.0) else {
        return;
    };

    let outcome = state
        .router
        .lock()
        .route(locked, &best, settings.require_wake_for_commands, Instant::now());

    match outcome.kind {
        RouteKind::Ignore => {}
        RouteKind::Wake => {
            state.voice.note_heard(&best.phrase);
            state.emit(CoreEvent::Heard {
                text: best.phrase.clone(),
                is_final: true,
            });
        }
        RouteKind::Unlock => {
            state.voice.note_heard(&best.phrase);
            state.emit(CoreEvent::Heard {
                text: best.phrase.clone(),
                is_final: true,
            });
            // Verify: the words must hash-match the passphrase and the voice
            // must match the enrolled print. Then the caller decides whether a
            // second factor is still required.
            match crate::auth::verify_voice(
                &state.db,
                &settings,
                &best.phrase,
                &utterance.speaker,
            ) {
                Ok(result) if result.ok => {
                    state.emit(CoreEvent::Auth(result.clone()));
                    if settings.require_second_factor {
                        // Phrase + voice cleared: ask for the second factor.
                        state.partial_auth.store(true, Ordering::Relaxed);
                        let factors = crate::auth::second_step_factors(
                            &state.db,
                            &settings,
                            state.hello_available(),
                        );
                        state.emit(CoreEvent::Challenge { factors });
                    } else {
                        crate::commands::finish_unlock(state);
                    }
                }
                Ok(result) => {
                    state.emit(CoreEvent::Auth(result.clone()));
                    state.toast("Unlock refused", &result.message, "warn");
                }
                Err(e) => {
                    log::warn!("voice verification error: {e}");
                }
            }
        }
        RouteKind::Command => {
            state.voice.note_heard(&best.phrase);
            state.voice.note_command(&best.phrase);
            state.emit(CoreEvent::Heard {
                text: best.phrase.clone(),
                is_final: true,
            });
            execute_command(state, &best.phrase, Recogniser::Template);
        }
    }
}

/// Look up a command by phrase and run its action.
pub fn execute_command(state: &Arc<AppState>, phrase: &str, source: Recogniser) {
    // Both recognisers hear the same utterance, and a live stream reports a
    // partial and then the final segment of it. Dropping a repeat only when the
    // *other* recogniser produced it keeps a deliberate "next, next" working.
    {
        let mut last = state.last_command.lock();
        if let Some((prev, at, from)) = last.as_ref() {
            if prev == phrase && *from != source && at.elapsed() < Duration::from_millis(1500) {
                return;
            }
        }
        *last = Some((phrase.to_string(), Instant::now(), source));
    }

    let commands = match state.db.list_commands() {
        Ok(c) => c,
        Err(_) => return,
    };
    let Some(cmd) = commands.iter().find(|c| c.phrase == phrase) else {
        return;
    };
    if !cmd.enabled {
        return;
    }

    // Commands flagged `requires_auth` need a recent, verified factor.
    if cmd.requires_auth && !recent_auth(state, 300) {
        state.emit(CoreEvent::Command {
            phrase: cmd.phrase.clone(),
            action: "authentication required".into(),
            ok: false,
        });
        state.toast(
            "Authentication required",
            "Verify a factor before running this command.",
            "warn",
        );
        return;
    }

    let result = run_action(state, &cmd.action_type, cmd.payload.as_deref());
    let ok = result.is_ok();
    let detail = match &result {
        Ok(()) => cmd.action.clone(),
        Err(e) => e.to_string(),
    };
    state.emit(CoreEvent::Command {
        phrase: cmd.phrase.clone(),
        action: detail.clone(),
        ok,
    });
    let _ = state.db.bump_command_hits(&cmd.phrase);
    state.log_event(
        "voice_command",
        if ok { "info" } else { "warn" },
        &format!("\"{}\"", cmd.phrase),
        Some(&detail),
    );
}

fn recent_auth(state: &Arc<AppState>, within_secs: i64) -> bool {
    let Ok(Some(at)) = state.db.last_auth_time() else {
        return false;
    };
    let Ok(t) = chrono::DateTime::parse_from_rfc3339(&at) else {
        return false;
    };
    (chrono::Utc::now() - t.with_timezone(&chrono::Utc)).num_seconds() <= within_secs
}

/// Execute one action. Shared by the voice path and the manual Automation pane.
pub fn run_action(
    state: &Arc<AppState>,
    action: &str,
    payload: Option<&str>,
) -> crate::error::BvResult<()> {
    use crate::platform as p;
    match action {
        "lock" => {
            crate::commands::perform_lock(state, "voice command");
            Ok(())
        }
        "unlock" => Ok(()), // only meaningful from the lock screen
        "launch_app" => {
            let target = payload.unwrap_or("").to_string();
            launch_by_name(state, &target)
        }
        "close_app" => {
            let target = payload.unwrap_or("").trim_end_matches(".exe").to_string();
            p::launcher::close_process(&target)
        }
        "run_program" => {
            let path = payload.unwrap_or("");
            p::launcher::launch(path, None, None)
        }
        "open_url" => {
            let url = payload.unwrap_or("https://www.google.com");
            open_url(url)
        }
        "open_folder" => {
            let path = payload.unwrap_or("");
            p::launcher::launch("explorer.exe", Some(path), None)
        }
        other => p::automation::run_action(other, payload),
    }
}

fn launch_by_name(state: &Arc<AppState>, name: &str) -> crate::error::BvResult<()> {
    let apps = state.db.list_apps()?;
    let needle = name.trim().to_ascii_lowercase();
    // Match by exact name, then by voice phrase, then by substring.
    let found = apps
        .iter()
        .filter(|a| a.enabled)
        .find(|a| a.name.to_ascii_lowercase() == needle)
        .or_else(|| {
            apps.iter().filter(|a| a.enabled).find(|a| {
                a.voice_phrase
                    .as_deref()
                    .map(|p| p.to_ascii_lowercase().contains(&needle) || needle.contains(&p.to_ascii_lowercase()))
                    .unwrap_or(false)
            })
        })
        .or_else(|| {
            apps.iter()
                .filter(|a| a.enabled)
                .find(|a| a.name.to_ascii_lowercase().contains(&needle) || needle.contains(&a.name.to_ascii_lowercase()))
        });
    let Some(app) = found else {
        return Err(crate::error::BvError::msg(format!(
            "\"{name}\" is not in the approved launcher list"
        )));
    };
    crate::platform::launcher::launch(&app.command, app.args.as_deref(), app.cwd.as_deref())?;
    let _ = state.db.bump_app_launch(app.id);
    // Keep a protected app authorised while it runs, if it is protected.
    if let Ok(rules) = state.db.list_protected_apps() {
        if let Some(rule) = rules.iter().find(|r| {
            app.command.to_ascii_lowercase().ends_with(&r.process_name.to_ascii_lowercase())
        }) {
            state
                .authorizations
                .lock()
                .grant(&rule.process_name, Duration::from_secs(300));
        }
    }
    Ok(())
}

#[cfg(windows)]
fn open_url(url: &str) -> crate::error::BvResult<()> {
    // Hand the URL to the shell, but only for the schemes we allow.
    let allowed = ["http://", "https://", "ms-settings:", "mailto:", "ms-windows-store:"];
    if !allowed.iter().any(|p| url.starts_with(p)) {
        return Err(crate::error::BvError::msg("that URL scheme is not permitted"));
    }
    use std::os::windows::process::CommandExt;
    std::process::Command::new("rundll32.exe")
        .arg("url.dll,FileProtocolHandler")
        .arg(url)
        .creation_flags(0x08000000) // CREATE_NO_WINDOW
        .spawn()
        .map_err(|e| crate::error::BvError::msg(format!("could not open the URL: {e}")))?;
    Ok(())
}

#[cfg(not(windows))]
fn open_url(_url: &str) -> crate::error::BvResult<()> {
    Err(crate::error::BvError::Unsupported("opening URLs".into()))
}

// ---------------------------------------------------------- supervisor loop

fn supervisor(state: Arc<AppState>) {
    let mut router_guard = Instant::now();
    let mut last_guard = Instant::now();
    let mut last_file_guard = Instant::now();
    let mut last_voice_check = Instant::now();
    let mut unlocked_at = Instant::now();
    let mut last_mode = state.mode();

    loop {
        std::thread::sleep(Duration::from_millis(600));
        let settings = state.settings();

        // --- mode transitions -------------------------------------------
        let mode = state.mode();
        if mode != last_mode {
            if mode == SecurityMode::Unlocked {
                unlocked_at = Instant::now();
                crate::platform::lock::set_task_switch_blocked(false);
                retract_overlays(&state);
            }
            // The lock screen needs the dictation grammar; the desktop does not.
            restart_speech(&state);
            last_mode = mode;
        }

        // --- voice watchdog ---------------------------------------------
        if last_voice_check.elapsed() > Duration::from_secs(5) {
            last_voice_check = Instant::now();
            let should_run = settings.always_on_voice
                && (mode == SecurityMode::Unlocked || settings.listen_while_locked);
            if should_run && !state.voice.is_running() {
                log::warn!("voice engine stopped unexpectedly; restarting");
                state.voice.bump_recovery();
                let st = state.voice.start();
                state.emit(CoreEvent::Voice(st));
                state.log_event(
                    "engine_recovery",
                    "warn",
                    "Voice engine recovered",
                    Some("the listener had stopped and was restarted"),
                );
            }
            // The offline recogniser must survive a device change or a crash
            // too: it is the component that actually understands words.
            if should_run
                && state.speech_available.load(Ordering::Relaxed)
                && !state.speech.is_running()
            {
                log::warn!("offline recogniser stopped unexpectedly; restarting");
                state.voice.bump_recovery();
                restart_speech(&state);
                let err = state.speech.last_error();
                state.log_event(
                    "engine_recovery",
                    "warn",
                    "Speech recogniser recovered",
                    err.as_deref(),
                );
            }
        }

        // --- STT watchdog ------------------------------------------------
        // The word recogniser is the component that understands commands, so it
        // gets the same treatment as the listener: if it dies, bring it back.
        if last_voice_check.elapsed() > Duration::from_secs(5) {
            if settings.stt_enabled && !state.stt.is_running() {
                log::warn!("speech-to-text worker stopped unexpectedly; restarting");
                sync_stt(&state);
                state.log_event(
                    "engine_recovery",
                    "warn",
                    "Speech-to-text worker recovered",
                    state.stt.last_error().as_deref(),
                );
            }
        }

        // --- app guard ---------------------------------------------------
        if last_guard.elapsed() > Duration::from_millis(400) {
            last_guard = Instant::now();
            guard_apps(&state);
        }

        // --- file guard --------------------------------------------------
        // Slower than the process guard: walking the protected list touches the
        // filesystem, and a tampered DACL is not a race we have to win in
        // milliseconds.
        if last_file_guard.elapsed() > Duration::from_secs(5) {
            last_file_guard = Instant::now();
            enforce_file_protection(&state);
        }

        // --- idle / auto relock ------------------------------------------
        if settings.auto_relock_secs > 0
            && mode == SecurityMode::Unlocked
            && unlocked_at.elapsed() > Duration::from_secs(settings.auto_relock_secs as u64)
        {
            crate::commands::perform_lock(&state, "auto-relock");
        }

        // --- rate-limit housekeeping ------------------------------------
        if router_guard.elapsed() > Duration::from_secs(60) {
            router_guard = Instant::now();
            state.authorizations.lock().prune();
        }

        if state.shutting_down.load(Ordering::Relaxed) {
            break;
        }
    }
}

fn guard_apps(state: &Arc<AppState>) {
    let rules = match state.db.list_protected_apps() {
        Ok(r) => r,
        Err(_) => return,
    };
    if rules.is_empty() {
        return;
    }
    let running = applock::list_processes();
    let now = Instant::now();
    let mut seen: Vec<String> = Vec::new();

    for rule in &rules {
        let name = applock::normalise_name(&rule.process_name);
        let matching: Vec<_> = running
            .iter()
            .filter(|p| applock::normalise_name(&p.name) == name)
            .collect();
        if matching.is_empty() {
            continue;
        }
        seen.push(name.clone());
        let until = state.authorizations.lock().until(&name);
        let decision = applock::decide(rule.require_auth, rule.locked, true, until, now);
        if decision == GuardDecision::Block {
            // Give the user a chance to authenticate before anything is killed.
            // Killing a protected app the instant it appears is what made the
            // feature feel broken: the app flashed up, vanished, and nothing
            // ever asked for a password.
            let first_seen = *state
                .pending_blocks
                .lock()
                .entry(name.clone())
                .or_insert(now);
            let waited = now.duration_since(first_seen);
            if waited < BLOCK_GRACE {
                // Ask once per grace window, not on every pass of the loop.
                if waited < Duration::from_millis(700) {
                    state.emit(CoreEvent::AppChallenge {
                        rule_id: rule.id,
                        process_name: rule.process_name.clone(),
                        display_name: rule.display_name.clone(),
                        grace_secs: (BLOCK_GRACE - waited).as_secs(),
                    });
                    state.toast(
                        "Protected application",
                        &format!("Authenticate to keep {} running.", rule.display_name),
                        "warn",
                    );
                    state.log_event(
                        "app_challenge",
                        "warn",
                        &format!("{} needs authentication", rule.display_name),
                        Some(&format!("pid {}", matching[0].pid)),
                    );
                }
                continue;
            }
            for p in &matching {
                log::info!("blocking protected process {} ({})", p.name, p.pid);
                if applock::terminate_pid(p.pid) {
                    state.log_event(
                        "app_blocked",
                        "warn",
                        &format!("Blocked {}", rule.display_name),
                        Some(&format!("pid {} terminated", p.pid)),
                    );
                }
            }
            state.pending_blocks.lock().remove(&name);
        } else {
            // Authorised: forget the pending prompt.
            state.pending_blocks.lock().remove(&name);
        }
    }

    // Auto-relock rules whose process has exited.
    for rule in &rules {
        let name = applock::normalise_name(&rule.process_name);
        if rule.auto_relock && rule.locked && !seen.contains(&name) {
            // Process is gone; nothing to do but drop any stale grant.
            state.authorizations.lock().revoke(&name);
        }
    }
    state.pending_blocks.lock().retain(|_, at| at.elapsed() < Duration::from_secs(120));
}

// ------------------------------------------------------------- full-screen mode

/// Create a passive overlay on every monitor except the primary.
pub fn deploy_overlays(state: &Arc<AppState>) {
    let app = match state.app.lock().clone() {
        Some(a) => a,
        None => return,
    };
    let monitors = crate::platform::lock::monitors().unwrap_or_default();
    for (i, m) in monitors.iter().enumerate() {
        if m.primary {
            continue;
        }
        let label = format!("overlay-{i}");
        if app.get_webview_window(&label).is_some() {
            continue;
        }
        let builder = tauri::WebviewWindowBuilder::new(
            &app,
            &label,
            tauri::WebviewUrl::App("index.html?overlay=1".into()),
        )
        .title("BLACKVAULT")
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .focused(false)
        .visible(true)
        .inner_size(m.width as f64, m.height as f64)
        .position(m.x as f64, m.y as f64)
        .background_color(tauri::window::Color(0, 0, 0, 255));
        if let Err(e) = builder.build() {
            log::warn!("could not create overlay for monitor {i}: {e}");
        }
    }
}

/// Close every overlay window.
pub fn retract_overlays(state: &Arc<AppState>) {
    let app = match state.app.lock().clone() {
        Some(a) => a,
        None => return,
    };
    for (_label, window) in app.webview_windows() {
        if window.label().starts_with("overlay-") {
            let _ = window.close();
        }
    }
}

// -------------------------------------------------------------------- tray

fn build_tray(state: &Arc<AppState>) {
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

    let app = match state.app.lock().clone() {
        Some(a) => a,
        None => return,
    };

    let show = MenuItem::with_id(&app, "show", "Open BLACKVAULT", true, None::<&str>).ok();
    let lock = MenuItem::with_id(&app, "lock", "Lock the PC", true, None::<&str>).ok();
    let voice = MenuItem::with_id(&app, "voice", "Toggle voice engine", true, None::<&str>).ok();
    let quit = MenuItem::with_id(&app, "quit", "Quit", true, None::<&str>).ok();

    let menu = match (show, lock, voice, quit) {
        (Some(s), Some(l), Some(v), Some(q)) => Menu::with_items(&app, &[&s, &l, &v, &q]).ok(),
        _ => None,
    };

    let mut builder = TrayIconBuilder::with_id("blackvault-tray")
        .tooltip("BLACKVAULT — Your Voice. Your Control. Your Security.")
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| {
            let state: Arc<AppState> = app.state::<Arc<AppState>>().inner().clone();
            match event.id().as_ref() {
                "show" => {
                    if let Some(w) = app.get_webview_window("main") {
                        let _ = w.show();
                        let _ = w.unminimize();
                        let _ = w.set_focus();
                    }
                }
                "lock" => crate::commands::perform_lock(&state, "tray"),
                "voice" => {
                    if state.voice.is_running() {
                        let st = state.voice.stop();
                        state.emit(CoreEvent::Voice(st));
                    } else {
                        let st = state.voice.start();
                        state.emit(CoreEvent::Voice(st));
                    }
                }
                "quit" => {
                    state.shutting_down.store(true, Ordering::Relaxed);
                    crate::platform::service::mark_clean(&state.data_dir);
                    app.exit(0);
                }
                _ => {}
            }
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
        });

    if let Some(menu) = menu {
        builder = builder.menu(&menu);
    }
    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }
    if let Err(e) = builder.build(&app) {
        log::warn!("could not create the tray icon: {e}");
    }
}

/// Called on the way out so the crash marker is cleared.
pub fn shutdown(state: &Arc<AppState>) {
    state.shutting_down.store(true, Ordering::Relaxed);
    let _ = state.voice.stop();
    crate::platform::service::mark_clean(&state.data_dir);
}

#[allow(dead_code)]
fn engine_used(_: &VoiceEngine) {}
