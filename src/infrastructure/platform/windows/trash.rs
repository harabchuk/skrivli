//! Recycle Bin via `SHFileOperationW`.

use std::path::{Path, PathBuf};

use windows::core::PCWSTR;
use windows::Win32::UI::Shell::{
    SHFileOperationW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FO_DELETE,
    SHFILEOPSTRUCTW,
};

use crate::common::error::AppError;
use crate::services::ports::Trash;

pub(super) struct WindowsTrash;

impl Trash for WindowsTrash {
    fn discard(&self, paths: &[PathBuf]) -> Result<(), AppError> {
        send_to_recycle_bin(paths)
    }
}

/// Move one or more existing files to the Recycle Bin. Missing paths are skipped.
fn send_to_recycle_bin(paths: &[impl AsRef<Path>]) -> Result<(), AppError> {
    let existing: Vec<&Path> = paths
        .iter()
        .map(AsRef::as_ref)
        .filter(|path| path.exists())
        .collect();
    if existing.is_empty() {
        return Ok(());
    }
    let mut from = Vec::new();
    for path in &existing {
        from.extend(wide_path(path));
    }
    from.push(0);

    let mut file_op = SHFILEOPSTRUCTW {
        hwnd: windows::Win32::Foundation::HWND::default(),
        wFunc: FO_DELETE,
        pFrom: PCWSTR(from.as_ptr()),
        pTo: PCWSTR::null(),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI).0 as u16,
        fAnyOperationsAborted: false.into(),
        hNameMappings: std::ptr::null_mut(),
        lpszProgressTitle: PCWSTR::null(),
    };

    // SAFETY: `from` is a double-NUL-terminated multi-string for the call.
    let code = unsafe { SHFileOperationW(&mut file_op) };
    if code != 0 || file_op.fAnyOperationsAborted.as_bool() {
        return Err(AppError::message(
            "library delete",
            format!("SHFileOperationW returned {code}"),
        ));
    }
    Ok(())
}

fn wide_path(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::send_to_recycle_bin;

    #[test]
    fn recycle_accepts_missing_paths() {
        // Arrange
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let missing = std::env::temp_dir().join(format!("skrivli-missing-{id}.wav"));

        // Act
        let result = send_to_recycle_bin(&[missing.as_path()]);

        // Assert
        assert!(result.is_ok());
    }

    #[test]
    fn recycle_moves_a_temp_file() {
        // Arrange
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("skrivli-recycle-{id}.txt"));
        fs::write(&path, b"bye").expect("write");

        // Act
        let result = send_to_recycle_bin(&[path.as_path()]);

        // Assert
        assert!(result.is_ok(), "{result:?}");
        assert!(!path.exists());
    }
}
