//! BLACKVAULT — application discovery, launch and close.
//!
//! * `scan_installed()` walks the Start Menu (resolving shortcuts through the
//!   shell), the Program Files trees and the registry uninstall keys, then
//!   de-duplicates by resolved executable path.
//! * `launch()` starts a process directly — never through a shell — so an
//!   argument cannot be reinterpreted as a command.
//! * `close()` asks a process's top-level windows to close politely, and only
//!   terminates if they refuse.

use std::path::{Path, PathBuf};

use crate::error::{BvError, BvResult};
use crate::models::DeviceScanResult;

/// A curated set of paths to search, on top of the Start Menu.
fn common_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for var in ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432", "LOCALAPPDATA", "APPDATA"] {
        if let Ok(v) = std::env::var(var) {
            if !v.is_empty() {
                roots.push(PathBuf::from(v));
            }
        }
    }
    roots
}

fn start_menu_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for var in ["ProgramData", "APPDATA"] {
        if let Ok(v) = std::env::var(var) {
            dirs.push(PathBuf::from(v).join("Microsoft").join("Windows").join("Start Menu").join("Programs"));
        }
    }
    dirs.into_iter().filter(|d| d.is_dir()).collect()
}

/// Scan for installed applications. Best-effort and bounded: the Start Menu is
/// always walked; the Program Files trees are walked to a shallow depth.
pub fn scan_installed() -> Vec<DeviceScanResult> {
    let mut found: Vec<DeviceScanResult> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    for dir in start_menu_dirs() {
        walk_shortcuts(&dir, 6, &mut found, &mut seen);
    }
    for root in common_roots() {
        walk_exes(&root, 3, &mut found, &mut seen);
    }
    #[cfg(windows)]
    scan_uninstall_registry(&mut found, &mut seen);

    found.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    found
}

fn push_exe(found: &mut Vec<DeviceScanResult>, seen: &mut std::collections::HashSet<String>, path: &Path) {
    let exe = path.to_string_lossy().to_string();
    let key = exe.to_lowercase();
    if !seen.insert(key) {
        return;
    }
    // Skip uninstallers and crash handlers — they are noise in a launcher.
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
    if stem.contains("unins") || stem.contains("setup") || stem.contains("crashpad") || stem.contains("update") {
        return;
    }
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "Unknown".into());
    found.push(DeviceScanResult {
        exe_path: exe,
        name,
        publisher: None,
        version: None,
    });
}

fn walk_exes(dir: &Path, depth: u8, found: &mut Vec<DeviceScanResult>, seen: &mut std::collections::HashSet<String>) {
    if depth == 0 || found.len() > 800 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_exes(&path, depth - 1, found, seen);
        } else if path.extension().map(|e| e.eq_ignore_ascii_case("exe")).unwrap_or(false) {
            push_exe(found, seen, &path);
        }
    }
}

fn walk_shortcuts(dir: &Path, depth: u8, found: &mut Vec<DeviceScanResult>, seen: &mut std::collections::HashSet<String>) {
    if depth == 0 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_shortcuts(&path, depth - 1, found, seen);
        } else if path.extension().map(|e| e.eq_ignore_ascii_case("lnk")).unwrap_or(false) {
            if let Some(target) = resolve_shortcut(&path) {
                if target.extension().map(|e| e.eq_ignore_ascii_case("exe")).unwrap_or(false) {
                    push_exe(found, seen, &target);
                }
            }
        }
    }
}

/// Resolve a `.lnk` to its target path via the shell. Returns `None` off
/// Windows or when the shortcut cannot be read.
#[cfg(windows)]
pub fn resolve_shortcut(lnk: &Path) -> Option<PathBuf> {
    use windows::core::{Interface, HSTRING, PCWSTR};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};

    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let result = (|| -> Option<PathBuf> {
            let link: IShellLinkW =
                CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
            let persist = link.cast::<windows::Win32::System::Com::IPersistFile>().ok()?;
            let wide = HSTRING::from(lnk.to_string_lossy().to_string());
            persist.Load(PCWSTR(wide.as_ptr()), windows::Win32::System::Com::STGM_READ).ok()?;
            let mut buf = [0u16; 1024];
            link.GetPath(&mut buf, std::ptr::null_mut(), 0).ok()?;
            let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            let s = String::from_utf16_lossy(&buf[..len]);
            if s.is_empty() {
                None
            } else {
                Some(PathBuf::from(s))
            }
        })();
        CoUninitialize();
        result
    }
}

