//! Window handles: listing, describing and moving top-level windows, and the plain child-window
//! tree for programs whose UI Automation provider refuses us or exposes nothing useful.
//! WinForms and Win32 controls are real child windows, so they can be found by class, text and
//! — for WinForms — the control's own `Name` property.

use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use windows::Win32::System::Memory::{MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAllocEx, VirtualFreeEx};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_OPERATION, PROCESS_VM_READ,
    QueryFullProcessImageNameW,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, PWSTR, w};

use crate::geom::ScreenRect;

/// Per window message: a busy program must not stall us.
const TIMEOUT_MS: u32 = 500;

pub fn hwnd(h: isize) -> HWND {
    HWND(h as *mut _)
}

pub fn exists(h: isize) -> bool {
    unsafe { IsWindow(Some(hwnd(h))).as_bool() }
}

pub fn pid_of(h: isize) -> u32 {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd(h), Some(&mut pid)) };
    pid
}

pub fn is_visible(h: isize) -> bool {
    unsafe { IsWindowVisible(hwnd(h)).as_bool() }
}

pub fn is_minimized(h: isize) -> bool {
    unsafe { IsIconic(hwnd(h)).as_bool() }
}

pub fn is_maximized(h: isize) -> bool {
    unsafe { IsZoomed(hwnd(h)).as_bool() }
}

pub fn root_of(h: isize) -> isize {
    unsafe { GetAncestor(hwnd(h), GA_ROOT).0 as isize }
}

pub fn foreground() -> isize {
    unsafe { GetForegroundWindow().0 as isize }
}

pub fn is_tool_window(h: isize) -> bool {
    let ex = unsafe { GetWindowLongW(hwnd(h), GWL_EXSTYLE) } as u32;
    ex & WS_EX_TOOLWINDOW.0 != 0
}

pub fn class_name(h: isize) -> String {
    let mut buf = [0u16; 256];
    let n = unsafe { GetClassNameW(hwnd(h), &mut buf) } as usize;
    String::from_utf16_lossy(&buf[..n])
}

/// `WindowsForms10.BUTTON.app.0.2726b20_r7_ad1` → `WindowsForms10.BUTTON`: the suffix
/// changes between builds and runs of the same program.
pub fn stable_class(class: &str) -> String {
    match class.find(".app.") {
        Some(i) if class.starts_with("WindowsForms") => class[..i].to_string(),
        _ => class.to_string(),
    }
}

/// The window's text, read with WM_GETTEXT so it also works for edit controls of other
/// processes (GetWindowText does not).
pub fn text(h: isize) -> String {
    unsafe {
        let mut len = 0usize;
        if SendMessageTimeoutW(hwnd(h), WM_GETTEXTLENGTH, WPARAM(0), LPARAM(0), SMTO_ABORTIFHUNG, TIMEOUT_MS, Some(&mut len))
            .0
            == 0
        {
            return String::new();
        }
        let mut buf = vec![0u16; len.min(1 << 20) + 1];
        let mut got = 0usize;
        SendMessageTimeoutW(
            hwnd(h),
            WM_GETTEXT,
            WPARAM(buf.len()),
            LPARAM(buf.as_mut_ptr() as isize),
            SMTO_ABORTIFHUNG,
            TIMEOUT_MS,
            Some(&mut got),
        );
        String::from_utf16_lossy(&buf[..got.min(buf.len())])
    }
}

/// Replaces the window's text with WM_SETTEXT, which the system copies across processes.
pub fn set_text(h: isize, value: &str) -> bool {
    let wide: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
    let mut result = 0usize;
    let sent = unsafe {
        SendMessageTimeoutW(
            hwnd(h),
            WM_SETTEXT,
            WPARAM(0),
            LPARAM(wide.as_ptr() as isize),
            SMTO_ABORTIFHUNG,
            TIMEOUT_MS * 4,
            Some(&mut result),
        )
    };
    sent.0 != 0 && result != 0
}

/// A top-level window's caption, as the system keeps it (a busy program cannot stall us).
pub fn title(h: isize) -> String {
    let mut buf = [0u16; 512];
    let n = unsafe { GetWindowTextW(hwnd(h), &mut buf) } as usize;
    String::from_utf16_lossy(&buf[..n.min(buf.len())])
}

pub fn rect(h: isize) -> Option<ScreenRect> {
    let mut r = RECT::default();
    unsafe { GetWindowRect(hwnd(h), &mut r) }.ok()?;
    let r = ScreenRect { left: r.left, top: r.top, right: r.right, bottom: r.bottom };
    (!r.is_empty()).then_some(r)
}

pub fn children(h: isize) -> Vec<isize> {
    let mut out = Vec::new();
    unsafe {
        let mut c = GetWindow(hwnd(h), GW_CHILD).unwrap_or_default();
        while !c.is_invalid() {
            out.push(c.0 as isize);
            c = GetWindow(c, GW_HWNDNEXT).unwrap_or_default();
        }
    }
    out
}

