//! Atomic replace via `ReplaceFileW`.

use std::path::Path;

use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::{ReplaceFileW, REPLACEFILE_IGNORE_MERGE_ERRORS};

use crate::common::error::AppError;

/// Replace `replaced` with `replacement`. When the destination is missing, rename.
/// On success the temp name is gone (ReplaceFileW removes it; rename moves it).
pub(crate) fn replace_file(
    replacement: &Path,
    replaced: &Path,
    context: &'static str,
) -> Result<(), AppError> {
    if !replaced.exists() {
        return std::fs::rename(replacement, replaced)
            .map_err(|source| AppError::io(context, source));
    }
    let replacement_wide = wide_path(replacement);
    let replaced_wide = wide_path(replaced);
    // SAFETY: both buffers are NUL-terminated and point at files in the same
    // directory. The temp file is closed. ReplaceFileW moves it onto `replaced`.
    let result = unsafe {
        ReplaceFileW(
            PCWSTR(replaced_wide.as_ptr()),
            PCWSTR(replacement_wide.as_ptr()),
            PCWSTR::null(),
            REPLACEFILE_IGNORE_MERGE_ERRORS,
            None,
            None,
        )
    };
    result.map_err(|error| AppError::message(context, error.to_string()))
}

fn wide_path(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}
