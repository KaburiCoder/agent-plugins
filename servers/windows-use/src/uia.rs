//! UI Automation access. Every function takes the worker thread's own `Uia`; COM objects
//! never cross threads.

use anyhow::Result;
use uiautomation::patterns::{
    UIExpandCollapsePattern, UIInvokePattern, UILegacyIAccessiblePattern, UISelectionItemPattern, UITextPattern,
    UITogglePattern, UIValuePattern,
};
use uiautomation::types::{ControlType, ExpandCollapseState, Handle};
use uiautomation::{UIAutomation, UIElement, UITreeWalker};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx};
use windows::Win32::UI::Accessibility::{CUIAutomation8, IUIAutomation, IUIAutomation2};
use windows::Win32::UI::WindowsAndMessaging::IsWindow;
use windows::core::Interface;

use crate::geom::ScreenRect;
use crate::input::NotPressed;

const UIA_TIMEOUT_MS: u32 = 2000;

pub struct Uia {
    pub auto: UIAutomation,
    /// The control view: what assistive technology shows, without layout-only wrappers.
    pub walker: UITreeWalker,
}

impl Uia {
    /// A client whose calls give up after a timeout: a program that ignores UI Automation would
    /// otherwise stall the caller for the default 20 seconds.
    pub fn new() -> Result<Self> {
        let auto = unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let raw: IUIAutomation = CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)?;
            if let Ok(two) = raw.cast::<IUIAutomation2>() {
                let _ = two.SetConnectionTimeout(UIA_TIMEOUT_MS);
                let _ = two.SetTransactionTimeout(UIA_TIMEOUT_MS);
            }
            UIAutomation::from(raw)
        };
        let walker = auto.get_control_view_walker()?;
        Ok(Self { auto, walker })
    }

    pub fn element_at(&self, x: i32, y: i32) -> Option<UIElement> {
        self.auto.element_from_point(uiautomation::types::Point::new(x, y)).ok()
    }

    /// The element behind a window handle, also when minimized or on another desktop.
    pub fn from_hwnd(&self, h: isize) -> Option<UIElement> {
        let hwnd = HWND(h as *mut _);
        if !unsafe { IsWindow(Some(hwnd)).as_bool() } {
            return None;
        }
        self.auto.element_from_handle(Handle::from(hwnd)).ok()
    }

    pub fn children(&self, el: &UIElement) -> Vec<UIElement> {
        let mut out = Vec::new();
        let mut child = self.walker.get_first_child(el).ok();
        while let Some(c) = child {
            child = self.walker.get_next_sibling(&c).ok();
            out.push(c);
        }
        out
    }

    /// The top-level window an element is drawn in, by its nearest ancestor with a window handle
    /// (grid cells, toolbar buttons and list items have none of their own).
    pub fn window_of(&self, el: &UIElement) -> Option<isize> {
        let mut cur = el.clone();
        for _ in 0..64 {
            if let Some(h) = handle_of(&cur) {
                return Some(crate::win32::root_of(h));
            }
            cur = self.walker.get_parent(&cur).ok()?;
        }
        None
    }
}

/// The element's own window handle; None for parts drawn inside a window.
pub fn handle_of(el: &UIElement) -> Option<isize> {
    let h: Handle = el.get_native_window_handle().ok()?;
    let v: isize = h.into();
    (v != 0).then_some(v)
}

pub fn rect_of(el: &UIElement) -> Option<ScreenRect> {
    el.get_bounding_rectangle().ok().map(ScreenRect::from).filter(|r| !r.is_empty())
}

pub fn is_visible(el: &UIElement) -> bool {
    rect_of(el).is_some() && !el.is_offscreen().unwrap_or(false)
}

pub fn type_name(el: &UIElement) -> String {
    el.get_control_type().map(|c| format!("{c:?}")).unwrap_or_else(|_| "Unknown".into())
}

/// The UIA control type id for a name such as `Button` (case ignored).
pub fn control_type_id(name: &str) -> Option<i32> {
    (50000..=50040).find(|&id| ControlType::try_from(id).is_ok_and(|c| format!("{c:?}").eq_ignore_ascii_case(name)))
}

/// UIA_E_TIMEOUT: the program did not answer in time.
const UIA_E_TIMEOUT: i32 = 0x8013_1505_u32 as i32;
/// UIA_E_ELEMENTNOTENABLED
const UIA_E_NOT_ENABLED: i32 = 0x8004_0200_u32 as i32;
/// Errors raised before the element acted: not supported here (UIA_E_NOTSUPPORTED,
/// UIA_E_INVALIDOPERATION, E_NOTIMPL, E_NOINTERFACE).
const TURNED_DOWN: [i32; 4] = [0x8004_0204_u32 as i32, 0x8013_1509_u32 as i32, 0x8000_4001_u32 as i32, 0x8000_4002_u32 as i32];

