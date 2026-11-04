# BLACKVAULT

**YOUR VOICE. YOUR CONTROL. YOUR SECURITY.**

BLACKVAULT is a native Windows desktop security application: an always-running,
voice-controlled personal security system. It locks and unlocks the desktop by
voice, hides the desktop behind a full-screen privacy overlay, protects
applications, files and folders with real Windows access control, and drives the
desktop hands-free — next video, scroll, click, press enter, volume, media
transport and more.

Everything runs locally. There is no cloud dependency, no subscription and no
telemetry.

---

## What it actually does

| Area | Behaviour |
|---|---|
| **Lock by voice** | Say *"lock this PC"*. A borderless, top-most, full-screen black overlay covers the desktop on every monitor. Background processes, downloads and AI agents keep running. |
| **Unlock by voice** | Say *"unlock this PC"*, then speak your secret passphrase. The recognised words must hash-match the stored passphrase, and the configured second factor (PIN, pattern or Windows Hello) still has to follow. Optionally enrol a voiceprint so the voice itself must match as well. |
| **Voice launcher** | *"open chrome"*, *"open visual studio code"* — only apps on your approved list can ever be launched by voice. |
| **App lock** | Protect any `.exe`. A watchdog terminates it while locked, and releases it only after you authenticate. |
| **File & folder protection** | NTFS deny-ACL plus EFS content encryption. Never hide, never rename. |
| **Automation** | *"next"*, *"previous"*, *"scroll down"*, *"click"*, *"press enter"*, *"volume up"*, *"mute"*, *"show desktop"*, *"take a screenshot"*, and arbitrary key combos. All through real `SendInput`. |
| **Word recognition** | Offline, on-device, no cloud and no model download: Windows SAPI with a grammar built from your wake phrase, unlock phrase and every command. Type a command and it works immediately — there is no training step. |
| **Factors** | Secret passphrase, numeric PIN, 3×3 pattern, Windows Hello (fingerprint/face/PIN). All local, all salted-hashed, all rate limited. |

## What it deliberately does **not** do

- It does not replace or weaken the Windows secure session lock. `Ctrl+Alt+Del`
  still reaches the real secure desktop. The black mode is an application-level
  privacy overlay, not a security boundary.
- It does not store your voice recordings. Audio is analysed in memory and
  discarded; the `storeAudio` setting is forced off.
- It does not treat speech-to-text alone as authentication. A voice match is one
  signal among several, and it can never be the only one.
- It does not hide or rename protected files. A protection you cannot audit is
  not a protection.

## Requirements

- Windows 10 (build 1809+) or Windows 11, 64-bit
- A microphone
- .NET/WebView2 runtime (preinstalled on Windows 11 and current Windows 10)
- For spoken commands: a Windows speech recogniser. Most installs have one; if
  not, add the *Speech* language feature under Settings → Time & Language →
  Language → English → Language options. Without it BLACKVAULT says so in the
  dashboard instead of pretending voice control works, and every other feature
  (PIN, pattern, passphrase, app lock, file protection) is unaffected.
- Administrator rights **only** for protecting files and folders you do not own

## Install

1. Download `BLACKVAULT_1.0.0_x64_en-US.msi` (or the NSIS `.exe`) from Releases.
2. Run it. The installer places BLACKVAULT in Program Files and offers to start
   it at sign-in.
3. On first launch the **Security Setup Wizard** runs. It is skippable at every
   step. Set what you want — a passphrase, a PIN, a pattern, Windows Hello, a
   voice identity — and skip what you do not.
4. Say *"black vault"* (the default wake phrase), then *"lock this PC"*.

## Build from source

See [`docs/INSTALL.md`](docs/INSTALL.md) for the full toolchain setup.

```bash
# Frontend
npm install
npm run build

# Desktop application (Windows)
npm run tauri:build
# → src-tauri/target/release/bundle/msi/BLACKVAULT_1.0.0_x64_en-US.msi
# → src-tauri/target/release/bundle/nsis/BLACKVAULT_1.0.0_x64-setup.exe
```

The portable build is the single executable at
`src-tauri/target/release/blackvault.exe`. It runs in place with no installer.

## Documentation

| Document | Contents |
|---|---|
| [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) | Module map, data flow, threading, security boundaries |
| [`docs/SECURITY.md`](docs/SECURITY.md) | Threat model, what is protected and what is not, crypto choices |
| [`docs/COMMANDS.md`](docs/COMMANDS.md) | Every voice command and every automation action |
| [`docs/INSTALL.md`](docs/INSTALL.md) | Toolchain, build, packaging, code signing |
| [`docs/TESTING.md`](docs/TESTING.md) | Test strategy and how to reproduce the results |
| [`docs/CONFIGURATION.md`](docs/CONFIGURATION.md) | Every setting, where it is stored, what it changes |

## Project layout

```
src/                      TypeScript frontend (no framework)
  lib/                    IPC bridge, types, DOM helpers, icons, store
  ui/                     shell, lock screen, setup wizard
  panes/                  one module per screen
  styles/                 the design system
src-tauri/                Rust core
  src/
    lib.rs                app wiring and the command list
    commands.rs           the Tauri command surface
    engine.rs             background threads: voice routing, app guard, tray
    voice/                capture, VAD, MFCC/DTW matcher, router
    platform/             Windows integration (lock, hello, applock, fileprot,
                          automation, launcher, service, speech)
    auth.rs               the authentication policy
    crypto.rs             PBKDF2 hashing and DPAPI sealing
    db.rs                 SQLite persistence
    dsp.rs                feature extraction (pure, unit-tested)
docs/                     architecture, security, testing, configuration
tests/                    frontend unit tests (vitest)
```

## Status

Every feature listed above is implemented in this repository. Where a capability
depends on the host — Windows Hello on a machine without a biometric reader, EFS
on a non-NTFS volume, administrator rights for a file you do not own — the
application reports the limitation rather than pretending. See
[`docs/TESTING.md`](docs/TESTING.md) for the verification record.

## Licence

MIT. See `LICENSE`.
