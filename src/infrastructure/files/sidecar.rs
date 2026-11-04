//! Versioned recording sidecar. The write is a temp file, `sync_all`, then replace.

use std::fs::OpenOptions;
use std::io::Write;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use serde::Serialize;
use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::{ReplaceFileW, REPLACEFILE_IGNORE_MERGE_ERRORS};

use crate::common::error::AppError;

pub(crate) const SIDECAR_SCHEMA: u32 = 2;

#[derive(Debug, Serialize)]
struct FormatBody {
    sample_rate: u32,
    channels: u16,
    encoding: &'static str,
}

#[derive(Debug, Serialize)]
struct SidecarBody {
    schema_version: u32,
    id: u64,
    started_at_unix_ms: u64,
    ended_at_unix_ms: u64,
    duration_ms: u64,
    microphone_id: String,
    system_audio_id: String,
    format: FormatBody,
    problems: Vec<String>,
}

pub(crate) struct Sidecar {
    pub(crate) id: u64,
    pub(crate) started_at_unix_ms: u64,
    pub(crate) ended_at_unix_ms: u64,
    pub(crate) duration_ms: u64,
    pub(crate) microphone_id: String,
    pub(crate) system_audio_id: String,
    pub(crate) problems: Vec<String>,
}

pub(crate) fn write_sidecar(wav_path: &Path, sidecar: &Sidecar) -> Result<PathBuf, AppError> {
    let path = sidecar_path(wav_path)?;
    let body = SidecarBody {
        schema_version: SIDECAR_SCHEMA,
        id: sidecar.id,
        started_at_unix_ms: sidecar.started_at_unix_ms,
        ended_at_unix_ms: sidecar.ended_at_unix_ms,
        duration_ms: sidecar.duration_ms,
        microphone_id: sidecar.microphone_id.clone(),
        system_audio_id: sidecar.system_audio_id.clone(),
        format: FormatBody {
            sample_rate: 48_000,
            channels: 2,
            encoding: "pcm16",
        },
        problems: sidecar.problems.clone(),
    };
    let bytes = serde_json::to_vec_pretty(&body)
        .map_err(|error| AppError::message("save recording", error.to_string()))?;
    let temp = temp_sibling(&path);
    {
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&temp)
            .map_err(|source| AppError::io("save recording", source))?;
        file.write_all(&bytes)
            .map_err(|source| AppError::io("save recording", source))?;
        file.sync_all()
            .map_err(|source| AppError::io("save recording", source))?;
    }
    replace_file(&temp, &path)?;
    Ok(path)
}

fn sidecar_path(wav_path: &Path) -> Result<PathBuf, AppError> {
    let name = wav_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| AppError::message("save recording", "recording name is not unicode"))?;
    let stem = name.strip_suffix(".wav").unwrap_or(name);
    let directory = wav_path.parent().unwrap_or(Path::new("."));
    Ok(directory.join(format!("{stem}.json")))
}

fn temp_sibling(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_else(|| std::ffi::OsString::from("recording.json"));
    name.push(".tmp");
    path.with_file_name(name)
}

fn replace_file(replacement: &Path, replaced: &Path) -> Result<(), AppError> {
    if !replaced.exists() {
        return std::fs::rename(replacement, replaced)
            .map_err(|source| AppError::io("save recording", source));
    }
    let replacement_wide = wide_path(replacement);
    let replaced_wide = wide_path(replaced);
    // SAFETY: both buffers are NUL-terminated files in the same directory.
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
    result.map_err(|error| AppError::message("save recording", error.to_string()))
}

fn wide_path(path: &Path) -> Vec<u16> {
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::{write_sidecar, Sidecar};

    #[test]
    fn sidecar_round_trip_omits_samples() {
        // Arrange
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("skrivli-side-{id}"));
        fs::create_dir_all(&dir).expect("dir");
        let wav = dir.join("Skrivli_2026-11-04_13-01-02.wav");
        let sidecar = Sidecar {
            id: 7,
            started_at_unix_ms: 10,
            ended_at_unix_ms: 20,
            duration_ms: 10,
            microphone_id: "{mic}".to_string(),
            system_audio_id: "{system}".to_string(),
            problems: vec!["overrun".to_string()],
        };

        // Act
        let path = write_sidecar(&wav, &sidecar).expect("write");
        let text = fs::read_to_string(&path).expect("read");

        // Assert
        assert!(text.contains("\"schema_version\": 2"));
        assert!(text.contains("{system}"));
        assert!(text.contains("\"id\": 7"));
        assert!(text.contains("pcm16"));
        assert!(text.contains("overrun"));
        assert!(!dir.join("Skrivli_2026-11-04_13-01-02.json.tmp").exists());
        let _ = fs::remove_dir_all(dir);
    }
}
