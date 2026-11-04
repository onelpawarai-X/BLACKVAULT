# BLACKVAULT — Security Model

This document states what BLACKVAULT protects, how, and — just as importantly —
what it does not protect against. A security tool that overstates itself is worse
than no tool at all, so the limits are written down here rather than discovered
later.

## 1. Threat model

**In scope.** A person who picks up an unlocked, running machine; a person who
can see the screen but not the account; an application that would run and read
protected content without the owner's consent; an attacker who has copied the
database file off the machine.

**Out of scope.** Malware running as the same user with the same privileges; a
kernel-level attacker; someone who has the account password and can log in
normally; physical attacks such as DMA or a cold-boot read of RAM; anyone who
can modify the BLACKVAULT binary itself.

BLACKVAULT is a personal security layer. It is not a sandbox, not an EDR, and not
a substitute for full-disk encryption or a proper account password.

## 2. What each protection actually is

### 2.1 Full-Screen Privacy Mode

An application-level overlay. The BLACKVAULT window becomes borderless,
top-most, taskbar-skipping and full-screen, and one passive overlay window is
created for every secondary monitor. Background processes keep running.

**It is not a Windows secure desktop.** The real secure desktop — the one you
reach with `Ctrl+Alt+Del` — is a separate, protected desktop owned by the
operating system. BLACKVAULT does not touch it, does not call `LockWorkStation`
as a side effect, and cannot be used to spoof it. `Ctrl+Alt+Del` still works
exactly as Windows intends, which is the correct behaviour: a tool that could
intercept it would be a security hole, not a feature.

Optional hardening (Settings → Lock screen → *Block task switching while
locked*) installs a low-level keyboard hook that swallows the Windows key,
Alt+Tab and Alt+Esc while the overlay is up. This is a convenience, not a
boundary: a determined user can still reach the secure desktop.

### 2.2 Unlock authentication

An unlock can complete only when **all** applicable conditions hold:

1. **Words.** The recognised phrase, after normalisation (lower-case,
   punctuation and spacing removed), must verify against the stored
   PBKDF2-HMAC-SHA256 hash of the passphrase. This is checked first.
2. **Voice.** The utterance's speaker vector must be within the configured
   cosine similarity of the enrolled template. A missing enrolment fails closed,
   not open.
3. **Second factor.** When `requireSecondFactor` is on (default), a successful
   voice step moves the session to *passphrase cleared* and the unlock screen
   asks for the PIN, the pattern or Windows Hello. Only that factor completes
   the unlock.

Rate limiting: after `authRateLimitAttempts` failures inside
`authRateLimitWindowSecs`, further attempts are refused entirely for the rest of
the window — including attempts with the correct secret. Every attempt, success
or failure, is written to the audit log.

### 2.3 Secret storage

| Secret | Stored as | Where |
|---|---|---|
| Passphrase | `pbkdf2$210000$<salt>$<hash>` over the normalised phrase | `secrets` table |
| PIN | same scheme over the digits | `secrets` table |
| Pattern | same scheme over `"0-1-2-5-8"` | `secrets` table |
| Voice template | MFCC speaker vector, JSON, sealed with DPAPI | `voice_identity` |
| Data key | DPAPI-sealed under the current user | `secrets` table |

There is no column in the schema that can hold a plaintext credential, and the
`crypto` module has no function that writes one. Comparison is constant-time
(`subtle::ConstantTimeEq`).

**Honest limit.** A 4-digit PIN has only 10 000 possibilities. If someone
extracts the database file, PBKDF2 makes an offline search slow but not
impossible. The PIN is a convenience factor; the passphrase is the strong one.
Use the passphrase for anything that matters, and rely on Windows full-disk
encryption (BitLocker) to protect the database file itself.

### 2.4 File and folder protection

Two mechanisms, both honoured by Windows itself:

- **NTFS DACL.** The item's current security descriptor is read, converted to
  SDDL and kept as the *open* state. Sealing replaces the DACL with one that
  denies the interactive user **and `Authenticated Users`** the rights to read,
  write, execute and delete, while granting SYSTEM and Administrators. The file
  cannot be opened, listed or executed while sealed.

  Denying `Authenticated Users` as well as the user's own SID is load-bearing,
  not belt-and-braces. A deny ACE only overrides an allow ACE *for the same
  trustee*, so on an account that belongs to Administrators the old
  `(A;;GA;;;BA)` allow matched the token and the sealed file opened anyway —
  that was a real bug. `AU` is in every interactive token, administrator
  included, so denying it closes the hole.

  The rights denied are `0x10067` (read/list, write, append, execute, delete
  child, delete) and deliberately **not** `GA`. Denying `GA` would deny
  `WRITE_DAC` too, and the owner's right to rewrite the DACL is the only thing
  that lets BLACKVAULT release the item again.
- **EFS.** `EncryptFileW` marks the file encrypted at rest, so the content is
  unreadable without the user's EFS key even if the DACL is bypassed offline.

Nothing is hidden or renamed. The denial is visible in the file's own Security
tab, which is the point: an auditable protection.