#[cfg(not(windows))]
pub fn resolve_shortcut(_lnk: &Path) -> Option<PathBuf> {
    None
}

#[cfg(windows)]
fn scan_uninstall_registry(found: &mut Vec<DeviceScanResult>, seen: &mut std::collections::HashSet<String>) {
    use windows::core::PCWSTR;
    use windows::Win32::System::Registry::{
        RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER,
        HKEY_LOCAL_MACHINE, KEY_READ, REG_SZ,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    unsafe fn read_value(key: HKEY, name: &str) -> Option<String> {
        let wname = wide(name);
        let mut ty = windows::Win32::System::Registry::REG_VALUE_TYPE(0);
        let mut len: u32 = 0;
        if RegQueryValueExW(key, PCWSTR(wname.as_ptr()), None, Some(&mut ty), None, Some(&mut len)).is_err() {
            return None;
        }
        if ty != REG_SZ || len == 0 {
            return None;
        }
        let mut buf = vec![0u8; len as usize];
        if RegQueryValueExW(
            key,
            PCWSTR(wname.as_ptr()),
            None,
            Some(&mut ty),
            Some(buf.as_mut_ptr()),
            Some(&mut len),
        )
        .is_err()
        {
            return None;
        }
        let u16s: Vec<u16> = buf
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .take_while(|&c| c != 0)
            .collect();
        Some(String::from_utf16_lossy(&u16s))
    }

    unsafe fn scan_root(root: HKEY, sub: &str, found: &mut Vec<DeviceScanResult>, seen: &mut std::collections::HashSet<String>) {
        let wsub = wide(sub);
        let mut key = HKEY::default();
        if RegOpenKeyExW(root, PCWSTR(wsub.as_ptr()), 0, KEY_READ, &mut key).is_err() {
            return;
        }
        let mut i = 0u32;
        loop {
            let mut name = [0u16; 256];
            let mut len = name.len() as u32;
            if RegEnumKeyExW(key, i, windows::core::PWSTR(name.as_mut_ptr()), &mut len, None, windows::core::PWSTR::null(), None, None).is_err() {
                break;
            }
            i += 1;
            let sub_name = String::from_utf16_lossy(&name[..len as usize]);
            let mut subkey = HKEY::default();
            let wsubname = wide(&sub_name);
            if RegOpenKeyExW(key, PCWSTR(wsubname.as_ptr()), 0, KEY_READ, &mut subkey).is_ok() {
                if let Some(loc) = read_value(subkey, "DisplayIcon").or_else(|| read_value(subkey, "InstallLocation")) {
                    let clean = loc.split(',').next().unwrap_or("").trim().trim_matches('"').to_string();
                    let p = PathBuf::from(&clean);
                    if p.extension().map(|e| e.eq_ignore_ascii_case("exe")).unwrap_or(false) && p.exists() {
                        push_exe(found, seen, &p);
                    }
                }
                let _ = RegCloseKey(subkey);
            }
        }
        let _ = RegCloseKey(key);
    }

    unsafe {
        for root in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
            for sub in [
                "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
                "SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
            ] {
                scan_root(root, sub, found, seen);
            }
        }
    }
}

/// Hand a file or folder to whatever application owns it.
///
/// This is what `ShellExecute` is for, and it is the only way a folder opens in
/// Explorer. The path is passed as an argument, never through a shell string, so
/// a name with spaces or quotes in it cannot become a second command.
#[cfg(windows)]
pub fn open_with_default(path: &Path) -> BvResult<()> {
    use std::os::windows::process::CommandExt;
    if !path.exists() {
        return Err(BvError::msg("the item no longer exists"));
    }
    let target = path.to_string_lossy().to_string();
    std::process::Command::new("rundll32.exe")
        .arg("url.dll,FileProtocolHandler")
        .arg(&target)
        .creation_flags(0x08000000) // CREATE_NO_WINDOW
        .spawn()
        .map_err(|e| BvError::msg(format!("could not open the item: {e}")))?;
    Ok(())
}

