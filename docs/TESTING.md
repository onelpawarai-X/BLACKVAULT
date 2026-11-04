# BLACKVAULT — Testing and Verification

## 1. How the test suite is organised

The codebase is deliberately split so that almost all of it can be tested
without a Windows machine, a microphone or a screen.

| Layer | Tested how | Where |
|---|---|---|
| DSP — MFCC, DTW, speaker vectors | pure unit tests, synthetic signals | `src-tauri/src/dsp.rs` |
| Phrase matching | pure unit tests | `src-tauri/src/voice/matcher.rs` |
| Voice activity detection | pure unit tests, synthetic energy curves | `src-tauri/src/voice/vad.rs` |
| Command routing policy | pure unit tests | `src-tauri/src/voice/router.rs` |
| Authentication policy | unit tests against an in-memory database | `src-tauri/src/auth.rs` |
| Cryptography | round-trip and negative tests | `src-tauri/src/crypto.rs` |
| Database | unit tests against an in-memory database | `src-tauri/src/db.rs` |
| Settings validation | unit tests on clamping | `src-tauri/src/settings.rs` |
| App-lock policy | pure decision-function tests | `src-tauri/src/platform/applock.rs` |
| Automation key/point parsing | pure unit tests | `src-tauri/src/platform/automation.rs` |
| Launcher argument splitting | pure unit tests | `src-tauri/src/platform/launcher.rs` |
| File-protection labels | unit tests | `src-tauri/src/platform/fileprot.rs` |
| File-open detection | pure unit tests over synthetic access times | `src-tauri/src/platform/fileguard.rs` |
| Frontend logic | vitest in jsdom | `tests/matcher.test.ts` |

What is **not** covered by automated tests, and why: the Win32 calls themselves.
They require a real desktop session. Each one is written so that all of its
decisions live in a pure function next to it (for example `applock::decide`,
`automation::parse_combo`), and those pure functions are what the tests exercise.
The Win32 call is then a thin, single-purpose wrapper.

## 2. Running the tests

```bash
# Rust core — works on any platform
cd src-tauri && cargo test

# Frontend
npm test
```

## 3. Verification record

This is the state of the suite as built.

### 3.1 Rust core

```
running 107 tests
test result: ok. 107 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

The suite covers, among others:

- **Crypto** — hash round-trip; hashes are salted (the same secret hashed twice
  gives different output); wrong secret rejected.
- **Auth** — passphrase must be at least three words; verification is
  case- and punctuation-insensitive; PIN length and digit rules; pattern needs
  four dots; rate limiting refuses even the correct secret once tripped; a
  successful attempt resets the failure counter; **a voice match without an
  enrolled voiceprint fails closed**; a voice match with a matching template
  passes and a very different vector fails.
- **Matcher** — normalisation strips case and punctuation; near phrases match
  within threshold; unrelated text does not match; acoustic scoring is monotonic.
- **VAD** — opens on sustained speech, closes after a quiet run, resets to idle.
- **Router** — the lock screen accepts only unlock-class matches; commands are
  ignored without a wake word when one is required; the wake window expires.
- **App lock** — an unprotected process is always allowed; a locked, unauthorised
  process is blocked; a live authorisation window allows it and an expired one
  does not; name normalisation strips `.exe` and case.
- **Automation** — key combos parse (modifiers + key); invalid combos are
  rejected; points and scroll directions parse; `Ctrl+Alt+Del` is refused.
- **Service** — the crash marker correctly detects an unclean previous exit.

### 3.2 Frontend

```
Test Files  1 passed (1)
     Tests  14 passed (14)
