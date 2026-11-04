//! Notification icon on a hidden message window.
//!
//! The window procedure is ours. It is not attached to the GPUI HWND.
//! `Shell_NotifyIconW` uses a stable GUID. Explorer broadcasts `TaskbarCreated`
//! after it restarts; that message adds the icon again. Normal stop posts
//! `WM_CLOSE`, which removes the icon before the thread exits.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use windows::core::{w, GUID, PCWSTR};
use windows::Win32::Foundation::{
    GetLastError, ERROR_CLASS_ALREADY_EXISTS, HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_GUID, NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE,
    NIM_SETVERSION, NOTIFYICONDATAW, NOTIFYICON_VERSION_4,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, GetCursorPos, GetMessageW, KillTimer, LoadIconW, PostMessageW,
    PostQuitMessage, RegisterClassW, RegisterWindowMessageW, SetForegroundWindow, SetTimer,
    ShowWindow, TrackPopupMenuEx, TranslateMessage, IDI_APPLICATION, MF_GRAYED, MF_STRING, SW_HIDE,
    TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLOSE,
    WM_CONTEXTMENU, WM_DESTROY, WM_LBUTTONDBLCLK, WM_LBUTTONUP, WM_RBUTTONUP, WM_TIMER, WM_USER,
    WNDCLASSW,
};

use crate::common::error::AppError;

/// Stable notify-icon GUID. Do not change it across builds.
const TRAY_GUID: GUID = GUID::from_u128(0x7c3e_8a21_4b6d_4e91_9f2a_1d5c_8b0e_6a44);
const TRAY_CALLBACK: u32 = WM_USER + 20;
const CLASS_NAME: PCWSTR = w!("Skrivli.TrayMessage");
const TOOLTIP: &str = "Skrivli";

const ID_SHOW: usize = 1;
const ID_START: usize = 2;
const ID_STOP: usize = 3;
const ID_FOLDER: usize = 4;
const ID_SETTINGS: usize = 5;
const ID_EXIT: usize = 6;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TrayCommand {
    Show,
    ToggleVisibility,
    StartRecording,
    StopRecording,
    OpenRecordingsFolder,
    Settings,
    Exit,
}

pub(crate) fn command_from_menu_id(id: i32) -> Option<TrayCommand> {
    match id as usize {
        ID_SHOW => Some(TrayCommand::Show),
        ID_START => Some(TrayCommand::StartRecording),
        ID_STOP => Some(TrayCommand::StopRecording),
        ID_FOLDER => Some(TrayCommand::OpenRecordingsFolder),
        ID_SETTINGS => Some(TrayCommand::Settings),
        ID_EXIT => Some(TrayCommand::Exit),
        _ => None,
    }
}

struct TrayState {
    on_command: Box<dyn Fn(TrayCommand) + Send>,
    stop: Arc<AtomicBool>,
    taskbar_created: u32,
    last_toggle: Instant,
}

thread_local! {
    static TRAY: RefCell<Option<TrayState>> = const { RefCell::new(None) };
}

/// Runs until `stop` is set and the message window closes. Call from a dedicated thread.
pub(crate) fn run(
    stop: Arc<AtomicBool>,
    hwnd_slot: Arc<AtomicIsize>,
    on_command: Box<dyn Fn(TrayCommand) + Send>,
) -> Result<(), AppError> {
    let taskbar_created = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
    TRAY.with(|slot| {
        *slot.borrow_mut() = Some(TrayState {
            on_command,
            stop,
            taskbar_created,
            last_toggle: Instant::now() - Duration::from_secs(1),
        });
    });
    let instance = module_instance()?;
    register_class(instance)?;
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS_NAME,
            w!("Skrivli.Tray"),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            windows::Win32::UI::WindowsAndMessaging::HWND_MESSAGE,
            windows::Win32::UI::WindowsAndMessaging::HMENU(std::ptr::null_mut()),
            instance,
            None,
        )
    }
    .map_err(|error| AppError::message("tray", error.to_string()))?;
    hwnd_slot.store(hwnd.0 as isize, Ordering::Release);
    let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
    if let Err(error) = add_icon(hwnd) {
        tracing::error!(error = %error, "tray icon was not added");
    }
    let _ = unsafe { SetTimer(hwnd, 1, 200, None) };
    pump();
    let _ = unsafe { KillTimer(hwnd, 1) };
    remove_icon(hwnd);
    hwnd_slot.store(0, Ordering::Release);
    TRAY.with(|slot| {
        *slot.borrow_mut() = None;
    });
    Ok(())
}

