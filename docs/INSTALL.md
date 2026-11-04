# BLACKVAULT — Installation and Build Guide

## 1. Installing the application

### Option A — MSI installer (recommended)

1. Download `BLACKVAULT_1.0.0_x64_en-US.msi` from the Releases page.
2. Double-click it. If SmartScreen warns about an unknown publisher, choose
   **More info → Run anyway**. The build is not code-signed unless you supplied
   your own certificate (see §4).
3. Follow the wizard. The installer offers to launch BLACKVAULT at sign-in.
4. Launch it. The **Security Setup Wizard** runs on first start.

### Option B — NSIS installer

`BLACKVAULT_1.0.0_x64-setup.exe` does the same job with a smaller download and a
per-user install option.

### Option C — Portable

`blackvault.exe` runs in place with no installer and no registry writes beyond
the auto-start entry, which is off by default. Copy it anywhere and run it. Its
data still lives in `%LOCALAPPDATA%\BLACKVAULT`, so two portable copies share
one configuration; delete that folder to start clean.

### After installing

The first-run wizard walks through, in order:

1. **Welcome** — what the app does, and the one thing it will not do (replace
   the Windows secure session lock).
2. **Secret passphrase** — the primary voice unlock. At least three words.
3. **Voice identity** — a few samples of your own voice. Skippable.
4. **PIN** — optional numeric factor. Skippable.
5. **Windows Hello** — offered only if the device supports it. Skippable.
6. **Start with Windows** — the auto-start preference.

Every step is skippable. You can leave with only a passphrase set, or with
nothing set at all and configure everything later from Settings.

## 2. Building from source

### 2.1 Prerequisites

| Tool | Version | Notes |
|---|---|---|
| Windows | 10 1809+ / 11 | Windows 11 is what the UI is designed against |
| Node.js | 20 LTS or newer | 22 LTS recommended |
| Rust | stable (1.77+) | `rustup` default host `x86_64-pc-windows-msvc` |
| MSVC build tools | VS 2022, "Desktop development with C++" | Provides `link.exe` |
| WebView2 runtime | Evergreen | Preinstalled on Windows 11 |
| WiX Toolset | 3.14 | Only needed to produce the `.msi` |

### 2.2 Steps

```powershell
git clone https://github.com/your-org/blackvault.git
cd blackvault

npm install

# Frontend only
npm run build

# Full desktop build (dev)
npm run tauri:dev

# Full desktop build (release bundles)
npm run tauri:build
```

Outputs:

```
src-tauri\target\release\blackvault.exe                              portable
src-tauri\target\release\bundle\msi\BLACKVAULT_1.0.0_x64_en-US.msi   installer
src-tauri\target\release\bundle\nsis\BLACKVAULT_1.0.0_x64-setup.exe  installer
```

### 2.3 Building on Linux or macOS

The frontend builds anywhere. The Rust core compiles on non-Windows hosts with
the platform modules stubbed, which is enough to run the unit tests — useful in
CI, where a full Windows build is expensive:

```bash
# Linux
sudo apt-get install -y libwebkit2gtk-4.1-dev libgtk-3-dev libasound2-dev \
                        librsvg2-dev libssl-dev pkg-config
cargo test --manifest-path src-tauri/Cargo.toml --lib

# Frontend tests
npm test
```

The stubs report `Unsupported` for anything that needs Win32, so the pure logic
— the DSP, the matcher, the router, the authentication policy, the database and
the crypto — is fully exercised without a Windows machine.

## 3. Verifying a build

```powershell
# Version and signature of the built exe
Get-Item .\src-tauri\target\release\blackvault.exe | Select-Object VersionInfo

# SHA-256 of the artifacts
Get-FileHash .\src-tauri\target\release\bundle\msi\*.msi -Algorithm SHA256

# Confirm the app is not running as admin unless you asked it to
Get-Process blackvault | Select-Object Name, Id
```

A correctly built release:

- starts with no console window,
- shows the splash, then the dashboard,
- reports its version in **Settings → About**,
- writes `%LOCALAPPDATA%\BLACKVAULT\blackvault.db` on first run.

## 4. Code signing

Unsigned builds trigger SmartScreen. To sign:

```powershell
$cert = "C:\certs\your-cert.pfx"
$pw   = "your-password"

# Tauri signs the bundles automatically when these are set.
$env:TAURI_SIGNING_PRIVATE_KEY = $pw
npm run tauri:build
```

For an EV or standard Authenticode certificate, sign the `.exe` and the two
installers after the build:

```powershell
signtool sign /fd SHA256 /f $cert /p $pw `
  .\src-tauri\target\release\blackvault.exe `
  .\src-tauri\target\release\bundle\msi\*.msi `
  .\src-tauri\target\release\bundle\nsis\*.exe
```

Never commit a certificate or its password. `.gitignore` already excludes
`*.pfx`, `*.pem`, `*.jks` and `*.key`.

## 5. Uninstalling

Uninstall from **Settings → Apps**, or run the MSI again and choose *Remove*.

What the uninstaller removes: the program files, the Start Menu entry, and the
auto-start registry value.

What it leaves: `%LOCALAPPDATA%\BLACKVAULT` — your database, your audit log and
your settings. That is deliberate, because it also holds the record of which
files you protected and the ACL snapshots needed to release them cleanly.

Before uninstalling, open **Settings → Danger zone → Factory reset**. That
releases every protected item and wipes the rules. Then delete
`%LOCALAPPDATA%\BLACKVAULT` by hand if you want the machine spotless.

## 6. Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| SmartScreen blocks the installer | Build is unsigned | *More info → Run anyway*, or sign the build |
| "Windows Hello unavailable" | No biometric reader and no Hello PIN configured | Set a Windows Hello PIN in Windows Settings, or use the passphrase/PIN factor |
| Voice engine shows *stopped* | No input device, or the microphone is in exclusive use | Settings → Voice engine → Restart; check Windows privacy settings allow desktop apps to use the mic |
| No sound is detected | Wrong default input device | Set the correct device as default in Windows, then restart the engine |
| Protection fails on a file | You do not own it and are not elevated | Run BLACKVAULT as administrator, or choose a location you own |
| The black screen will not lift | The unlock factor is not configured | `Ctrl+Alt+Del` still works — sign out and back in, then set a factor |
| A protected app still opens | It started between watchdog ticks, or it runs elevated | Wait for the tick; elevated processes cannot be terminated without elevation |
| Database is corrupt | Power loss during a write | Delete `blackvault.db` (and `-wal`, `-shm`); the app recreates it and re-seeds defaults |
