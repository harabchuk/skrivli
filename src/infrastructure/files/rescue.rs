//! Bounded rescue of a readable partial recording.
//!
//! When the configured recordings folder becomes unusable mid-session, the
//! session may copy a still-readable `*.wav.partial` into
//! `%LOCALAPPDATA%\Skrivli\rescue\`. Rescue runs only when the source bytes can
//! still be opened and read. It never claims success if the original is gone.
//!
//! Cleanup policy: never auto-delete unknown `*.wav.partial` or `*.recovered.wav`
//! files. Startup recovery may rename a safe partial to `*.recovered.wav`.
//! Abandon (delete) is only for a failed start before any usable audio was kept.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::common::error::AppError;

use super::wav::{MAX_SAFE_DATA_BYTES, WAV_HEADER_BYTES};

/// Largest rescue payload: one canonical RIFF WAV plus a small margin.
const MAX_RESCUE_BYTES: u64 = MAX_SAFE_DATA_BYTES.saturating_add(WAV_HEADER_BYTES);

/// Copy `partial` into `rescue_dir` when the source is readable.
///
/// Returns the destination path. Fails without writing a claim of recovery when
/// the source cannot be read or exceeds the size bound.
pub(crate) fn rescue_partial(partial: &Path, rescue_dir: &Path) -> Result<PathBuf, AppError> {
    let meta = fs::metadata(partial).map_err(|source| AppError::io("rescue recording", source))?;
    let len = meta.len();
    if len == 0 || len > MAX_RESCUE_BYTES {
        return Err(AppError::message(
            "rescue recording",
            "partial size is outside the rescue bound",
        ));
    }
    fs::create_dir_all(rescue_dir).map_err(|source| AppError::io("rescue recording", source))?;
    let destination = unique_rescue_path(rescue_dir, partial)?;
    let source = File::open(partial).map_err(|source| AppError::io("rescue recording", source))?;
    let mut buffer = Vec::new();
    source
        .take(MAX_RESCUE_BYTES)
        .read_to_end(&mut buffer)
        .map_err(|source| AppError::io("rescue recording", source))?;
    if buffer.is_empty() {
        return Err(AppError::message(
            "rescue recording",
            "partial could not be read",
        ));
    }
    {
        let mut out = File::create(&destination)
            .map_err(|source| AppError::io("rescue recording", source))?;
        out.write_all(&buffer)
            .map_err(|source| AppError::io("rescue recording", source))?;
        out.sync_all()
            .map_err(|source| AppError::io("rescue recording", source))?;
    }
    Ok(destination)
}

fn unique_rescue_path(rescue_dir: &Path, partial: &Path) -> Result<PathBuf, AppError> {
    let name = partial
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| AppError::message("rescue recording", "partial name is not unicode"))?;
    let stem = name
        .strip_suffix(".wav.partial")
        .or_else(|| name.strip_suffix(".partial"))
        .unwrap_or(name);
    for suffix in 0..1000_u32 {
        let file_name = if suffix == 0 {
            format!("{stem}.rescued.wav")
        } else {
            format!("{stem}-{suffix}.rescued.wav")
        };
        let candidate = rescue_dir.join(file_name);
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(AppError::message(
        "rescue recording",
        "rescue name collisions were exhausted",
    ))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::{rescue_partial, MAX_RESCUE_BYTES};

    fn temp_dir(label: &str) -> std::path::PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "skrivli-rescue-{label}-{}-{id}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("dir");
        dir
    }

    #[test]
    fn rescue_copies_readable_partial_and_skips_missing() {
        // Arrange
        let src_dir = temp_dir("src");
        let rescue_dir = temp_dir("dst");
        let partial = src_dir.join("Skrivli_2026-11-04_13-01-02.wav.partial");
        fs::write(&partial, b"RIFF....WAVEfmt ").expect("partial");
        let missing = src_dir.join("missing.wav.partial");

        // Act
        let saved = rescue_partial(&partial, &rescue_dir).expect("rescue");
        let absent = rescue_partial(&missing, &rescue_dir);

        // Assert
        assert!(saved.ends_with("Skrivli_2026-11-04_13-01-02.rescued.wav"));
        assert_eq!(fs::read(&saved).expect("read"), b"RIFF....WAVEfmt ");
        assert!(absent.is_err());
        let _ = fs::remove_dir_all(src_dir);
        let _ = fs::remove_dir_all(rescue_dir);
    }

    #[test]
    fn rescue_rejects_oversized_partial() {
        // Arrange
        let src_dir = temp_dir("big");
        let rescue_dir = temp_dir("big-dst");
        let partial = src_dir.join("huge.wav.partial");
        let file = fs::File::create(&partial).expect("create");
        file.set_len(MAX_RESCUE_BYTES.saturating_add(1))
            .expect("len");
        drop(file);

        // Act
        let result = rescue_partial(&partial, &rescue_dir);

        // Assert
        assert!(result.is_err());
        assert!(rescue_dir
            .read_dir()
            .expect("dir")
            .flatten()
            .next()
            .is_none());
        let _ = fs::remove_dir_all(src_dir);
        let _ = fs::remove_dir_all(rescue_dir);
    }
}
