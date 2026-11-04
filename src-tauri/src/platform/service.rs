//! BLACKVAULT — background service behaviour.
//!
//! "Service" here means the always-running engine that lives in the tray and
//! keeps the voice listener and the app-lock watchdog alive — not a Windows SCM
//! service (which would demand elevation and a machine-wide footprint this app
//! does not need, and which the user's own security policy may forbid).
//!
//! Responsibilities:
//!   * register/unregister the per-user startup entry (HKCU Run key);
//!   * enforce single-instance so two copies cannot fight over the database;
//!   * a watchdog that restarts the voice engine if it stops unexpectedly;
//!   * a crash marker so the next launch can log that it recovered.

use std::path::PathBuf;

use crate::error::{BvError, BvResult};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "BLACKVAULT";

/// Register or remove the per-user startup entry. Per-user (HKCU) is deliberate:
/// it needs no elevation and is removed cleanly when the user uninstalls.
#[cfg(windows)]
pub fn set_autostart(enabled: bool) -> BvResult<()> {
    use windows::core::PCWSTR;
    use windows::Win32::System::Registry::{
        RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
        KEY_SET_VALUE, REG_SZ,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    unsafe {
        let mut key = HKEY::default();
        let wkey = wide(RUN_KEY);
        let rc = RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(wkey.as_ptr()), 0, KEY_SET_VALUE, &mut key);
        if rc.is_err() {
            return Err(BvError::msg(format!("could not open Run key ({})", rc.0)));
        }
        let wname = wide(RUN_VALUE);
        let result = if enabled {
            let exe = std::env::current_exe()
                .map_err(|e| BvError::msg(format!("could not resolve exe: {e}")))?;
            let value = format!("\"{}\" --tray", exe.to_string_lossy());
            let wval = wide(&value);
            let bytes = std::slice::from_raw_parts(wval.as_ptr() as *const u8, wval.len() * 2);
            let rc = RegSetValueExW(key, PCWSTR(wname.as_ptr()), 0, REG_SZ, Some(bytes));
            if rc.is_err() {
                Err(BvError::msg(format!("could not write Run value ({})", rc.0)))
            } else {
                Ok(())
            }
        } else {
            let _ = RegDeleteValueW(key, PCWSTR(wname.as_ptr()));
            Ok(())
        };
        let _ = RegCloseKey(key);
        result
    }
}

#[cfg(not(windows))]
pub fn set_autostart(_enabled: bool) -> BvResult<()> {
    Err(BvError::Unsupported("startup registration".into()))
}

#[cfg(windows)]
pub fn autostart_enabled() -> bool {
    use windows::core::PCWSTR;
    use windows::Win32::System::Registry::{RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ};
    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }
    unsafe {
        let mut key = HKEY::default();
        let wkey = wide(RUN_KEY);
        if RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(wkey.as_ptr()), 0, KEY_READ, &mut key).is_err() {
            return false;
        }
        let wname = wide(RUN_VALUE);
        let found = RegQueryValueExW(key, PCWSTR(wname.as_ptr()), None, None, None, None).is_ok();
        let _ = RegCloseKey(key);
        found
    }
}

#[cfg(not(windows))]
pub fn autostart_enabled() -> bool {
    false
}

// ------------------------------------------------------------ single instance

/// Hold a named mutex for the lifetime of the process. Returns `None` when
/// another instance already owns it.
#[cfg(windows)]
pub struct InstanceGuard {
    handle: isize,
}

#[cfg(windows)]
impl InstanceGuard {
    pub fn acquire() -> Option<Self> {
        use windows::core::PCWSTR;
        use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
        use windows::Win32::System::Threading::CreateMutexW;

        let name = "Global\\BLACKVAULT-SingleInstance";
        let wname: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            match CreateMutexW(None, true, PCWSTR(wname.as_ptr())) {
                Ok(h) => {
                    if GetLastError() == ERROR_ALREADY_EXISTS {
                        None
                    } else {
                        Some(InstanceGuard { handle: h.0 as isize })
                    }
                }
                Err(_) => None,
            }
        }
    }
}

#[cfg(windows)]
impl Drop for InstanceGuard {
    fn drop(&mut self) {
        use windows::Win32::Foundation::CloseHandle;
        unsafe {
            let _ = CloseHandle(windows::Win32::Foundation::HANDLE(self.handle as *mut _));
        }
    }
}

#[cfg(not(windows))]
pub struct InstanceGuard;

#[cfg(not(windows))]
impl InstanceGuard {
    pub fn acquire() -> Option<Self> {
        Some(InstanceGuard)
    }
}

// --------------------------------------------------------------- crash marker

/// Written while the app is running, removed on a clean exit. If it is present
/// at launch, the previous run did not shut down cleanly.
pub fn crash_marker_path(dir: &std::path::Path) -> PathBuf {
    dir.join("running.lock")
}

pub fn mark_running(dir: &std::path::Path) -> BvResult<bool> {
    std::fs::create_dir_all(dir)?;
    let path = crash_marker_path(dir);
    let unclean = path.exists();
    std::fs::write(&path, chrono::Utc::now().to_rfc3339())?;
    Ok(unclean)
}

pub fn mark_clean(dir: &std::path::Path) {
    let _ = std::fs::remove_file(crash_marker_path(dir));
}

// ------------------------------------------------------------------ watchdog

/// Restart the voice engine if it stops while it should be running.
pub struct Watchdog {
    pub interval: std::time::Duration,
    pub max_restarts: u32,
}

impl Default for Watchdog {
    fn default() -> Self {
        Watchdog {
            interval: std::time::Duration::from_secs(5),
            max_restarts: 50,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crash_marker_detects_unclean_exit() {
        let dir = std::env::temp_dir().join("blackvault-test-marker");
        let _ = std::fs::remove_file(crash_marker_path(&dir));
        assert!(!mark_running(&dir).unwrap(), "first run is clean");
        assert!(mark_running(&dir).unwrap(), "second run sees the stale marker");
        mark_clean(&dir);
        assert!(!mark_running(&dir).unwrap());
        mark_clean(&dir);
    }
}
