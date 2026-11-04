//! Native folder picker. It runs from a user click, not from `Render::render`
//! and not on the capture thread.

use std::path::PathBuf;

use windows::core::PCWSTR;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Shell::{
    FileOpenDialog, IFileOpenDialog, FILEOPENDIALOGOPTIONS, FOS_FORCEFILESYSTEM, FOS_PATHMUSTEXIST,
    FOS_PICKFOLDERS, SIGDN_FILESYSPATH,
};

use crate::common::error::AppError;
use crate::infrastructure::audio::devices::take_pwstr;
use crate::services::recording::ensure_writable;

use super::shell_window::main_window_hwnd;

pub(crate) fn pick_recordings_folder() -> Result<Option<PathBuf>, AppError> {
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
            unsafe { windows::Win32::System::Com::CoUninitialize() };
        }
    }
}

fn cancelled(error: &windows::core::Error) -> bool {
    error.code().0 == 0x8007_04C7_u32 as i32
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}