/// What a failed press means, by its error code.
fn press_error(code: i32, message: &str) -> Result<(), NotPressed> {
    match code {
        // The program is still busy with what the press started (a form opening): pressing
        // again with the mouse would do it twice.
        UIA_E_TIMEOUT => Ok(()),
        UIA_E_NOT_ENABLED => Err(NotPressed::Disabled),
        c if TURNED_DOWN.contains(&c) => Err(NotPressed::Refused(format!("요소가 거부함 ({message})"))),
        // Anything else may come after the press took effect (the button closed its window).
        _ => Err(NotPressed::Unknown(message.to_string())),
    }
}

/// Presses the element through what it offers for that (invoke, toggle, select, expand),
/// without the mouse.
pub fn press(el: &UIElement) -> Result<&'static str, NotPressed> {
    let done = |how: &'static str, r: uiautomation::Result<()>| r.map(|_| how).or_else(|e| press_error(e.code(), e.message()).map(|_| how));
    let refused = |why: &str| Err(NotPressed::Refused(why.into()));
    match el.is_enabled() {
        Ok(true) => {}
        Ok(false) => return Err(NotPressed::Disabled),
        Err(_) => return Err(NotPressed::Gone),
    }
    // An item's invoke is often its double click (opening it); a click selects it.
    let item = matches!(
        el.get_control_type(),
        Ok(ControlType::ListItem | ControlType::TreeItem | ControlType::DataItem | ControlType::TabItem | ControlType::RadioButton)
    );
    if item {
        return match el.get_pattern::<UISelectionItemPattern>() {
            Ok(p) => done("select", p.select()),
            Err(_) => refused("고르는 기능이 없는 항목"),
        };
    }
    if let Ok(p) = el.get_pattern::<UIInvokePattern>() {
        return done("invoke", p.invoke());
    }
    if let Ok(p) = el.get_pattern::<UITogglePattern>() {
        return done("toggle", p.toggle());
    }
    if let Ok(p) = el.get_pattern::<UISelectionItemPattern>() {
        return done("select", p.select());
    }
    if let Ok(p) = el.get_pattern::<UIExpandCollapsePattern>() {
        return match p.get_state() {
            Ok(ExpandCollapseState::Collapsed) => done("expand", p.expand()),
            Ok(_) => done("collapse", p.collapse()),
            Err(_) => refused("펼침 상태를 읽지 못함"),
        };
    }
    refused("누르는 기능이 없는 요소")
}

/// Reads what the element shows. An input's value wins even when empty; the name is only
/// used for plain labels, which carry their text there.
pub fn read_value(el: &UIElement) -> Option<String> {
    if el.is_password().unwrap_or(false) {
        return Some("(비밀번호 칸: 읽지 않음)".into());
    }
    if let Ok(p) = el.get_pattern::<UIValuePattern>()
        && let Ok(v) = p.get_value()
    {
        return Some(v);
    }
    if let Ok(p) = el.get_pattern::<UITextPattern>()
        && let Ok(v) = p.get_document_range().and_then(|r| r.get_text(-1))
    {
        return Some(v);
    }
    if let Ok(p) = el.get_pattern::<UILegacyIAccessiblePattern>()
        && let Ok(v) = p.get_value()
        && !v.is_empty()
    {
        return Some(v);
    }
    el.get_name().ok()
}

/// The short value shown next to an element in listings: inputs only, not their name again.
pub fn value_hint(el: &UIElement) -> Option<String> {
    if el.is_password().unwrap_or(false) {
        return None;
    }
    let p = el.get_pattern::<UIValuePattern>().ok()?;
    p.get_value().ok()
}

/// Sets the value through ValuePattern. Err with the reason when the element has none or it
/// is read-only, so the caller can type instead.
pub fn set_value(el: &UIElement, value: &str) -> Result<(), String> {
    let p = el.get_pattern::<UIValuePattern>().map_err(|_| "값 설정 기능(ValuePattern)이 없음".to_string())?;
    if p.is_readonly().unwrap_or(false) {
        return Err("읽기 전용".into());
    }
    p.set_value(value).map_err(|e| e.message().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(code: u32) -> &'static str {
        match press_error(code as i32, "") {
            Ok(()) => "pressed",
            Err(NotPressed::Refused(_)) => "refused",
            Err(NotPressed::Disabled) => "disabled",
            Err(NotPressed::Gone) => "gone",
            Err(NotPressed::Unknown(_)) => "unknown",
        }
    }

    /// Only errors raised before the element acted may lead to a mouse click; anything else
    /// could press the target twice.
    #[test]
    fn only_errors_before_acting_allow_the_mouse() {
        assert_eq!(kind(0x8013_1505), "pressed");
        assert_eq!(kind(0x8004_0200), "disabled");
        for code in [0x8004_0204, 0x8013_1509, 0x8000_4001, 0x8000_4002] {
            assert_eq!(kind(code), "refused", "{code:#x}");
        }
        for code in [0x8004_0201, 0x8001_0108, 0x8007_06BE, 0x8000_4005] {
            assert_eq!(kind(code), "unknown", "{code:#x}");
        }
    }
}
