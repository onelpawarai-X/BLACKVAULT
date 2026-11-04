//! BLACKVAULT — desktop automation.
//!
//! All input is synthesised through `SendInput`, which is what a real keyboard
//! or mouse would produce. That means it works with media players, browsers and
//! any focused window, and it respects UIPI: BLACKVAULT cannot type into a
//! window running at a higher integrity level than itself, and it does not try.
//!
//! Supported: key combos, mouse move/click, wheel scrolling, the media transport
//! keys, the system volume keys, show-desktop / minimise-all, and a screenshot
//! of the primary monitor.

use crate::error::{BvError, BvResult};

/// Parse `"ctrl+shift+esc"` or `"win+d"` into a key sequence.
/// Returns the modifier virtual-key codes and the final key code.
pub fn parse_combo(combo: &str) -> BvResult<(Vec<u16>, u16)> {
    let mut mods = Vec::new();
    let mut key: Option<u16> = None;
    for part in combo.split('+').map(|p| p.trim().to_ascii_lowercase()) {
        if part.is_empty() {
            continue;
        }
        match part.as_str() {
            "ctrl" | "control" => mods.push(0x11),
            "shift" => mods.push(0x10),
            "alt" => mods.push(0x12),
            "win" | "super" | "meta" => mods.push(0x5B),
            other => {
                if key.is_some() {
                    return Err(BvError::msg(format!("more than one key in \"{combo}\"")));
                }
                key = Some(vk_for(other)?);
            }
        }
    }
    match key {
        Some(k) => Ok((mods, k)),
        None => Err(BvError::msg(format!("no key in \"{combo}\""))),
    }
}

/// Map a friendly key name to a Windows virtual-key code.
pub fn vk_for(name: &str) -> BvResult<u16> {
    let n = name.trim().to_ascii_lowercase();
    // Single character: letter or digit.
    if n.chars().count() == 1 {
        let c = n.chars().next().unwrap().to_ascii_uppercase();
        if c.is_ascii_alphanumeric() {
            return Ok(c as u16);
        }
    }
    let vk = match n.as_str() {
        "enter" | "return" => 0x0D,
        "esc" | "escape" => 0x1B,
        "space" | "spacebar" => 0x20,
        "tab" => 0x09,
        "backspace" => 0x08,
        "delete" | "del" => 0x2E,
        "insert" | "ins" => 0x2D,
        "home" => 0x24,
        "end" => 0x23,
        "pageup" | "pgup" => 0x21,
        "pagedown" | "pgdn" => 0x22,
        "left" => 0x25,
        "up" => 0x26,
        "right" => 0x27,
        "down" => 0x28,
        "f1" => 0x70, "f2" => 0x71, "f3" => 0x72, "f4" => 0x73,
        "f5" => 0x74, "f6" => 0x75, "f7" => 0x76, "f8" => 0x77,
        "f9" => 0x78, "f10" => 0x79, "f11" => 0x7A, "f12" => 0x7B,
        "media_next" | "next_track" => 0xB0,
        "media_prev" | "prev_track" => 0xB1,
        "media_stop" => 0xB2,
        "media_play_pause" | "play_pause" => 0xB3,
        "volume_mute" => 0xAD,
        "volume_down" => 0xAE,
        "volume_up" => 0xAF,
        "semicolon" => 0xBA,
        "equals" => 0xBB,
        "comma" => 0xBC,
        "minus" => 0xBD,
        "period" => 0xBE,
        "slash" => 0xBF,
        "backtick" => 0xC0,
        "lbracket" => 0xDB,
        "backslash" => 0xDC,
        "rbracket" => 0xDD,
        "quote" => 0xDE,
        _ => return Err(BvError::msg(format!("unknown key \"{name}\""))),
    };
    Ok(vk)
}

