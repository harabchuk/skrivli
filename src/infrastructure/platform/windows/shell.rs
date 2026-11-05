//! Show/hide the main window, folder picker, and Explorer reveal.

use std::path::{Path, PathBuf};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Shell::{
    FileOpenDialog, IFileOpenDialog, ShellExecuteW, FILEOPENDIALOGOPTIONS, FOS_FORCEFILESYSTEM,
    FOS_PATHMUSTEXIST, FOS_PICKFOLDERS, SIGDN_FILESYSPATH,
};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, IsIconic, SetForegroundWindow, ShowWindow, SW_HIDE, SW_SHOW, SW_SHOWNORMAL,
};

use crate::common::error::AppError;
use crate::services::ports::DesktopShell;
use crate::services::recording::ensure_writable;

/// gpui-pre 0.3.11's Windows hide is a no-op (D-01). Find the existing window.
const WINDOW_CLASS: PCWSTR = w!("Zed::Window");
const WINDOW_TITLE: PCWSTR = w!("Skrivli");

pub(super) struct WindowsShell;

impl DesktopShell for WindowsShell {
    fn set_window_visible(&self, visible: bool) -> bool {
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

    fn window_is_minimized(&self) -> bool {
        let Some(hwnd) = find_main_window() else {
            return false;
        };
        // SAFETY: the handle is a top-level window in this process, when found.
        unsafe { IsIconic(hwnd).as_bool() }
    }

    fn pick_recordings_folder(&self) -> Result<Option<PathBuf>, AppError> {
        pick_recordings_folder()
    }

    fn open_recordings_folder(&self, path: &Path) -> Result<(), AppError> {
        open_recordings_folder(path)
    }
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

fn main_window_hwnd() -> Option<HWND> {
    find_main_window()
}

fn pick_recordings_folder() -> Result<Option<PathBuf>, AppError> {
    let _apartment = sta_guard()?;
    let dialog: IFileOpenDialog =
        unsafe { CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER) }
            .map_err(|error| AppError::message("recordings folder", error.to_string()))?;
    unsafe {
        let current = dialog
            .GetOptions()
            .map_err(|error| AppError::message("recordings folder", error.to_string()))?;
        let flags = FILEOPENDIALOGOPTIONS(
            current.0 | FOS_PICKFOLDERS.0 | FOS_FORCEFILESYSTEM.0 | FOS_PATHMUSTEXIST.0,
        );
        dialog
            .SetOptions(flags)
            .map_err(|error| AppError::message("recordings folder", error.to_string()))?;
        dialog
            .SetTitle(PCWSTR(wide("Choose a recordings folder").as_ptr()))
            .map_err(|error| AppError::message("recordings folder", error.to_string()))?;
    }
    let owner = main_window_hwnd().unwrap_or_default();
    if let Err(error) = unsafe { dialog.Show(owner) } {
        if cancelled(&error) {
            return Ok(None);
        }
        return Err(AppError::message("recordings folder", error.to_string()));
    }
    let item = unsafe { dialog.GetResult() }
        .map_err(|error| AppError::message("recordings folder", error.to_string()))?;
    let name = unsafe { item.GetDisplayName(SIGDN_FILESYSPATH) }
        .map_err(|error| AppError::message("recordings folder", error.to_string()))?;
    let path = PathBuf::from(take_pwstr(name));
    ensure_writable(&path)?;
    Ok(Some(path))
}

fn open_recordings_folder(path: &Path) -> Result<(), AppError> {
    std::fs::create_dir_all(path)
        .map_err(|source| AppError::io("open recordings folder", source))?;
    let operation = wide("explore");
    let target = wide_path(path);
    // SAFETY: both buffers are NUL-terminated and live for the call.
    let instance = unsafe {
        ShellExecuteW(
            None,
            PCWSTR(operation.as_ptr()),
            PCWSTR(target.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    if (instance.0 as isize) <= 32 {
        return Err(AppError::message(
            "open recordings folder",
            format!("ShellExecuteW returned {}", instance.0 as isize),
        ));
    }
    Ok(())
}

struct StaGuard {
    uninit: bool,
}

fn sta_guard() -> Result<StaGuard, AppError> {
    let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    if hr.0 == 0 {
        Ok(StaGuard { uninit: true })
    } else if hr.0 == 1 || hr.0 == 0x8001_0106_u32 as i32 {
        Ok(StaGuard { uninit: false })
    } else {
        Err(AppError::message("recordings folder", hr.0.to_string()))
    }
}

impl Drop for StaGuard {
    fn drop(&mut self) {
        if self.uninit {
            unsafe { CoUninitialize() };
        }
    }
}

fn cancelled(error: &windows::core::Error) -> bool {
    error.code().0 == 0x8007_04C7_u32 as i32
}

fn take_pwstr(value: windows::core::PWSTR) -> String {
    let text = if value.is_null() {
        String::new()
    } else {
        unsafe { value.to_string() }.unwrap_or_default()
    };
    if !value.is_null() {
        unsafe { CoTaskMemFree(Some(value.0.cast())) };
    }
    text
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn wide_path(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}
