//! Mouse and keyboard injection through SendInput. Coordinates are physical screen pixels,
//! which is what UIA and xcap report in a per-monitor DPI aware process.

use std::thread::sleep;
use std::time::Duration;

use anyhow::{Result, bail};
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, GetCursorPos, GetForegroundWindow, GetWindowThreadProcessId, IsIconic, SW_RESTORE,
    SetCursorPos, SetForegroundWindow, ShowWindow,
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ClickKind {
    Left,
    Double,
    Right,
    Middle,
}

impl ClickKind {
    pub fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "" | "left" => Self::Left,
            "double" => Self::Double,
            "right" => Self::Right,
            "middle" => Self::Middle,
            other => bail!("알 수 없는 버튼: {other} (left·double·right·middle)"),
        })
    }
}

/// Why a target was not pressed without the mouse.
pub enum NotPressed {
    /// Turned down before anything happened: the mouse may press it instead.
    Refused(String),
    /// Disabled for now, usually because its form is still loading.
    Disabled,
    /// No longer there, or not answering: what was found is out of date.
    Gone,
    /// Failed partway: it may have been pressed, so it must not be pressed again.
    Unknown(String),
}

fn send(inputs: &[INPUT]) {
    unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
}

fn mouse(flags: MOUSE_EVENT_FLAGS, data: i32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 { mi: MOUSEINPUT { dwFlags: flags, mouseData: data as u32, ..Default::default() } },
    }
}

fn key(vk: VIRTUAL_KEY, up: bool) -> INPUT {
    let mut flags = KEYBD_EVENT_FLAGS(0);
    if up {
        flags |= KEYEVENTF_KEYUP;
    }
    if is_extended(vk) {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, dwFlags: flags, ..Default::default() } } }
}

fn is_extended(vk: VIRTUAL_KEY) -> bool {
    matches!(
        vk,
        VK_HOME | VK_END | VK_PRIOR | VK_NEXT | VK_LEFT | VK_RIGHT | VK_UP | VK_DOWN | VK_INSERT | VK_DELETE | VK_LWIN
            | VK_APPS | VK_RCONTROL | VK_RMENU
    )
}

pub fn cursor() -> (i32, i32) {
    let mut p = POINT::default();
    let _ = unsafe { GetCursorPos(&mut p) };
    (p.x, p.y)
}

pub fn move_to(x: i32, y: i32) {
    unsafe {
        let _ = SetCursorPos(x, y);
    }
    sleep(Duration::from_millis(30));
}