/// Visible top-level windows, front to back.
pub fn top_windows() -> Vec<isize> {
    unsafe extern "system" fn collect(h: HWND, l: LPARAM) -> BOOL {
        let out = unsafe { &mut *(l.0 as *mut Vec<isize>) };
        if unsafe { IsWindowVisible(h).as_bool() } {
            out.push(h.0 as isize);
        }
        BOOL(1)
    }
    let mut out: Vec<isize> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM(&mut out as *mut _ as isize));
    }
    out
}

/// Open on another of Windows' virtual desktops (hidden from this one, so cloaked). Bringing
/// it forward switches to that desktop.
pub fn on_other_desktop(h: isize) -> bool {
    use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx};
    use windows::Win32::UI::Shell::{IVirtualDesktopManager, VirtualDesktopManager};
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let Ok(desktops) = CoCreateInstance::<_, IVirtualDesktopManager>(&VirtualDesktopManager, None, CLSCTX_ALL) else {
            return false;
        };
        desktops.IsWindowOnCurrentVirtualDesktop(hwnd(h)).is_ok_and(|here| !here.as_bool())
    }
}

/// Shown but not on screen: on another virtual desktop, or a suspended app's frame.
pub fn is_cloaked(h: isize) -> bool {
    use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
    let mut cloaked = 0u32;
    let got = unsafe {
        DwmGetWindowAttribute(hwnd(h), DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut _, std::mem::size_of::<u32>() as u32)
    };
    got.is_ok() && cloaked != 0
}

/// The front-most visible, uncloaked top-level window under a screen point.
pub fn top_window_at(x: i32, y: i32) -> Option<isize> {
    top_windows().into_iter().find(|&h| {
        let ex = unsafe { GetWindowLongW(hwnd(h), GWL_EXSTYLE) } as u32;
        ex & WS_EX_TRANSPARENT.0 == 0 && rect(h).is_some_and(|r| r.contains(x, y)) && !is_cloaked(h)
    })
}

/// The deepest visible child window under a screen point.
pub fn deepest_at(x: i32, y: i32) -> Option<isize> {
    unsafe {
        let mut cur = hwnd(top_window_at(x, y)?);
        loop {
            let mut pt = POINT { x, y };
            let _ = windows::Win32::Graphics::Gdi::ScreenToClient(cur, &mut pt);
            let child = ChildWindowFromPointEx(cur, pt, CWP_SKIPINVISIBLE | CWP_SKIPTRANSPARENT);
            if child.is_invalid() || child == cur {
                return Some(cur.0 as isize);
            }
            cur = child;
        }
    }
}

/// Presses a button control with BM_CLICK, without the mouse. Posted rather than sent: a
/// button that opens a dialog would keep a sent message waiting until the dialog closes.
pub fn press(h: isize) -> Result<(), crate::input::NotPressed> {
    use crate::input::NotPressed;
    use windows::Win32::Graphics::Gdi::ClientToScreen;
    use windows::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled;
    let refused = |why: &str| Err(NotPressed::Refused(why.into()));
    if !exists(h) {
        return Err(NotPressed::Gone);
    }
    // The system class underneath, also for buttons of Delphi, VB or WinForms programs.
    let mut buf = [0u16; 64];
    let n = unsafe { RealGetWindowClassW(hwnd(h), &mut buf) } as usize;
    let style = unsafe { GetWindowLongW(hwnd(h), GWL_STYLE) } as u32;
    if !String::from_utf16_lossy(&buf[..n.min(buf.len())]).eq_ignore_ascii_case("Button")
        || style & BS_TYPEMASK as u32 == BS_GROUPBOX as u32
    {
        return refused("버튼이 아닌 컨트롤");
    }
    if !unsafe { IsWindowEnabled(hwnd(h)) }.as_bool() {
        return Err(NotPressed::Disabled);
    }
    // BM_CLICK clicks the button's top-left corner, and WinForms buttons ignore a click whose
    // point is covered by another window.
    let mut corner = POINT { x: 0, y: 0 };
    if !unsafe { ClientToScreen(hwnd(h), &mut corner) }.as_bool() || unsafe { WindowFromPoint(corner) } != hwnd(h) {
        return refused("버튼 모서리가 다른 창에 가려짐");
    }
    unsafe { PostMessageW(Some(hwnd(h)), BM_CLICK, WPARAM(0), LPARAM(0)) }.map_err(|e| NotPressed::Refused(e.message()))
}

pub fn is_enabled(h: isize) -> bool {
    unsafe { windows::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled(hwnd(h)) }.as_bool()
}

/// Waits until the window's thread has handled what was posted to it, within two seconds.
pub fn settle(h: isize) {
    unsafe {
        SendMessageTimeoutW(hwnd(h), WM_NULL, WPARAM(0), LPARAM(0), SMTO_ABORTIFHUNG, 2000, None);
    }
    std::thread::sleep(std::time::Duration::from_millis(100));
}

