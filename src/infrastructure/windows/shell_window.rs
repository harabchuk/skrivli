//! Show and hide the GPUI window without subclassing it.
//!
//! gpui-pre 0.3.11's Windows `Platform::hide` is a no-op and `Window` has no
//! hide method. The shell finds the existing `Zed::Window` titled Skrivli and
//! calls `ShowWindow`. It does not replace the window procedure.

use windows::core::w;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, IsIconic, SetForegroundWindow, ShowWindow, SW_HIDE, SW_SHOW,
};

const WINDOW_CLASS: windows::core::PCWSTR = w!("Zed::Window");
const WINDOW_TITLE: windows::core::PCWSTR = w!("Skrivli");

pub(crate) fn set_main_window_visible(visible: bool) -> bool {
    let Some(hwnd) = find_main_window() else {
        return false;
    };
    // SAFETY: the handle is a top-level window in this process, when found.
    unsafe {
        if visible {
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
        } else {
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
    }
    true
}

pub(crate) fn main_window_is_iconic() -> bool {
    let Some(hwnd) = find_main_window() else {
        return false;
    };
    // SAFETY: the handle is a top-level window in this process, when found.
    unsafe { IsIconic(hwnd).as_bool() }
}

pub(crate) fn main_window_hwnd() -> Option<HWND> {
    find_main_window()
}

fn find_main_window() -> Option<HWND> {
    // SAFETY: class and title are static NUL-terminated literals.
    let hwnd = unsafe { FindWindowW(WINDOW_CLASS, WINDOW_TITLE) }.ok()?;
    if hwnd.0.is_null() {
        None
    } else {
        Some(hwnd)
    }
}