pub fn click_at(x: i32, y: i32, kind: ClickKind) {
    move_to(x, y);
    let pair = |down, up| send(&[mouse(down, 0), mouse(up, 0)]);
    match kind {
        ClickKind::Left => pair(MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
        ClickKind::Right => pair(MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
        ClickKind::Middle => pair(MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP),
        ClickKind::Double => {
            pair(MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP);
            sleep(Duration::from_millis(60));
            pair(MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP);
        }
    }
}

/// Holds the left button from one point to another, moving in steps so drag-and-drop
/// targets see the pointer travel.
pub fn drag(from: (i32, i32), to: (i32, i32)) {
    move_to(from.0, from.1);
    send(&[mouse(MOUSEEVENTF_LEFTDOWN, 0)]);
    sleep(Duration::from_millis(80));
    const STEPS: i32 = 12;
    for i in 1..=STEPS {
        let x = from.0 + (to.0 - from.0) * i / STEPS;
        let y = from.1 + (to.1 - from.1) * i / STEPS;
        unsafe {
            let _ = SetCursorPos(x, y);
        }
        sleep(Duration::from_millis(15));
    }
    sleep(Duration::from_millis(60));
    send(&[mouse(MOUSEEVENTF_LEFTUP, 0)]);
}

/// Wheel notches at a point: positive scrolls up (or right when `horizontal`).
pub fn scroll(x: i32, y: i32, notches: i32, horizontal: bool) {
    move_to(x, y);
    let flag = if horizontal { MOUSEEVENTF_HWHEEL } else { MOUSEEVENTF_WHEEL };
    for _ in 0..notches.unsigned_abs() {
        send(&[mouse(flag, notches.signum() * 120)]);
        sleep(Duration::from_millis(20));
    }
}

/// Types text as Unicode characters, so it does not depend on the keyboard layout or the
/// IME state (Korean input mode would otherwise turn Latin letters into Hangul).
pub fn type_text(text: &str) {
    for unit in text.encode_utf16() {
        if unit == '\n' as u16 {
            send(&[key(VK_RETURN, false), key(VK_RETURN, true)]);
            continue;
        }
        if unit == '\r' as u16 {
            continue;
        }
        if unit == '\t' as u16 {
            send(&[key(VK_TAB, false), key(VK_TAB, true)]);
            continue;
        }
        let ki = |flags| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 { ki: KEYBDINPUT { wScan: unit, dwFlags: flags, ..Default::default() } },
        };
        send(&[ki(KEYEVENTF_UNICODE), ki(KEYEVENTF_UNICODE | KEYEVENTF_KEYUP)]);
        sleep(Duration::from_millis(4));
    }
}

/// Presses a combination such as `Ctrl+Shift+S`, `Enter` or `Alt+F4`.
pub fn key_combo(combo: &str) -> Result<()> {
    let mut keys = Vec::new();
    for part in combo.split('+').map(str::trim).filter(|p| !p.is_empty()) {
        keys.push(parse_key(part).ok_or_else(|| anyhow::anyhow!("알 수 없는 키: {part}"))?);
    }
    if keys.is_empty() {
        bail!("누를 키가 비어 있음");
    }
    let mut inputs: Vec<INPUT> = keys.iter().map(|&k| key(k, false)).collect();
    inputs.extend(keys.iter().rev().map(|&k| key(k, true)));
    send(&inputs);
    Ok(())
}

fn parse_key(name: &str) -> Option<VIRTUAL_KEY> {
    let upper = name.to_ascii_uppercase();
    let vk = match upper.as_str() {
        "CTRL" | "CONTROL" => VK_CONTROL,
        "SHIFT" => VK_SHIFT,
        "ALT" => VK_MENU,
        "WIN" | "SUPER" | "META" => VK_LWIN,
        "ENTER" | "RETURN" => VK_RETURN,
        "TAB" => VK_TAB,
        "ESC" | "ESCAPE" => VK_ESCAPE,
        "SPACE" => VK_SPACE,
        "BACKSPACE" | "BKSP" => VK_BACK,
        "DELETE" | "DEL" => VK_DELETE,
        "INSERT" | "INS" => VK_INSERT,
        "HOME" => VK_HOME,
        "END" => VK_END,
        "PAGEUP" | "PGUP" => VK_PRIOR,
        "PAGEDOWN" | "PGDN" => VK_NEXT,
        "UP" | "↑" => VK_UP,
        "DOWN" | "↓" => VK_DOWN,
        "LEFT" | "←" => VK_LEFT,
        "RIGHT" | "→" => VK_RIGHT,
        "APPS" | "MENU" | "CONTEXT" => VK_APPS,
        "PRINTSCREEN" | "PRTSC" => VK_SNAPSHOT,
        "CAPSLOCK" => VK_CAPITAL,
        "NUMLOCK" => VK_NUMLOCK,
        "SCROLLLOCK" => VK_SCROLL,
        "PAUSE" => VK_PAUSE,
        "HANGUL" | "한영" => VK_HANGUL,
        "HANJA" | "한자" => VK_HANJA,
        "-" | "MINUS" => VK_OEM_MINUS,
        "=" | "PLUS" | "EQUALS" => VK_OEM_PLUS,
        "," | "COMMA" => VK_OEM_COMMA,
        "." | "PERIOD" => VK_OEM_PERIOD,
        "/" | "SLASH" => VK_OEM_2,
        ";" | "SEMICOLON" => VK_OEM_1,
        "'" | "QUOTE" => VK_OEM_7,
        "[" => VK_OEM_4,
        "]" => VK_OEM_6,
        "\\" | "BACKSLASH" => VK_OEM_5,
        "`" | "BACKTICK" => VK_OEM_3,
        _ => {
            if let Some(n) = upper.strip_prefix("NUM").and_then(|n| n.parse::<u16>().ok())
                && n <= 9
            {
                return Some(VIRTUAL_KEY(VK_NUMPAD0.0 + n));
            }
            if let Some(n) = upper.strip_prefix('F').and_then(|n| n.parse::<u16>().ok())
                && (1..=24).contains(&n)
            {
                return Some(VIRTUAL_KEY(VK_F1.0 + n - 1));
            }
            let mut chars = upper.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) if c.is_ascii_alphanumeric() => VIRTUAL_KEY(c as u16),
                _ => return None,
            }
        }
    };
    Some(vk)
}

/// Brings a top-level window to the front. Windows refuses SetForegroundWindow from a
/// background process, so this escalates: plain request, then sharing the foreground
/// thread's input state, then an Alt press as a last resort. True when it is in front.
pub fn focus_window(hwnd: isize) -> bool {
    let hwnd = HWND(hwnd as *mut _);
    let in_front = || unsafe { GetForegroundWindow() } == hwnd;
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        if !(SetForegroundWindow(hwnd).as_bool() && in_front()) {
            let fg_thread = GetWindowThreadProcessId(GetForegroundWindow(), None);
            let me = GetCurrentThreadId();
            if fg_thread != 0 && fg_thread != me && AttachThreadInput(me, fg_thread, true).as_bool() {
                let _ = BringWindowToTop(hwnd);
                let _ = SetForegroundWindow(hwnd);
                let _ = AttachThreadInput(me, fg_thread, false);
            }
        }
        if !in_front() {
            // A bare Alt press and release opens the menu bar of the window that receives the
            // release; an unassigned key in between keeps it a chord, so no menu activates.
            send(&[key(VK_MENU, false), key(VK_NONAME, false), key(VK_NONAME, true)]);
            let _ = SetForegroundWindow(hwnd);
            send(&[key(VK_MENU, true)]);
        }
    }
    sleep(Duration::from_millis(120));
    in_front()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_keys() {
        assert_eq!(parse_key("ctrl"), Some(VK_CONTROL));
        assert_eq!(parse_key("F12"), Some(VK_F12));
        assert_eq!(parse_key("a"), Some(VIRTUAL_KEY('A' as u16)));
        assert_eq!(parse_key("num5"), Some(VK_NUMPAD5));
        assert_eq!(parse_key("한영"), Some(VK_HANGUL));
        assert_eq!(parse_key("nope"), None);
        assert!(key_combo("").is_err());
    }
}