#[cfg(windows)]
mod imp {
    use super::*;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS,
        KEYEVENTF_KEYUP, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN,
        MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL,
        MOUSEINPUT, MOUSEEVENTF_MOVE, MOUSEEVENTF_ABSOLUTE, VIRTUAL_KEY,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN,
    };

    fn key_input(vk: u16, up: bool) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(vk),
                    wScan: 0,
                    dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }

    fn mouse_input(flags: windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS, dx: i32, dy: i32, data: i32) -> INPUT {
        INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx,
                    dy,
                    mouseData: data as u32,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }

    fn send(inputs: &[INPUT]) -> BvResult<()> {
        let sent = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
        if sent as usize == inputs.len() {
            Ok(())
        } else {
            Err(BvError::msg("SendInput was blocked"))
        }
    }

    pub fn press_keys(combo: &str) -> BvResult<()> {
        let (mods, key) = parse_combo(combo)?;
        let mut inputs = Vec::new();
        for m in &mods {
            inputs.push(key_input(*m, false));
        }
        inputs.push(key_input(key, false));
        inputs.push(key_input(key, true));
        for m in mods.iter().rev() {
            inputs.push(key_input(*m, true));
        }
        send(&inputs)
    }

    pub fn tap_key(vk: u16) -> BvResult<()> {
        send(&[key_input(vk, false), key_input(vk, true)])
    }

    pub fn click(x: Option<i32>, y: Option<i32>, button: &str) -> BvResult<()> {
        if let (Some(x), Some(y)) = (x, y) {
            let (sw, sh) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
            if sw > 0 && sh > 0 {
                // Absolute coordinates are normalised to 0..65535.
                let nx = (x as i64 * 65535 / sw as i64) as i32;
                let ny = (y as i64 * 65535 / sh as i64) as i32;
                send(&[mouse_input(MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE, nx, ny, 0)])?;
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }
        let (down, up) = match button {
            "right" => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
            "middle" => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP),
            _ => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
        };
        send(&[mouse_input(down, 0, 0, 0)])?;
        std::thread::sleep(std::time::Duration::from_millis(30));
        send(&[mouse_input(up, 0, 0, 0)])
    }

    /// Two clicks in quick succession, the way a file manager expects.
    pub fn double_click(x: Option<i32>, y: Option<i32>) -> BvResult<()> {
        click(x, y, "left")?;
        std::thread::sleep(std::time::Duration::from_millis(60));
        click(None, None, "left")
    }

    /// Jump to the top or the bottom of whatever is in front.
    ///
    /// There is no "scroll to the end" input event, so the page keys are used
    /// the way a person would: Ctrl+Home and Ctrl+End for documents, and a long
    /// run of wheel notches as the fallback a video or a web page responds to.
    pub fn scroll_to(top: bool) -> BvResult<()> {
        let combo = if top { "ctrl+home" } else { "ctrl+end" };
        let _ = press_keys(combo);
        let notches = if top { 60 } else { -60 };
        scroll(0, notches * 60)
    }

    pub fn scroll(dx: i32, dy: i32) -> BvResult<()> {
        // One wheel notch is 120; `dy` is given in pixels, so scale it.
        let notches = (dy / 60).clamp(-20, 20) * 120;
        let mut inputs = Vec::new();
        if notches != 0 {
            inputs.push(mouse_input(MOUSEEVENTF_WHEEL, 0, 0, notches));
        }
        if dx != 0 {
            const MOUSEEVENTF_HWHEEL: windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS =
                windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS(0x01000);
            let hnotches = (dx / 60).clamp(-20, 20) * 120;
            inputs.push(mouse_input(MOUSEEVENTF_HWHEEL, 0, 0, hnotches));
        }
        if inputs.is_empty() {
            return Ok(());
        }
        send(&inputs)
    }

    pub fn show_desktop() -> BvResult<()> {
        press_keys("win+d")
    }

    pub fn minimize_all() -> BvResult<()> {
        press_keys("win+m")
    }

    pub fn screenshot() -> BvResult<String> {
        use windows::Win32::Graphics::Gdi::{
            BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC,
            GetDIBits, ReleaseDC, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, SRCCOPY,
        };
        let (w, h) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
        if w <= 0 || h <= 0 {
            return Err(BvError::msg("could not read screen size"));
        }
        let dir = dirs_pictures();
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!(
            "BLACKVAULT-{}.bmp",
            chrono::Local::now().format("%Y%m%d-%H%M%S")
        ));

        unsafe {
            let screen = GetDC(windows::Win32::Foundation::HWND::default());
            let mem = CreateCompatibleDC(screen);
            let bmp = CreateCompatibleBitmap(screen, w, h);
            let old = windows::Win32::Graphics::Gdi::SelectObject(mem, bmp);
            BitBlt(mem, 0, 0, w, h, screen, 0, 0, SRCCOPY)
                .map_err(|e| BvError::msg(format!("BitBlt failed: {e}")))?;

            let mut info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h, // top-down
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: 0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let stride = w as usize * 4;
            let mut pixels = vec![0u8; stride * h as usize];
            let lines = GetDIBits(
                mem,
                bmp,
                0,
                h as u32,
                Some(pixels.as_mut_ptr() as *mut _),
                &mut info,
                DIB_RGB_COLORS,
            );
            windows::Win32::Graphics::Gdi::SelectObject(mem, old);
            let _ = DeleteObject(bmp);
            let _ = DeleteDC(mem);
            ReleaseDC(windows::Win32::Foundation::HWND::default(), screen);
            if lines == 0 {
                return Err(BvError::msg("GetDIBits failed"));
            }
            write_bmp(&path, w as u32, h as u32, &pixels)?;
        }
        Ok(path.to_string_lossy().to_string())
    }

    /// Minimal 32-bit BMP writer — avoids pulling in an image encoder.
    fn write_bmp(path: &std::path::Path, w: u32, h: u32, bgra: &[u8]) -> BvResult<()> {
        use std::io::Write;
        let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
        let pixel_size = (w * h * 4) as u32;
        let file_size = 54 + pixel_size;
        f.write_all(b"BM")?;
        f.write_all(&file_size.to_le_bytes())?;
        f.write_all(&[0u8; 4])?;
        f.write_all(&54u32.to_le_bytes())?;
        f.write_all(&40u32.to_le_bytes())?;
        f.write_all(&(w as i32).to_le_bytes())?;
        f.write_all(&(h as i32).to_le_bytes())?;
        f.write_all(&1u16.to_le_bytes())?;
        f.write_all(&32u16.to_le_bytes())?;
        f.write_all(&0u32.to_le_bytes())?;
        f.write_all(&pixel_size.to_le_bytes())?;
        f.write_all(&2835i32.to_le_bytes())?;
        f.write_all(&2835i32.to_le_bytes())?;
        f.write_all(&0u32.to_le_bytes())?;
        f.write_all(&0u32.to_le_bytes())?;
        f.write_all(bgra)?;
        f.flush()?;
        Ok(())
    }

    fn dirs_pictures() -> std::path::PathBuf {
        let base = std::env::var("USERPROFILE").unwrap_or_else(|_| ".".into());
        std::path::Path::new(&base).join("Pictures").join("BLACKVAULT")
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;

    pub fn press_keys(_c: &str) -> BvResult<()> {
        Err(BvError::Unsupported("keyboard automation".into()))
    }
    pub fn tap_key(_v: u16) -> BvResult<()> {
        Err(BvError::Unsupported("keyboard automation".into()))
    }
    pub fn double_click(_x: Option<i32>, _y: Option<i32>) -> BvResult<()> {
        Err(BvError::Unsupported("mouse input".into()))
    }

    pub fn scroll_to(_top: bool) -> BvResult<()> {
        Err(BvError::Unsupported("mouse input".into()))
    }

    pub fn click(_x: Option<i32>, _y: Option<i32>, _b: &str) -> BvResult<()> {
        Err(BvError::Unsupported("mouse automation".into()))
    }
    pub fn scroll(_dx: i32, _dy: i32) -> BvResult<()> {
        Err(BvError::Unsupported("mouse automation".into()))
    }
    pub fn show_desktop() -> BvResult<()> {
        Err(BvError::Unsupported("show desktop".into()))
    }
    pub fn minimize_all() -> BvResult<()> {
        Err(BvError::Unsupported("minimize all".into()))
    }
    pub fn screenshot() -> BvResult<String> {
        Err(BvError::Unsupported("screenshot".into()))
    }
}