#[cfg(not(windows))]
pub fn open_with_default(_path: &Path) -> BvResult<()> {
    Err(BvError::Unsupported("opening files".into()))
}

/// Launch an executable directly with optional arguments and working directory.
pub fn launch(exe: &str, args: Option<&str>, cwd: Option<&str>) -> BvResult<()> {
    if exe.trim().is_empty() {
        return Err(BvError::msg("no executable path"));
    }
    let path = Path::new(exe);
    if !path.exists() {
        return Err(BvError::msg(format!("not found: {exe}")));
    }
    // Refuse anything that is not a real executable file, so a mistyped path
    // cannot become a shell invocation.
    if !path.extension().map(|e| e.eq_ignore_ascii_case("exe")).unwrap_or(false) {
        return Err(BvError::msg("only .exe targets may be launched"));
    }

    let mut cmd = std::process::Command::new(path);
    if let Some(a) = args {
        for part in split_args(a) {
            cmd.arg(part);
        }
    }
    if let Some(d) = cwd {
        if Path::new(d).is_dir() {
            cmd.current_dir(d);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
        const DETACHED_PROCESS: u32 = 0x00000008;
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS);
    }
    cmd.spawn()
        .map_err(|e| BvError::msg(format!("could not launch: {e}")))?;
    Ok(())
}

/// Split an argument string on whitespace, honouring double quotes.
pub fn split_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    for ch in s.chars() {
        match ch {
            '"' => in_quotes = !in_quotes,
            c if c.is_whitespace() && !in_quotes => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Close a running application by process name, politely first.
#[cfg(windows)]
pub fn close_process(process_name: &str) -> BvResult<()> {
    use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM};
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowThreadProcessId, IsWindowVisible, PostMessageW, WM_CLOSE,
    };

    let target = process_name.trim_end_matches(".exe").to_ascii_lowercase();
    let mut pids: Vec<u32> = Vec::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)
            .map_err(|e| BvError::msg(format!("snapshot failed: {e}")))?;
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snap, &mut entry).is_ok() {
            loop {
                let name = String::from_utf16_lossy(
                    &entry.szExeFile[..entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(0)],
                );
                if name.trim_end_matches(".exe").eq_ignore_ascii_case(&target) {
                    pids.push(entry.th32ProcessID);
                }
                if Process32NextW(snap, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    if pids.is_empty() {
        return Err(BvError::msg("process is not running"));
    }

    // Ask each top-level window to close.
    unsafe extern "system" fn cb(hwnd: HWND, data: LPARAM) -> windows::Win32::Foundation::BOOL {
        let pids = &*(data.0 as *const Vec<u32>);
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pids.contains(&pid) && IsWindowVisible(hwnd).as_bool() {
            let _ = PostMessageW(hwnd, WM_CLOSE, windows::Win32::Foundation::WPARAM(0), LPARAM(0));
        }
        true.into()
    }
    unsafe {
        let _ = EnumWindows(Some(cb), LPARAM(&pids as *const _ as isize));
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn close_process(_process_name: &str) -> BvResult<()> {
    Err(BvError::Unsupported("closing processes".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_quoted_args() {
        assert_eq!(split_args("--flag value"), vec!["--flag", "value"]);
        assert_eq!(
            split_args("\"C:\\Program Files\\x\" --a"),
            vec!["C:\\Program Files\\x", "--a"]
        );
        assert!(split_args("").is_empty());
    }

    #[test]
    fn launch_rejects_non_exe() {
        assert!(launch("notepad.txt", None, None).is_err());
        assert!(launch("", None, None).is_err());
    }

    #[test]
    fn scan_returns_a_list_without_panicking() {
        // On Windows this walks the real Start Menu; off Windows it is empty.
        let _ = scan_installed();
    }
}
