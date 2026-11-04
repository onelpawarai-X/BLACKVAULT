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

/// Start every background concern. Idempotent per process.
pub fn spawn(state: Arc<AppState>) {
    // 1. Wire the voice engine's sinks.
    let s = Arc::clone(&state);
    state.voice.set_sink(Arc::new(move |u: Utterance| on_utterance(&s, u)));

    let s = Arc::clone(&state);
    state.voice.set_status_sink(Arc::new(move |st: VoiceStatus| {
        s.emit(CoreEvent::Voice(st));
    }));

    // 2. Rebuild the recogniser templates from the database.
    rebuild_templates(&state);

    // 3. Start listening if the user asked for always-on voice.
    let settings = state.settings();
    if settings.always_on_voice {
        let st = state.voice.start();
        state.voice.set_wake_phrase(Some(settings.wake_phrase.clone()));
        state.emit(CoreEvent::Voice(st));
    }

    // 4. Supervisor thread: app guard + watchdog + timers.
    let sup = Arc::clone(&state);
    std::thread::Builder::new()
        .name("blackvault-supervisor".into())
        .spawn(move || supervisor(sup))
        .ok();

    // 5. Tray icon.
    build_tray(&state);
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

// ------------------------------------------------------------- voice routing

fn on_utterance(state: &Arc<AppState>, utterance: Utterance) {
    if utterance.mfcc.is_empty() {
        return;
    }
    let settings = state.settings();
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
            execute_command(state, &best.phrase);
        }
    }
}

/// Look up a command by phrase and run its action.
pub fn execute_command(state: &Arc<AppState>, phrase: &str) {
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
            app.command.to_ascii_lowercase().ends_with(&rule.process_name.to_ascii_lowercase())
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
        }

        // --- app guard ---------------------------------------------------
        if last_guard.elapsed() > Duration::from_millis(1200) {
            last_guard = Instant::now();
            guard_apps(&state);
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
            for p in &matching {
                log::info!("blocking protected process {} ({})", p.name, p.pid);
                if applock::terminate_pid(p.pid) {
                    state.log_event(
                        "app_blocked",
                        "warn",
                        &format!("Blocked {}", rule.display_name),
                        Some(&format!("pid {} terminated", p.pid)),
                    );
                    state.toast(
                        "Application blocked",
                        &format!("{} is protected. Authenticate to run it.", rule.display_name),
                        "warn",
                    );
                }
            }
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
