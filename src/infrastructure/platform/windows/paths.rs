//! User directories from the Windows environment.

use crate::common::error::AppError;
use crate::common::paths::AppPaths;

pub(crate) fn user_paths() -> Result<AppPaths, AppError> {
    let roaming = std::env::var_os("APPDATA")
        .ok_or_else(|| AppError::message("app paths", "APPDATA is not set"))?;
    let local = std::env::var_os("LOCALAPPDATA")
        .ok_or_else(|| AppError::message("app paths", "LOCALAPPDATA is not set"))?;
    let home = std::env::var_os("USERPROFILE")
        .ok_or_else(|| AppError::message("app paths", "USERPROFILE is not set"))?;
    Ok(AppPaths::from_bases(roaming, local, home))
}
