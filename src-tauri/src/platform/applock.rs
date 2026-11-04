//! BLACKVAULT — application lock enforcement.
//!
//! Enforcement is a watchdog over the process table, not a hook or a shim:
//!
//!   * every protected rule names a process image (e.g. `chrome.exe`);
//!   * a watcher thread enumerates processes on a short interval;
//!   * a process that matches a *locked* rule and has no live authorisation is
//!     terminated;
//!   * authorisation is granted for a bounded window after the user passes an
//!     authentication check or launches the app through BLACKVAULT;
//!   * when the process exits, the rule re-locks after its delay.
//!
//! A rule with `require_auth` off is treated as "allowed to run, but recorded",
//! which is the honest behaviour: a rule that does nothing would be worse than
//! no rule at all.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// The decision the watcher makes for one running process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardDecision {
    /// Not protected — leave it alone.
    Allow,
    /// Protected, locked, and not authorised — terminate it.
    Block,
    /// Protected but currently authorised — leave it alone.
    Authorized,
}

/// Pure decision function, so the policy is testable without a process table.
pub fn decide(
    protected: bool,
    locked: bool,
    require_auth: bool,
    authorized_until: Option<Instant>,
    now: Instant,
) -> GuardDecision {
    if !protected {
        return GuardDecision::Allow;
    }
    if !require_auth {
        return GuardDecision::Authorized;
    }
    if !locked {
        return GuardDecision::Authorized;
    }
    match authorized_until {
        Some(t) if now < t => GuardDecision::Authorized,
        _ => GuardDecision::Block,
    }
}

/// A live authorisation grant: this process name may run until `until`.
#[derive(Debug, Default)]
pub struct Authorizations {
    grants: HashMap<String, Instant>,
}

impl Authorizations {
    pub fn new() -> Self {
        Self::default()
    }

    /// Grant a window for a process image name (case-insensitive).
    pub fn grant(&mut self, process_name: &str, window: Duration) {
        self.grants
            .insert(normalise_name(process_name), Instant::now() + window);
    }

    pub fn until(&self, process_name: &str) -> Option<Instant> {
        self.grants.get(&normalise_name(process_name)).copied()
    }

    pub fn revoke(&mut self, process_name: &str) {
        self.grants.remove(&normalise_name(process_name));
    }

    pub fn clear(&mut self) {
        self.grants.clear();
    }

    /// Drop grants that have expired so the map cannot grow without bound.
    pub fn prune(&mut self) {
        let now = Instant::now();
        self.grants.retain(|_, t| *t > now);
    }
}

pub fn normalise_name(name: &str) -> String {
    let lower = name.trim().to_ascii_lowercase();
    lower.strip_suffix(".exe").unwrap_or(&lower).to_string()
}

/// Snapshot of one running process.
#[derive(Debug, Clone)]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
}

#[cfg(windows)]
pub fn list_processes() -> Vec<ProcessInfo> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };

    let mut out = Vec::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return out;
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snap, &mut entry).is_ok() {
            loop {
                let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
                let name = String::from_utf16_lossy(&entry.szExeFile[..len]);
                out.push(ProcessInfo {
                    pid: entry.th32ProcessID,
                    name,
                });
                if Process32NextW(snap, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    out
}

#[cfg(not(windows))]
pub fn list_processes() -> Vec<ProcessInfo> {
    Vec::new()
}

/// Terminate one process by id. Returns true when the call succeeded.
#[cfg(windows)]
pub fn terminate_pid(pid: u32) -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};

    unsafe {
        match OpenProcess(PROCESS_TERMINATE, false, pid) {
            Ok(handle) => {
                let ok = TerminateProcess(handle, 1).is_ok();
                let _ = CloseHandle(handle);
                ok
            }
            Err(_) => false,
        }
    }
}

#[cfg(not(windows))]
pub fn terminate_pid(_pid: u32) -> bool {
    false
}

/// Put a freshly-launched process into a job object that kills it when
/// BLACKVAULT exits, so a protected app cannot outlive its guard.
#[cfg(windows)]
pub fn adopt_into_job(pid: u32) -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, SetInformationJobObject,
        JobObjectExtendedLimitInformation, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE};

    unsafe {
        let Ok(job) = CreateJobObjectW(None, None) else {
            return false;
        };
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let _ = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const core::ffi::c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        let Ok(proc) = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, false, pid) else {
            let _ = CloseHandle(job);
            return false;
        };
        let ok = AssignProcessToJobObject(job, proc).is_ok();
        let _ = CloseHandle(proc);
        // The job handle is intentionally leaked: it must outlive the call so
        // the kill-on-close guarantee holds for the app's whole lifetime.
        ok
    }
}

#[cfg(not(windows))]
pub fn adopt_into_job(_pid: u32) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unprotected_is_always_allowed() {
        let now = Instant::now();
        assert_eq!(decide(false, true, true, None, now), GuardDecision::Allow);
    }

    #[test]
    fn locked_and_unauthorised_is_blocked() {
        let now = Instant::now();
        assert_eq!(decide(true, true, true, None, now), GuardDecision::Block);
    }

    #[test]
    fn authorisation_window_allows() {
        let now = Instant::now();
        let until = now + Duration::from_secs(30);
        assert_eq!(decide(true, true, true, Some(until), now), GuardDecision::Authorized);
        assert_eq!(decide(true, true, true, Some(until), until + Duration::from_secs(1)), GuardDecision::Block);
    }

    #[test]
    fn require_auth_off_is_allowed() {
        let now = Instant::now();
        assert_eq!(decide(true, true, false, None, now), GuardDecision::Authorized);
    }

    #[test]
    fn unlocked_rule_is_allowed() {
        let now = Instant::now();
        assert_eq!(decide(true, false, true, None, now), GuardDecision::Authorized);
    }

    #[test]
    fn authorisations_normalise_names() {
        let mut a = Authorizations::new();
        a.grant("Chrome.EXE", Duration::from_secs(10));
        assert!(a.until("chrome").is_some());
        assert!(a.until("CHROME.EXE").is_some());
        a.revoke("chrome.exe");
        assert!(a.until("chrome").is_none());
    }

    #[test]
    fn authorisations_prune_expired() {
        let mut a = Authorizations::new();
        a.grant("app", Duration::from_millis(1));
        std::thread::sleep(Duration::from_millis(5));
        a.prune();
        assert!(a.until("app").is_none());
    }

    #[test]
    fn normalise_strips_exe_and_case() {
        assert_eq!(normalise_name("Notepad.EXE"), "notepad");
        assert_eq!(normalise_name(" chrome.exe "), "chrome");
        assert_eq!(normalise_name("code"), "code");
    }
}
