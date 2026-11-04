#!/usr/bin/env bash
# Type-check BLACKVAULT's Windows-only modules without a Windows SDK.
#
# `cargo check --target x86_64-pc-windows-msvc` needs no linker, but the bundled
# SQLite in the real crate is compiled by `cc`, which does need MSVC's `lib.exe`.
# This script side-steps that: it builds a throwaway crate holding only the
# `platform/*` modules (which never touch SQLite) plus small stand-ins for the
# two types they borrow from the crate, and type-checks it for the Windows
# target. The `windows` crate itself is not gated on `cfg(windows)`, so the real
# Win32 bindings are checked exactly as the release build will see them.
#
# Requires: rustup target add x86_64-pc-windows-msvc
#
# Usage:  scripts/win-check.sh
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
src="$root/src-tauri/src"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

mkdir -p "$work/src/platform"
cd "$work"

# Feature list is kept in step with src-tauri/Cargo.toml.
cat > Cargo.toml <<'TOML'
[package]
name = "blackvault-wincheck"
version = "0.0.0"
edition = "2021"

[dependencies]
windows = { version = "0.58", features = [
  "Win32_Foundation",
  "Win32_System_Threading",
  "Win32_System_JobObjects",
  "Win32_System_LibraryLoader",
  "Win32_System_SystemInformation",
  "Win32_System_Registry",
  "Win32_System_Memory",
  "Win32_Security",
  "Win32_Security_Authorization",
  "Win32_Security_Credentials",
  "Win32_Security_Cryptography",
  "Win32_Storage_FileSystem",
  "Win32_System_Com",
  "Win32_System_Ole",
  "Win32_System_ProcessStatus",
  "Win32_System_Diagnostics_ToolHelp",
  "Win32_UI_Input_KeyboardAndMouse",
  "Win32_UI_Input",
  "Win32_UI_WindowsAndMessaging",
  "Win32_UI_Shell",
  "Win32_Graphics_Gdi",
  "Win32_Graphics_Dwm",
  "Win32_Media_Audio",
  "Win32_Media",
  "Win32_Media_Speech",
  "Win32_System_Com_StructuredStorage",
  "Win32_System_Variant",
  "Foundation",
  "Foundation_Collections",
  "Security_Credentials_UI",
  "Win32_Devices_Display",
] }
chrono = { version = "0.4", features = ["serde"] }
log = "0.4"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
base64 = "0.22"
parking_lot = "0.12"
rand = "0.8"
sha2 = "0.10"
hmac = "0.12"
subtle = "2"
zeroize = { version = "1", features = ["derive"] }
pbkdf2 = { version = "0.12", default-features = false, features = ["simple", "std"] }
TOML

cat > src/lib.rs <<'RS'
pub mod crypto;
pub mod error;
pub mod models;
pub mod platform;
RS

# Stand-ins for the two crate modules the platform layer borrows from. The real
# ones pull in SQLite, which is what this script exists to avoid.
cat > src/error.rs <<'RS'
#[derive(Debug)]
pub enum BvError {
    Crypto,
    Unsupported(String),
    Msg(String),
}
impl BvError {
    pub fn msg(m: impl Into<String>) -> Self {
        BvError::Msg(m.into())
    }
}
impl std::fmt::Display for BvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl From<std::io::Error> for BvError {
    fn from(e: std::io::Error) -> Self {
        BvError::Msg(e.to_string())
    }
}
pub type BvResult<T> = Result<T, BvError>;
RS

cat > src/models.rs <<'RS'
#[derive(Debug, Clone, Default)]
pub struct DeviceScanResult {
    pub exe_path: String,
    pub name: String,
    pub publisher: Option<String>,
    pub version: Option<String>,
}
RS

cat > src/platform/mod.rs <<'RS'
pub mod applock;
pub mod automation;
pub mod fileprot;
pub mod hello;
pub mod launcher;
pub mod lock;
pub mod service;
pub mod speech;
RS

for f in applock automation fileprot hello launcher lock service speech; do
  cp "$src/platform/$f.rs" "src/platform/$f.rs"
done
cp "$src/crypto.rs" "src/crypto.rs"

exec cargo check --target x86_64-pc-windows-msvc --message-format short "$@"