pub(crate) fn nudge(hwnd_slot: &AtomicIsize) {
    let raw = hwnd_slot.load(Ordering::Acquire);
    if raw == 0 {
        return;
    }
    let hwnd = HWND(raw as *mut _);
    // SAFETY: the tray thread published this message-window handle.
    let _ = unsafe { PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0)) };
}

fn pump() {
    loop {
        let mut message = unsafe { std::mem::zeroed() };
        let status = unsafe { GetMessageW(&mut message, None, 0, 0) };
        if status.0 == 0 || status.0 == -1 {
            break;
        }
        unsafe {
            let _ = TranslateMessage(&message);
            let _ = DispatchMessageW(&message);
        }
    }
}

fn module_instance() -> Result<HINSTANCE, AppError> {
    // SAFETY: a null module name returns this process's executable module.
    let module = unsafe { GetModuleHandleW(PCWSTR::null()) }
        .map_err(|error| AppError::message("tray", error.to_string()))?;
    Ok(HINSTANCE(module.0))
}

fn register_class(instance: HINSTANCE) -> Result<(), AppError> {
    let class = WNDCLASSW {
        lpfnWndProc: Some(tray_wndproc),
        hInstance: instance,
        lpszClassName: CLASS_NAME,
        ..Default::default()
    };
    let atom = unsafe { RegisterClassW(&class) };
    if atom == 0 {
        let error = unsafe { GetLastError() };
        if error != ERROR_CLASS_ALREADY_EXISTS {
            return Err(AppError::message("tray", error.0.to_string()));
        }
    }
    Ok(())
}

fn add_icon(hwnd: HWND) -> Result<(), AppError> {
    let mut data = icon_data(hwnd);
    // SAFETY: `data` is a fully sized NOTIFYICONDATAW for this message window.
    let added = unsafe { Shell_NotifyIconW(NIM_ADD, &data) };
    if !added.as_bool() {
        return Err(AppError::message(
            "tray",
            format!("NIM_ADD failed ({})", unsafe { GetLastError() }.0),
        ));
    }
    data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
    let versioned = unsafe { Shell_NotifyIconW(NIM_SETVERSION, &data) };
    if !versioned.as_bool() {
        tracing::warn!("tray icon version was not set");
    }
    Ok(())
}

fn remove_icon(hwnd: HWND) {
    let data = icon_data(hwnd);
    // SAFETY: deleting a missing icon is ignored by the shell.
    let _ = unsafe { Shell_NotifyIconW(NIM_DELETE, &data) };
}

fn icon_data(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut data = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_GUID | NIF_SHOWTIP,
        uCallbackMessage: TRAY_CALLBACK,
        guidItem: TRAY_GUID,
        ..Default::default()
    };
    if let Ok(icon) = unsafe { LoadIconW(None, IDI_APPLICATION) } {
        data.hIcon = icon;
    }
    write_wide(&mut data.szTip, TOOLTIP);
    data
}

fn write_wide(target: &mut [u16], text: &str) {
    for (slot, unit) in target
        .iter_mut()
        .zip(text.encode_utf16().chain(std::iter::once(0)))
    {
        *slot = unit;
    }
}