```

- Phrase normalisation parity with the Rust side.
- Fuzzy matching: exact, near-miss with a typo, and rejection of unrelated text.
- DOM helpers: attributes and children, **text is never interpreted as HTML**
  (an injection attempt stays inert), mount replaces content, null/false
  children are skipped.
- Formatting: byte sizes and relative times.

### 3.3 Build verification

```
npm run build          → vite build, 25 modules, dist/ produced
npx tsc --noEmit       → 0 errors
cargo test             → 107 passed, 0 failed
```

## 4. Manual verification checklist

Automated tests cannot press `Win+D`. These are the checks to run by hand on a
Windows machine, in the order they should be performed. Each is a thing that can
actually be observed.

### Voice engine

- [ ] First launch: the wizard appears; skipping every step leaves the app
      usable and Settings still reachable.
- [ ] Enrol a voice in Voice → Enrol voice. The identity card shows a sample
      count and a threshold.
- [ ] Say the wake phrase: the status line shows the phrase it heard.
- [ ] Unplug the microphone: the engine reports stopped, logs a recovery, and
      resumes within a few seconds of the device returning.
- [ ] Restart the app: the engine starts again on its own.

### Protected applications and files

- [ ] Protect an application in **Application Lock**. Start it from Explorer (not
      from BLACKVAULT): a BLACKVAULT window opens over it asking for the PIN, and
      the application closes within the grace period if the window is ignored.
- [ ] Enter the PIN in that window: the window plays its release animation, closes,
      and the application starts again.
- [ ] Protect a file in **Files & Folders**, then open it from Explorer: the
      prompt window appears with the file's name, and the file opens once the
      password is accepted.
- [ ] Press **Open** on a protected item in the dashboard: the same prompt window
      appears.
- [ ] Set *Seal strength* to `hard`: opening the item now shows Windows' own
      "access denied" instead of the prompt. This is the documented trade-off.
- [ ] With the control centre minimised to the tray, start a protected
      application: the prompt window still appears.

### Lock and unlock

- [ ] "lock this PC": every monitor goes black; the taskbar is gone; a running
      download keeps progressing (verify by watching the file's size).
- [ ] `Ctrl+Alt+Del` still reaches the Windows secure desktop.
- [ ] "unlock this PC": the challenge appears; the correct passphrase plus the
      configured second factor unlocks; a wrong passphrase does not.
- [ ] Five wrong passphrases: the sixth attempt is refused with a cool-down
      message, and the audit log shows the rate-limit event.
- [ ] The unlock ceremony plays on success.

### Launcher

- [ ] Add an app by picking an `.exe`; give it a voice phrase; launch it by
      voice.
- [ ] Say a phrase for an app that is not on the list: it is refused and logged.

### App lock

- [ ] Protect Notepad. Launch it: it is terminated and an event is logged.
- [ ] Authenticate, then launch it: it runs, and re-locks after it exits.

### File protection

- [ ] Protect a folder. Try to open it: access is denied.
- [ ] Release it from the app: access returns and the permissions are the ones
      it had before.
- [ ] Lock the desktop: the item is sealed again.

### Automation

- [ ] "next" advances a playing video.
- [ ] "scroll down" scrolls the focused window.
- [ ] "click" clicks where the pointer is.
- [ ] "press enter" sends Enter to the focused window.
- [ ] "volume up" changes the system volume.

### Persistence

- [ ] Change a setting, quit, relaunch: the setting survives.
- [ ] Kill the process from Task Manager, relaunch: the app logs that it
      recovered from an unclean shutdown.

## 5. Honest limitations found during verification

These are real, and are documented rather than hidden:

- **App lock is a watchdog, not a kernel filter.** A protected process exists
  for up to ~1.2 s before it is terminated. It is designed to stop casual
  opening, not to be an unbreakable sandbox.
- **Elevated windows reject synthetic input.** Windows UIPI means BLACKVAULT
  cannot type into an application running at a higher integrity level. This is
  correct operating-system behaviour and is not worked around.
- **EFS needs NTFS.** On a non-NTFS volume the applied mechanism reports `acl`
  rather than `acl+efs`, and the item is protected by permissions alone.
- **Windows Hello needs a device.** On a machine with no biometric reader and no
  Hello PIN, the factor is reported unavailable and the other factors carry the
  unlock.
- **Protecting a file you do not own needs elevation.** Without it, the
  protection call fails and the failure is reported; nothing is silently
  half-applied.