pub use imp::*;

/// Perform a named automation action. `payload` carries parameters where the
/// action needs them (key combo, scroll amount, click coordinates).
pub fn run_action(action: &str, payload: Option<&str>) -> BvResult<()> {
    match action {
        "media_next" => tap_key(vk_for("media_next")?),
        "media_prev" => tap_key(vk_for("media_prev")?),
        "media_play_pause" => tap_key(vk_for("media_play_pause")?),
        "volume_up" => tap_key(vk_for("volume_up")?),
        "volume_down" => tap_key(vk_for("volume_down")?),
        "volume_mute" => tap_key(vk_for("volume_mute")?),
        "key_press" => {
            let combo = payload.unwrap_or("enter");
            press_keys(combo)
        }
        "mouse_click" => {
            let (x, y) = parse_point(payload);
            click(x, y, "left")
        }
        "mouse_double_click" => {
            let (x, y) = parse_point(payload);
            double_click(x, y)
        }
        "mouse_right_click" => {
            let (x, y) = parse_point(payload);
            click(x, y, "right")
        }
        "mouse_middle_click" => {
            let (x, y) = parse_point(payload);
            click(x, y, "middle")
        }
        "scroll_top" => scroll_to(true),
        "scroll_bottom" => scroll_to(false),
        "scroll" => {
            let (dx, dy) = parse_scroll(payload);
            scroll(dx, dy)
        }
        "minimize_all" => minimize_all(),
        "show_desktop" => show_desktop(),
        "screenshot" => screenshot().map(|_| ()),
        other => Err(BvError::msg(format!("unknown automation action \"{other}\""))),
    }
}

