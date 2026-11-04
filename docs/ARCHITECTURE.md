# BLACKVAULT — Architecture

## 1. Shape of the system

BLACKVAULT is a Tauri 2 application: a Rust core process with a WebView2
frontend, talking over a typed IPC boundary.

```
┌──────────────────────────────────────────────────────────────────────┐
│  WebView2 (TypeScript)                                               │
│  src/                                                                │
│    lib/bridge.ts  ─── invoke() ───────────────┐                     │
│    lib/store.ts   ◄── events ─────────────────┤                     │
│    ui/, panes/                                │                     │
└───────────────────────────────────────────────┼─────────────────────┘
                                                │  tauri IPC
┌───────────────────────────────────────────────▼─────────────────────┐
│  Rust core process                                                   │
│                                                                      │
│  commands.rs   the #[tauri::command] surface (1:1 with bridge.ts)    │
│      │                                                               │
│      ├── auth.rs        authentication policy, rate limiting         │
│      ├── db.rs          SQLite: settings, hashes, rules, audit       │
│      ├── crypto.rs      PBKDF2 hashing, DPAPI sealing                │
│      ├── dsp.rs         MFCC, DTW, speaker vectors (pure)            │
│      ├── voice/         capture → VAD → match → route (voiceprint)   │
│      └── platform/      Windows: lock, hello, applock, fileprot,     │
│                         automation, launcher, service, speech        │
│                                                                      │
│  engine.rs     supervisor thread: voice routing, app guard,          │
│                watchdog, tray                                        │
└──────────────────────────────────────────────────────────────────────┘
```

## 2. Why this split

**The core decides; the UI reflects.** No security decision is made in
TypeScript. The frontend can ask to lock, to unlock, to protect a path — it
cannot decide whether an unlock succeeded, whether a voice match is sufficient,
or whether a protected app may run. Every one of those lives in Rust, where it
can be unit-tested without a browser.

**Pure logic is separated from the platform.** `dsp.rs`, `voice/matcher.rs`,
`voice/router.rs` and the policy functions in `auth.rs` take plain data and
return plain data. They have no Windows calls, no I/O and no globals, which is
why they carry the bulk of the test suite. Everything that touches Win32 is
behind `#[cfg(windows)]` with an explicit `Unsupported` stub elsewhere.

## 3. Threads

| Thread | Owns | Lifetime |
|---|---|---|
| Tauri main | the window, the event loop | process |
| `blackvault-voice` | the cpal capture stream, VAD, MFCC extraction | restarts on device failure |
| `blackvault-speech` | the Windows SAPI recogniser and its event loop | restarts on device failure |
| `blackvault-hotkey` | the `Ctrl+Alt+B` emergency-release hotkey message loop | process |
| `blackvault-supervisor` | app-guard polling, voice watchdog, auto-relock, grant pruning | process |

### Word recognition vs. voice identity

There are two listeners, and they answer different questions:

* **`voice/` (MFCC + DTW)** answers *who is speaking*. It compares an utterance
  against enrolled voiceprint templates and is used for the optional
  voice-identity factor.
* **`platform/speech.rs` (Windows SAPI)** answers *what was said*. It runs a
  speech-recognition grammar built from the wake phrase, the unlock phrase and
  every enabled command, and reports recognised text.

* **`platform/stt.rs` (the ZYRAXON endpoint)** answers *what was said* as well,
  and it is the path that works on a machine where SAPI has no grammar for the
  user's phrases. It takes the 16 kHz PCM the capture thread already produced and
  posts it to the endpoint in a WAV container, then feeds the transcript into the
  same `on_text` path as SAPI. It runs alongside SAPI rather than instead of it:
  whichever hears the command first wins, and a dedup guard in `execute_command`
  stops one utterance firing twice.

Commands are matched on the recognised **text**, which is what makes a
newly-typed command work immediately: no recording step is required. When the
overlay is up and a passphrase is configured, a free-form dictation grammar is
armed as well, so the passphrase can be spoken. Speech is only ever an input
method — the phrase is still checked against its PBKDF2 hash and the second
factor still stands between the words and the desktop.

When no recogniser is installed the engine says so (`speechAvailable: false` on
the status) rather than implying voice control works; the voiceprint path still
recognises phrases that have been taught by voice.

The voice thread never blocks on the database or on the UI: it pushes finished
utterances into a sink callback, which runs the match and the route. The
supervisor sleeps 600 ms between ticks and does its heavier work (process
enumeration) at most once every 1.2 s.

Shared state is one `Arc<AppState>`:

```rust
pub struct AppState {
    db: Db,                          // Arc<Mutex<Connection>>
    settings: RwLock<Settings>,      // read-mostly
    mode: RwLock<SecurityMode>,      // unlocked | locking | locked | unlocking
    voice: VoiceEngine,              // atomics + mutexes internally
    authorizations: Mutex<Authorizations>,  // app-lock grants
    partial_auth: AtomicBool,        // passphrase cleared, awaiting 2nd factor
    unlock_token: Mutex<Option<String>>,
    ...
}
```

## 4. Data flow: locking

```
voice "lock this pc"
  → engine::on_utterance
  → matcher::best_match  (DTW against enrolled templates)
  → router::route        (locked? wake required? score above threshold?)
  → engine::execute_command
  → commands::perform_lock
      1. main window: borderless, top-most, skip-taskbar, fullscreen
      2. engine::deploy_overlays   → one window per secondary monitor
      3. platform::lock::hide_cursor / set_task_switch_blocked
      4. reseal_items              → re-apply NTFS deny + EFS
      5. emit Mode{locked} + Status
```

The desktop is hidden by being covered. Nothing is suspended, no process is
touched, no Windows security mechanism is altered.

## 5. Data flow: unlocking

```
voice "unlock this pc" (or the enrolled passphrase)
  → router accepts only unlock-class matches while locked
  → auth::verify_voice:
        a) normalise the recognised words, verify against the stored
           PBKDF2 hash of the passphrase        → wrong words: fail
        b) cosine(utterance speaker vector, enrolled template)
           ≥ threshold                          → wrong voice: fail
  → if settings.require_second_factor:
        emit Challenge{factors}; wait for PIN / pattern / Hello
  → commands::finish_unlock
      1. window back to normal
      2. retract overlays, show cursor
      3. emit Unlocked{token}; the UI plays the unlock ceremony
      4. audit event
```

A voice match alone can never complete an unlock when a second factor is
configured, and it can never complete one at all when the passphrase hash does
not match — the hash check comes first, and it is not bypassable by sounding
like the user.

## 6. Voice engine internals

```
cpal stream (any rate, any channel count)
  → downmix to mono
  → LinearResampler → 16 kHz
  → 20 ms blocks
  → Vad: adaptive noise floor, 3-block open debounce, 12-block close
  → utterance buffer (capped at 6 s)
  → dsp::voiced_frames   MFCC (26 mel filters → 13 cepstra, CMN applied)
  → dsp::speaker_vector  mean+σ per coefficient, L2-normalised (26 dims)
  → matcher::best_match  DTW distance → similarity score
  → router::route
```

Recognition is speaker-dependent template matching: you teach a phrase once and
it is matched acoustically. This is fully offline, needs no model download, and
is what makes the wake phrase and command phrases work out of the box. The
`vosk` feature flag is reserved for plugging in an open-vocabulary model later;
it is off by default so the shipped build has no model dependency.

If the audio device disappears, the stream errors or the channel closes, the
engine loop backs off (400 ms → 10 s) and reopens the device. It never gives up
silently. The supervisor thread independently notices a stopped engine and
restarts it.

## 7. Security boundaries

| Boundary | Enforced by | Notes |
|---|---|---|
| Unlock | `auth.rs` | hash + optional speaker check + optional second factor + rate limit |
| Protected app | `platform/applock.rs` | process watchdog terminates while locked; grants are time-bounded |
| Protected file | `platform/fileprot.rs` | NTFS deny-ACL + EFS; snapshot restored on release |
| Secrets at rest | `crypto.rs` | PBKDF2-HMAC-SHA256, 210 000 iterations, 16-byte salt, constant-time compare |
| Reversible data | `crypto.rs` | DPAPI (current user), never a plaintext key on disk |
| IPC | `tauri.conf.json` CSP + capability list | `script-src 'self'`, no remote origins, no eval |
| Elevated windows | Windows UIPI | BLACKVAULT cannot type into a higher-integrity window, and does not attempt to |

## 8. Database

One SQLite file at `%LOCALAPPDATA%\BLACKVAULT\blackvault.db`, WAL mode, 5 s busy
timeout. Tables: `settings`, `secrets`, `voice_commands`, `launcher_apps`,
`protected_apps`, `protected_items`, `events`, `voice_identity`,
`voice_templates`, `auth_state`.

The `secrets` table holds only PBKDF2 strings and DPAPI-sealed blobs. There is
no column anywhere in the schema that can hold a plaintext credential.

## 9. Startup and recovery

- Per-user `HKCU\...\Run` entry (`--tray`), no elevation needed.
- A named mutex gives single-instance behaviour so two copies cannot fight over
  the database.
- A `running.lock` marker is written on start and removed on clean exit; its
  presence at launch is logged as an unclean shutdown and triggers engine
  recovery.
- Closing the main window hides it to the tray rather than ending the process,
  so the voice listener and the app guard survive.
