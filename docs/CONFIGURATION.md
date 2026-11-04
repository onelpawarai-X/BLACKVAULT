# BLACKVAULT — Configuration

Everything is editable in the app under **Settings**. Nothing requires editing a
file by hand. This document exists so the values are auditable and so a
supported configuration can be reproduced.

## Where state lives

```
%LOCALAPPDATA%\BLACKVAULT\
  blackvault.db          SQLite: settings, secret hashes, rules, audit log
  blackvault.db-wal      WAL (transient)
  running.lock           present only while the app is running
  logs\
    blackvault-audit-*.json   audit exports
```

Nothing is written anywhere else. There is no machine-wide state, no service,
and no registry key beyond the single `HKCU\...\Run` value when auto-start is on.

## Settings reference

| Setting | Default | Range | Effect |
|---|---|---|---|
| `alwaysOnVoice` | `true` | — | Start the listener at launch and keep it running |
| `wakePhrase` | `"black vault"` | 1–6 words | The phrase that arms a command |
| `wakePhraseEnabled` | `true` | — | When off, commands run without a wake word |
| `requireWakeForCommands` | `true` | — | Require the wake phrase before every command |
| `listenWhileLocked` | `true` | — | Keep listening in privacy mode (needed for voice unlock) |
| `voiceprintThreshold` | `0.62` | 0.30–0.95 | Cosine similarity needed for a voice match |
| `requireSecondFactor` | `true` | — | A voice match alone never completes an unlock |
| `authRateLimitAttempts` | `5` | 1–20 | Failures allowed before the cool-down |
| `authRateLimitWindowSecs` | `120` | 10–3600 | Length of the cool-down window |
| `autoRelockSecs` | `0` | 0–86400 | Lock automatically after this idle period (0 = off) |
| `multiMonitorOverlay` | `true` | — | Cover every monitor, not just the primary |
| `hideCursorWhenLocked` | `true` | — | Hide the pointer in privacy mode |
| `blockTaskSwitchWhenLocked` | `false` | — | Swallow Win / Alt+Tab / Alt+Esc while locked |
| `offlineRecognition` | `false` | — | Use the Windows offline recogniser as well as the endpoint |
| `appGuardEnabled` | `true` | — | Watch for protected applications starting |
| `appLockGraceSecs` | `12` | 5–300 | Seconds an unauthorised application may run while its prompt is up |
| `fileGuardEnabled` | `true` | — | Raise the prompt when a protected item is opened |
| `fileLockMode` | `"prompt"` | `prompt` / `hard` | `prompt` denies write and delete so the item still opens; `hard` denies read too |
| `storeAudio` | `false` | forced `false` | Never enabled; audio is never persisted |
| `startWithWindows` | `false` | — | Write the per-user Run entry |
| `theme` | `"black"` | — | The only theme; kept for forward compatibility |
| `auditLogRetentionDays` | `90` | 1–3650 | Events older than this are pruned at launch |
| `hotkeyLock` | `null` | a combo | Global shortcut that locks without voice |
| `hotkeyPanic` | `null` | a combo | Global shortcut that locks and stops the listener |

Two settings are enforced rather than merely defaulted:

- `storeAudio` is forced to `false` in `Settings::sanitize`. There is no code
  path that writes audio to disk, so the setting is a statement of intent, not a
  toggle.
- `voiceprintThreshold` is clamped to a sane band so a mistyped value cannot make
  voice matching trivially permissive.

## Secrets

Secrets are set in the app, never in a file:

| Secret | Where set | Rules |
|---|---|---|
| Passphrase | Settings → Authentication → Secret passphrase | At least three words |
| PIN | Settings → Authentication → PIN | 4–12 digits |
| Pattern | Settings → Authentication → Pattern | At least four dots |
| Voice identity | Voice → Enrol voice | Several samples of the same phrase |
| Windows Hello | Settings → Authentication → Verify with Windows Hello | Whatever the device supports |

Each is stored as `pbkdf2$210000$<salt>$<hash>`. Clearing a factor removes its
row; the unlock screen then stops offering it.

## Voice commands

Commands live in the `voice_commands` table and are managed in **Voice**. Each
row is a phrase, an action type, an optional payload, an enabled flag and a
"requires authentication" flag. The editor writes through the same code path the
seed uses, so a hand-made command and a shipped one are indistinguishable.

Default commands are seeded once, on first run, and are ordinary rows — editing
or deleting them is permanent.

## Launcher entries

Launcher entries live in `launcher_apps`: a display name, the executable path,
optional arguments and working directory, an optional voice phrase, an enabled
flag. `Scan Installed Apps` reads the Start Menu and the common install roots and
offers what it finds; nothing is added without you choosing it.

## Protected applications

`protected_apps` rows are created by **Applications → Protect an application**.
Each row stores the executable path, the derived process name, and the three
policy flags: `require_auth`, `auto_relock`, `relock_after_secs`. The watchdog in
`engine.rs` reads this table directly, so a change takes effect on the next tick
without a restart.

## Protected items

`protected_items` rows record the path, whether it is a file or a folder, the
applied mechanism label, and the ACL snapshot taken at protection time (in
SDDL). The snapshot is what makes the release exact: unprotecting restores the
permissions the item had, rather than guessing at a default.

## Audit log

Every security-relevant action writes an `events` row: lock, unlock, auth
success, auth failure, rate limiting, engine start/stop/recovery, app blocked,
file protected/unprotected/released, setting changes, voice commands run.

- **Events** pane shows the most recent 200 by default.
- **Export** writes the full log as JSON to `logs\`.
- Retention is applied at launch using `auditLogRetentionDays`.

## Resetting

**Settings → Danger zone → Factory reset** releases every protected item (so
nothing is left sealed with a rule that no longer exists), then wipes the rules,
the secrets and the settings. It does not delete your files and does not touch
the audit log — a reset should be auditable too.