/// `"400,300"` → (Some(400), Some(300)); `"none"`/empty → (None, None).
pub fn parse_point(payload: Option<&str>) -> (Option<i32>, Option<i32>) {
    match payload {
        None => (None, None),
        Some(p) => {
            let p = p.trim();
            if p.is_empty() || p.eq_ignore_ascii_case("here") || p.eq_ignore_ascii_case("none") {
                return (None, None);
            }
            let mut it = p.split([',', ' ', 'x']).filter(|s| !s.trim().is_empty());
            let x = it.next().and_then(|v| v.trim().parse::<i32>().ok());
            let y = it.next().and_then(|v| v.trim().parse::<i32>().ok());
            (x, y)
        }
    }
}

/// `"down:600"` → (0, 600); `"up:300"` → (0, -300); `"left:200"` → (-200, 0).
pub fn parse_scroll(payload: Option<&str>) -> (i32, i32) {
    let Some(p) = payload else { return (0, 600) };
    let p = p.trim().to_ascii_lowercase();
    if let Some(rest) = p.strip_prefix("down:") {
        return (0, rest.trim().parse().unwrap_or(600));
    }
    if let Some(rest) = p.strip_prefix("up:") {
        return (0, -rest.trim().parse().unwrap_or(600));
    }
    if let Some(rest) = p.strip_prefix("right:") {
        return (rest.trim().parse().unwrap_or(600), 0);
    }
    if let Some(rest) = p.strip_prefix("left:") {
        return (-rest.trim().parse().unwrap_or(600), 0);
    }
    // Bare number: treat as a downward scroll.
    (0, p.parse().unwrap_or(600))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modifier_combos() {
        let (mods, key) = parse_combo("ctrl+shift+esc").unwrap();
        assert_eq!(mods, vec![0x11, 0x10]);
        assert_eq!(key, 0x1B);
    }

    #[test]
    fn parses_single_letter_and_win_combo() {
        assert_eq!(parse_combo("a").unwrap().1, 0x41);
        let (mods, key) = parse_combo("win+d").unwrap();
        assert_eq!(mods, vec![0x5B]);
        assert_eq!(key, 0x44);
    }

    #[test]
    fn rejects_bad_combos() {
        assert!(parse_combo("ctrl+").is_err());
        assert!(parse_combo("ctrl+a+b").is_err());
        assert!(parse_combo("notakey").is_err());
    }

    #[test]
    fn parses_points() {
        assert_eq!(parse_point(Some("400,300")), (Some(400), Some(300)));
        assert_eq!(parse_point(Some("here")), (None, None));
        assert_eq!(parse_point(None), (None, None));
        assert_eq!(parse_point(Some("  ")), (None, None));
    }

    #[test]
    fn parses_scroll_directions() {
        assert_eq!(parse_scroll(Some("down:600")), (0, 600));
        assert_eq!(parse_scroll(Some("up:300")), (0, -300));
        assert_eq!(parse_scroll(Some("left:200")), (-200, 0));
        assert_eq!(parse_scroll(Some("750")), (0, 750));
        assert_eq!(parse_scroll(None), (0, 600));
    }
}