**Honest limits.** EFS is unavailable on non-NTFS volumes and on Home editions
configured without it; when that happens the applied label reports `acl` rather
than `acl+efs`. An Administrator can always take ownership of a file — that is
what Administrator means. Protecting files you do not own needs elevation.

Both protections are re-checked. The supervisor re-applies the DACL every few
seconds if a sealed item's deny ACE has been edited away, so a protection that
was removed by hand does not stay removed.

Releasing an item is itself an authenticated action, and it happens in the
prompt window rather than in the control centre. `reveal_path` raises that
window; the PIN, the password or the secret passphrase is verified against the
same PBKDF2 hash the lock screen uses, and the item re-seals on the next lock.
Windows Hello is offered as an extra button when the machine supports it, never
as the only way through — a machine without Hello can still release its own
files.

`fileLockMode` decides what "sealed" means:

- `prompt` (default) denies write, append and delete, but leaves read allowed.
  The item therefore still opens — Word opens the document, Explorer opens the
  folder — and the prompt window appears on top asking for the password. This is
  the mode where the user sees the feature working.
- `hard` denies read as well, so the content cannot be opened at all and Windows
  reports its own "access denied". Stronger, but with no card and no way to
  release from the file itself.

Reading a protected item is noticed by watching its last-access time. A sealed
item cannot be written to, so its modification time would never change — the
access time is the signal that something opened it. On a volume where Windows
has been configured not to update access times (`fsutil behavior set
DisableLastAccess 1`) that signal is absent, and the **Open** button in
**Files & Folders** is the guaranteed way to raise the prompt.

### 2.5 Application lock

A watchdog enumerates processes every 0.4 s. When a process matches a *locked*,
*require-auth* rule with no live authorisation, BLACKVAULT raises an
authentication prompt and gives the process `appLockGraceSecs` (12 by default)
before terminating it. Killing a protected application the instant it appeared —
with no prompt — was a real bug: the application flashed up, vanished, and
nothing ever asked for a password. Authorisations are time-bounded (5 minutes)
and granted only after a successful authentication or a launch through
BLACKVAULT.

The prompt is its own window, not a card inside the control centre, because the
control centre is in the tray while the user works. Accepting the password
releases the process name and, if the unauthorised copy was already terminated,
starts the application again from the path recorded in the rule.

**Honest limit.** This is a race, not a wall: between a process starting and the
next watchdog tick, it exists for up to 1.2 s. It is designed to stop a
protected app from being casually opened, not to be an unbreakable sandbox.

### 2.6 Voice privacy

Audio is captured, analysed in memory, and discarded. There is no code path that
writes a recording to disk. The `storeAudio` setting exists for schema
compatibility and is forced to `false` in `Settings::sanitize`, so it cannot be
switched on from the UI or by editing the database. The only thing derived from
your voice that persists is the speaker template — a 26-number feature vector,
not audio, and it is DPAPI-sealed.

### 2.7 IPC and the frontend

The WebView is locked down in `tauri.conf.json`:

- `script-src 'self'` — no remote script, no `eval`.
- `connect-src 'self' ipc: http://ipc.localhost` — the frontend can talk to the
  core and nothing else.
- `freezePrototype: true` — the standard Tauri prototype-pollution guard.
- A capability file lists the exact permissions the main window has; there is no
  blanket filesystem or shell permission.

The frontend builds all DOM through `document.createElement` and
`textContent`. User-supplied strings are never assigned to `innerHTML`, so a
crafted command phrase or file name cannot inject markup. There is a unit test
for exactly this.

## 3. Cryptographic choices

| Purpose | Algorithm | Parameters | Rationale |
|---|---|---|---|
| Secret hashing | PBKDF2-HMAC-SHA256 | 210 000 iterations, 16-byte random salt per secret | Simple, available, well understood; a fresh salt per secret defeats rainbow tables |
| Integrity digest | HMAC-SHA256 | random per-install key | Keyed, so it cannot be forged without the key |
| Reversible sealing | DPAPI `CryptProtectData` | current-user scope, `CRYPTPROTECT_UI_FORBIDDEN` | The key is derived from the user's logon credentials and never leaves the OS |
| Unlock hand-off | 32 random bytes, base64url | one-shot, cleared on use | Stops a stale UI from completing an unlock it did not earn |

On non-Windows hosts (development, CI) DPAPI is replaced by an in-process XOR
keyed with a random per-run value. That fallback is clearly marked, reports
`dpapi_available() == false`, and is never used to protect real secrets on a
Windows build.

## 4. What an attacker with the database file gets

- The PBKDF2 hashes. A weak PIN can be brute-forced offline with effort; a
  multi-word passphrase will not be.
- The voice template, sealed with DPAPI — useless off the machine, because the
  key is bound to the user's logon.
- The audit log and the protection rules: the file names you protected, when,
  and which commands you use. This is metadata, not content.

It does **not** get your files' contents (EFS), your passphrase, your PIN, or
your pattern.

## 5. Reporting a problem

If you find a flaw in any of the above, the useful report says which claim in
this document is wrong and how you demonstrated it. Claims here are meant to be
checkable: every one maps to a function in the source, and most have a test.