/// The `Name` a WinForms developer gave the control (`btnSave`), via the registered
/// WM_GETCONTROLNAME message that test tools use. The string is written into a buffer
/// allocated inside the target process.
pub fn control_name(h: isize) -> Option<String> {
    if !class_name(h).starts_with("WindowsForms") {
        return None;
    }
    const CHARS: usize = 256;
    unsafe {
        let msg = RegisterWindowMessageW(w!("WM_GETCONTROLNAME"));
        let process = OpenProcess(PROCESS_VM_OPERATION | PROCESS_VM_READ, false, pid_of(h)).ok()?;
        let remote = VirtualAllocEx(process, None, CHARS * 2, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
        let mut result = None;
        if !remote.is_null() {
            let mut written = 0usize;
            let sent =
                SendMessageTimeoutW(hwnd(h), msg, WPARAM(CHARS), LPARAM(remote as isize), SMTO_ABORTIFHUNG, TIMEOUT_MS, Some(&mut written));
            let mut buf = [0u16; CHARS];
            if sent.0 != 0 && ReadProcessMemory(process, remote, buf.as_mut_ptr() as *mut _, CHARS * 2, None).is_ok() {
                let end = buf.iter().position(|&c| c == 0).unwrap_or(CHARS);
                result = Some(String::from_utf16_lossy(&buf[..end])).filter(|s| !s.is_empty());
            }
            let _ = VirtualFreeEx(process, remote, 0, MEM_RELEASE);
        }
        let _ = CloseHandle(process);
        result
    }
}

/// The exe file name and full path of a process; empty when it cannot be opened.
pub fn process_info(pid: u32) -> (String, String) {
    if pid == 0 {
        return (String::new(), String::new());
    }
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return (process_name_from_list(pid).unwrap_or_default(), String::new());
        };
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(h);
        if !ok {
            return (process_name_from_list(pid).unwrap_or_default(), String::new());
        }
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        let name = path.rsplit('\\').next().unwrap_or_default().to_string();
        (name, path)
    }
}

/// The exe name from the process list, which is there even for processes we may not open.
fn process_name_from_list(pid: u32) -> Option<String> {
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
    };
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
        let mut entry = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut more = Process32FirstW(snap, &mut entry).is_ok();
        let mut found = None;
        while more {
            if entry.th32ProcessID == pid {
                let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
                found = Some(String::from_utf16_lossy(&entry.szExeFile[..len]));
                break;
            }
            more = Process32NextW(snap, &mut entry).is_ok();
        }
        let _ = CloseHandle(snap);
        found
    }
}

/// Whether the process runs with administrator rights. Windows keeps a normal process from
/// seeing into or sending input to such programs (UIPI). A process whose token we may not
/// even read is treated as elevated.
pub fn is_elevated(pid: u32) -> bool {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
    use windows::Win32::System::Threading::OpenProcessToken;
    unsafe {
        let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return true };
        let mut token = HANDLE::default();
        let opened = OpenProcessToken(process, TOKEN_QUERY, &mut token).is_ok();
        let _ = CloseHandle(process);
        if !opened {
            return true;
        }
        let mut elevation = TOKEN_ELEVATION::default();
        let mut len = 0u32;
        let read = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut _),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
        .is_ok();
        let _ = CloseHandle(token);
        read && elevation.TokenIsElevated != 0
    }
}

/// The window belongs to an administrator program we, running normally, cannot drive.
pub fn out_of_reach(h: isize) -> bool {
    !is_elevated(std::process::id()) && is_elevated(pid_of(h))
}

pub fn show(h: isize, cmd: SHOW_WINDOW_CMD) {
    unsafe {
        let _ = ShowWindow(hwnd(h), cmd);
    }
}

pub fn move_window(h: isize, r: ScreenRect) -> bool {
    unsafe {
        SetWindowPos(hwnd(h), None, r.left, r.top, r.width(), r.height(), SWP_NOZORDER | SWP_NOACTIVATE).is_ok()
    }
}

/// Asks the window to close, as its close button would; the program may still ask to save.
pub fn close(h: isize) -> bool {
    unsafe { PostMessageW(Some(hwnd(h)), WM_CLOSE, WPARAM(0), LPARAM(0)).is_ok() }
}

/// Plain kind of a Win32/WinForms control by its class.
pub fn kind_label(class: &str) -> &'static str {
    let c = class.to_ascii_uppercase();
    if c.contains("BUTTON") {
        "Button"
    } else if c.contains("EDIT") {
        "Edit"
    } else if c.contains("STATIC") {
        "Text"
    } else if c.contains("COMBOBOX") {
        "ComboBox"
    } else if c.contains("LISTBOX") || c.contains("LISTVIEW") {
        "List"
    } else if c.contains("TREEVIEW") {
        "Tree"
    } else if c.contains("TABCONTROL") || c.contains("SYSTABCONTROL") {
        "Tab"
    } else {
        "Control"
    }
}

/// Labels from HTML-capable controls arrive as markup (`<font ...><b>text</b></font>`).
pub fn strip_markup(s: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_class_drops_build_suffix() {
        assert_eq!(stable_class("WindowsForms10.BUTTON.app.0.2726b20_r7_ad1"), "WindowsForms10.BUTTON");
        assert_eq!(stable_class("Notepad"), "Notepad");
    }

    #[test]
    fn markup_is_stripped() {
        assert_eq!(strip_markup("<font color=red><b>저장</b></font>"), "저장");
    }
}
