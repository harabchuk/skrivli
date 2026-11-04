//! Open the recordings directory. This creates the folder when it is missing
//! and does not list its contents.

use std::path::Path;

use windows::core::PCWSTR;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

use crate::common::error::AppError;

pub(crate) fn open_recordings_folder(path: &Path) -> Result<(), AppError> {
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

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn wide_path(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}