unsafe extern "system" fn tray_wndproc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_CLOSE || message == WM_DESTROY {
        remove_icon(hwnd);
        if message == WM_CLOSE {
            let _ = DestroyWindow(hwnd);
        } else {
            PostQuitMessage(0);
        }
        return LRESULT(0);
    }
    let taskbar = TRAY.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|state| state.taskbar_created)
            .unwrap_or(0)
    });
    if taskbar != 0 && message == taskbar {
        if let Err(error) = add_icon(hwnd) {
            tracing::error!(error = %error, "tray icon was not restored");
        }
        return LRESULT(0);
    }
    if message == WM_TIMER {
        let stop = TRAY.with(|slot| {
            slot.borrow()
                .as_ref()
                .is_some_and(|state| state.stop.load(Ordering::Acquire))
        });
        if stop {
            let _ = DestroyWindow(hwnd);
        }
        return LRESULT(0);
    }
    if message == TRAY_CALLBACK {
        handle_callback(hwnd, lparam);
        return LRESULT(0);
    }
    DefWindowProcW(hwnd, message, wparam, lparam)
}

fn handle_callback(hwnd: HWND, lparam: LPARAM) {
    let event = (lparam.0 as u32) & 0xFFFF;
    if event == WM_LBUTTONUP || event == WM_LBUTTONDBLCLK || event == WM_USER {
        toggle_once();
        return;
    }
    if event == WM_RBUTTONUP || event == WM_CONTEXTMENU {
        if let Some(command) = popup(hwnd) {
            emit(command);
        }
    }
}

fn toggle_once() {
    TRAY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(state) = slot.as_mut() else {
            return;
        };
        if state.last_toggle.elapsed() < Duration::from_millis(250) {
            return;
        }
        state.last_toggle = Instant::now();
        (state.on_command)(TrayCommand::ToggleVisibility);
    });
}

fn emit(command: TrayCommand) {
    TRAY.with(|slot| {
        if let Some(state) = slot.borrow().as_ref() {
            (state.on_command)(command);
        }
    });
}

fn popup(hwnd: HWND) -> Option<TrayCommand> {
    let menu = unsafe { CreatePopupMenu() }.ok()?;
    append(menu, ID_SHOW, "Show Skrivli", false);
    append(menu, ID_START, "Start recording", true);
    append(menu, ID_STOP, "Stop recording", true);
    append(menu, ID_FOLDER, "Open recordings folder", false);
    append(menu, ID_SETTINGS, "Settings", false);
    append(menu, ID_EXIT, "Exit", false);
    let mut point = POINT::default();
    let _ = unsafe { GetCursorPos(&mut point) };
    let _ = unsafe { SetForegroundWindow(hwnd) };
    let flags = TPM_RETURNCMD.0 | TPM_NONOTIFY.0 | TPM_RIGHTBUTTON.0;
    let selected = unsafe { TrackPopupMenuEx(menu, flags, point.x, point.y, hwnd, None) };
    let _ = unsafe { DestroyMenu(menu) };
    command_from_menu_id(selected.0)
}

fn append(
    menu: windows::Win32::UI::WindowsAndMessaging::HMENU,
    id: usize,
    label: &str,
    disabled: bool,
) {
    let wide = label
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let flags = if disabled {
        MF_STRING | MF_GRAYED
    } else {
        MF_STRING
    };
    let _ = unsafe { AppendMenuW(menu, flags, id, PCWSTR(wide.as_ptr())) };
}

#[cfg(test)]
mod tests {
    use super::{command_from_menu_id, TrayCommand};

    #[test]
    fn menu_ids_map_to_tray_commands() {
        // Arrange
        let ids = [1, 2, 3, 4, 5, 6, 0];

        // Act
        let mapped: Vec<_> = ids.into_iter().map(command_from_menu_id).collect();

        // Assert
        assert_eq!(
            mapped,
            vec![
                Some(TrayCommand::Show),
                Some(TrayCommand::StartRecording),
                Some(TrayCommand::StopRecording),
                Some(TrayCommand::OpenRecordingsFolder),
                Some(TrayCommand::Settings),
                Some(TrayCommand::Exit),
                None,
            ]
        );
    }
}
