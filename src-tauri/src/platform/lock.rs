//! BLACKVAULT — Full-Screen Privacy Mode.
//!
//! This is deliberately *not* a replacement for the Windows secure desktop.
//! It is an application-level privacy overlay: the BLACKVAULT window is made
//! borderless, top-most and full-screen on every monitor, and — critically —
//! the desktop is hidden by covering it rather than by touching any Windows
//! security mechanism. Running processes, downloads and AI agents are untouched.
//!
//! We never call `LockWorkStation` here, and we never try to defeat the Windows
//! security model. `Ctrl+Alt+Del` still reaches the real secure desktop, exactly
//! as it should.

use crate::error::BvResult;

/// Description of one monitor, in virtual-desktop coordinates.
#[derive(Debug, Clone, Copy)]
pub struct MonitorRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub primary: bool,
}

#[cfg(windows)]
pub fn monitors() -> BvResult<Vec<MonitorRect>> {
    use windows::Win32::Foundation::{BOOL, LPARAM, RECT, TRUE};
    use windows::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
    };

    unsafe extern "system" fn cb(
        monitor: HMONITOR,
        _hdc: HDC,
        _clip: *mut RECT,
        data: LPARAM,
    ) -> BOOL {
        let list = &mut *(data.0 as *mut Vec<MonitorRect>);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(monitor, &mut info).as_bool() {
            list.push(MonitorRect {
                x: info.rcMonitor.left,
                y: info.rcMonitor.top,
                width: (info.rcMonitor.right - info.rcMonitor.left) as u32,
                height: (info.rcMonitor.bottom - info.rcMonitor.top) as u32,
                primary: info.dwFlags & 1 != 0,
            });
        }
        TRUE
    }

    let mut list: Vec<MonitorRect> = Vec::new();
    unsafe {
        let ok = EnumDisplayMonitors(None, None, Some(cb), LPARAM(&mut list as *mut _ as isize));
        if ok.as_bool() && !list.is_empty() {
            Ok(list)
        } else {
            Err(crate::error::BvError::msg("could not enumerate monitors"))
        }
    }
}

#[cfg(not(windows))]
pub fn monitors() -> BvResult<Vec<MonitorRect>> {
    // One synthetic monitor so the overlay logic can be exercised in tests.
    Ok(vec![MonitorRect {
        x: 0,
        y: 0,
        width: 1920,
        height: 1080,
        primary: true,
    }])
}

/// Hide the system cursor for the duration of the lock. The cursor is restored
/// by `show_cursor` (or by the window losing the overlay).
#[cfg(windows)]
pub fn hide_cursor() {
    use windows::Win32::UI::WindowsAndMessaging::ShowCursor;
    unsafe {
        // ShowCursor is a counter; drive it well below zero.
        for _ in 0..16 {
            ShowCursor(false);
        }
    }
}

#[cfg(windows)]
pub fn show_cursor() {
    use windows::Win32::UI::WindowsAndMessaging::ShowCursor;
    unsafe {
        for _ in 0..16 {
            ShowCursor(true);
        }
    }
}

#[cfg(not(windows))]
pub fn hide_cursor() {}

#[cfg(not(windows))]
pub fn show_cursor() {}

/// Position and size a window to cover a monitor exactly.
#[cfg(windows)]
pub fn cover_monitor(hwnd: isize, rect: MonitorRect) -> BvResult<()> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, HWND_TOPMOST, SWP_NOACTIVATE, SWP_SHOWWINDOW,
    };
    unsafe {
        SetWindowPos(
            HWND(hwnd as *mut _),
            HWND_TOPMOST,
            rect.x,
            rect.y,
            rect.width as i32,
            rect.height as i32,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        )
        .map_err(|e| crate::error::BvError::msg(format!("SetWindowPos failed: {e}")))?;
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn cover_monitor(_hwnd: isize, _rect: MonitorRect) -> BvResult<()> {
    Err(crate::error::BvError::Unsupported("cover_monitor".into()))
}

/// When enabled, install a low-level keyboard hook that swallows the Windows
/// key and Alt+Tab while the overlay is up, so the desktop cannot be peeked at.
/// This is an *addition* to the overlay, never a substitute for Windows' own
/// secure session lock.
#[cfg(windows)]
pub fn set_task_switch_blocked(blocked: bool) {
    use parking_lot::Mutex;
    use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        VK_ESCAPE, VK_LWIN, VK_RWIN, VK_TAB,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, SetWindowsHookExW, UnhookWindowsHookEx, KBDLLHOOKSTRUCT, WH_KEYBOARD_LL,
        WM_KEYDOWN, WM_SYSKEYDOWN,
    };

    static HOOK: Mutex<Option<isize>> = Mutex::new(None);

    unsafe extern "system" fn proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code >= 0 {
            let msg = wparam.0 as u32;
            if msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN {
                let kb = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
                let vk = kb.vkCode;
                let win_key = vk == VK_LWIN.0 as u32 || vk == VK_RWIN.0 as u32;
                let alt_tab = vk == VK_TAB.0 as u32;
                let alt_esc = vk == VK_ESCAPE.0 as u32;
                if win_key || alt_tab || alt_esc {
                    return LRESULT(1); // swallow
                }
            }
        }
        CallNextHookEx(None, code, wparam, lparam)
    }

    let mut guard = HOOK.lock();
    unsafe {
        if blocked && guard.is_none() {
            if let Ok(h) = SetWindowsHookExW(WH_KEYBOARD_LL, Some(proc), None, 0) {
                *guard = Some(h.0 as isize);
            }
        } else if !blocked {
            if let Some(h) = guard.take() {
                let _ = UnhookWindowsHookEx(windows::Win32::UI::WindowsAndMessaging::HHOOK(h as *mut _));
            }
        }
    }
}

#[cfg(not(windows))]
pub fn set_task_switch_blocked(_blocked: bool) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monitor_list_is_never_empty() {
        // On non-Windows this returns the synthetic monitor; on Windows a real
        // machine always has at least one.
        let m = monitors().unwrap();
        assert!(!m.is_empty());
    }
}
