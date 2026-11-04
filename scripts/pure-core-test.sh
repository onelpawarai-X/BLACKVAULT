#!/usr/bin/env bash
# Run BLACKVAULT's platform-independent core tests on any host.
#
# The core (crypto, database, DSP, matching, routing, auth policy and the
# decision logic of every platform module) is written so it compiles without
# Windows. That makes the security-critical parts testable on a Linux or macOS
# machine and in CI, where a Windows SDK is not available.
#
# This script assembles those modules into a throwaway crate and runs their
# unit tests. It is the same set of tests `cargo test` runs on Windows, minus
# the Windows-only code paths.
#
# Usage:  scripts/pure-core-test.sh
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
src="$root/src-tauri/src"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

mkdir -p "$work/src/voice" "$work/src/platform"

cat > "$work/Cargo.toml" <<'TOML'
[package]
name = "blackvault-core"
version = "0.0.0"
edition = "2021"

[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
rusqlite = { version = "0.32", features = ["bundled"] }
chrono = { version = "0.4", features = ["serde"] }
thiserror = "2"
base64 = "0.22"
rand = "0.8"
sha2 = "0.10"
hmac = "0.12"
pbkdf2 = { version = "0.12", features = ["simple"] }
subtle = "2"
parking_lot = "0.12"
log = "0.4"
zeroize = { version = "1", features = ["derive"] }
rustfft = "6"
num-complex = "0.4"
ureq = "2"
TOML

cat > "$work/src/lib.rs" <<'RS'
mod auth;
mod crypto;
mod db;
mod dsp;
mod error;
mod models;
mod platform;
mod settings;
mod voice;
RS

cat > "$work/src/voice/mod.rs" <<'RS'
pub mod matcher;
pub mod router;
pub mod vad;
RS

cat > "$work/src/platform/mod.rs" <<'RS'
pub mod applock;
pub mod automation;
pub mod fileprot;
pub mod hello;
pub mod launcher;
pub mod lock;
pub mod service;
pub mod speech;
pub mod stt;
RS

# Modules shared verbatim with the real crate.
for f in auth.rs crypto.rs db.rs dsp.rs error.rs models.rs settings.rs \
         voice/matcher.rs voice/router.rs voice/vad.rs \
         platform/applock.rs platform/automation.rs platform/fileprot.rs \
         platform/hello.rs platform/launcher.rs platform/lock.rs platform/service.rs \
         platform/speech.rs platform/stt.rs; do
  cp "$src/$f" "$work/src/$f"
done

cd "$work"
exec cargo test "$@"
