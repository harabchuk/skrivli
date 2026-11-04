//! Versioned recording sidecar (`<basename>.json`).
//!
//! Writes use a temp sibling, `sync_all`, then replace. Corrupt or missing
//! sidecars are never deleted automatically — the library presents them as
//! malformed/missing metadata and keeps the audio entry.

use std::fs::OpenOptions;
use std::io::Write;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::{ReplaceFileW, REPLACEFILE_IGNORE_MERGE_ERRORS};

use crate::common::error::AppError;
use crate::domain::ids::RecordingId;
use crate::domain::recording::{RecordingFormat, SidecarState, TranscriptStatus};

pub(crate) const SIDECAR_SCHEMA: u32 = 3;
const LEGACY_SCHEMA: u32 = 2;

#[derive(Debug, Serialize, Deserialize)]
struct FormatBody {
    sample_rate: u32,
    channels: u16,
    encoding: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct TranscriptionBody {
    #[serde(default)]
    status: String,
    #[serde(default)]
    progress_percent: Option<u8>,
    #[serde(default)]
    model: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SidecarBody {
    schema_version: u32,
    id: u64,
    started_at_unix_ms: u64,
    ended_at_unix_ms: u64,
    duration_ms: u64,
    microphone_id: String,
    system_audio_id: String,
    format: FormatBody,
    #[serde(default)]
    problems: Vec<String>,
    #[serde(default)]
    transcription: TranscriptionBody,
}

/// Values written when a recording session finalizes.
pub(crate) struct Sidecar {
    pub(crate) id: u64,
    pub(crate) started_at_unix_ms: u64,
    pub(crate) ended_at_unix_ms: u64,
    pub(crate) duration_ms: u64,
    pub(crate) microphone_id: String,
    pub(crate) system_audio_id: String,
    pub(crate) problems: Vec<String>,
}

/// Loaded sidecar for the library catalog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LoadedSidecar {
    pub(crate) id: RecordingId,
    pub(crate) started_at_unix_ms: u64,
    pub(crate) ended_at_unix_ms: u64,
    pub(crate) duration_ms: u64,
    pub(crate) microphone_id: String,
    pub(crate) system_audio_id: String,
    pub(crate) format: RecordingFormat,
    pub(crate) problems: Vec<String>,
    pub(crate) transcript: TranscriptStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SidecarLoad {
    Loaded(LoadedSidecar),
    Missing,
    Malformed,
}

pub(crate) fn sidecar_path_for(audio_path: &Path) -> Result<PathBuf, AppError> {
    let stem = audio_stem(audio_path)?;
    let directory = audio_path.parent().unwrap_or(Path::new("."));
    Ok(directory.join(format!("{stem}.json")))
}

pub(crate) fn transcript_path_for(audio_path: &Path) -> Result<PathBuf, AppError> {
    let stem = audio_stem(audio_path)?;
    let directory = audio_path.parent().unwrap_or(Path::new("."));
    Ok(directory.join(format!("{stem}.txt")))
}

pub(crate) fn write_sidecar(wav_path: &Path, sidecar: &Sidecar) -> Result<PathBuf, AppError> {
    let path = sidecar_path_for(wav_path)?;
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
            encoding: "pcm16".to_string(),
        },
        problems: sidecar.problems.clone(),
        transcription: TranscriptionBody::default(),
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

pub(crate) fn load_sidecar(audio_path: &Path) -> SidecarLoad {
    let path = match sidecar_path_for(audio_path) {
        Ok(path) => path,
        Err(_) => return SidecarLoad::Malformed,
    };
    if !path.is_file() {
        return SidecarLoad::Missing;
    }
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(_) => return SidecarLoad::Malformed,
    };
    parse_sidecar_text(&text).map_or(SidecarLoad::Malformed, SidecarLoad::Loaded)
}

pub(crate) fn sidecar_state(load: &SidecarLoad) -> SidecarState {
    match load {
        SidecarLoad::Loaded(_) => SidecarState::Present,
        SidecarLoad::Missing => SidecarState::Missing,
        SidecarLoad::Malformed => SidecarState::Malformed,
    }
}

fn parse_sidecar_text(text: &str) -> Option<LoadedSidecar> {
    let body: SidecarBody = serde_json::from_str(text).ok()?;
    if body.schema_version != SIDECAR_SCHEMA && body.schema_version != LEGACY_SCHEMA {
        return None;
    }
    if body.id == 0 {
        return None;
    }
    let pcm16 = body.format.encoding.eq_ignore_ascii_case("pcm16");
    let format = RecordingFormat::new(body.format.sample_rate, body.format.channels, pcm16)?;
    let transcript = parse_transcript(&body.transcription);
    Some(LoadedSidecar {
        id: RecordingId::from_raw(body.id),
        started_at_unix_ms: body.started_at_unix_ms,
        ended_at_unix_ms: body.ended_at_unix_ms,
        duration_ms: body.duration_ms,
        microphone_id: body.microphone_id,
        system_audio_id: body.system_audio_id,
        format,
        problems: body.problems,
        transcript,
    })
}

fn parse_transcript(body: &TranscriptionBody) -> TranscriptStatus {
    match body.status.to_ascii_lowercase().as_str() {
        "" | "none" | "—" | "-" => TranscriptStatus::None,
        "queued" => TranscriptStatus::Queued,
        "ready" => TranscriptStatus::Ready,
        "failed" => TranscriptStatus::Failed,
        "progress" => TranscriptStatus::Progress(body.progress_percent.unwrap_or(0).min(100)),
        other if other.ends_with('%') => {
            let digits = other.trim_end_matches('%');
            digits
                .parse::<u8>()
                .map(TranscriptStatus::Progress)
                .unwrap_or(TranscriptStatus::None)
        }
        _ => TranscriptStatus::None,
    }
}

fn audio_stem(audio_path: &Path) -> Result<String, AppError> {
    let name = audio_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| AppError::message("library", "recording name is not unicode"))?;
    let stem = name
        .strip_suffix(".wav")
        .or_else(|| name.strip_suffix(".m4a"))
        .unwrap_or(name);
    Ok(stem.to_string())
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

    use super::{load_sidecar, write_sidecar, Sidecar, SidecarLoad, SIDECAR_SCHEMA};
    use crate::domain::recording::TranscriptStatus;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("skrivli-side-{tag}-{id}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("dir");
        dir
    }

    #[test]
    fn sidecar_round_trip_omits_samples() {
        // Arrange
        let dir = temp_dir("rt");
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
        let loaded = load_sidecar(&wav);

        // Assert
        assert!(text.contains(&format!("\"schema_version\": {SIDECAR_SCHEMA}")));
        assert!(text.contains("{system}"));
        assert!(text.contains("\"id\": 7"));
        assert!(text.contains("pcm16"));
        assert!(text.contains("overrun"));
        assert!(!dir.join("Skrivli_2026-11-04_13-01-02.json.tmp").exists());
        match loaded {
            SidecarLoad::Loaded(doc) => {
                assert_eq!(doc.id.raw(), 7);
                assert_eq!(doc.transcript, TranscriptStatus::None);
            }
            other => panic!("expected loaded, got {other:?}"),
        }
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn load_tolerates_schema_v2_and_marks_corrupt() {
        // Arrange
        let dir = temp_dir("v2");
        let wav = dir.join("clip.wav");
        let json = dir.join("clip.json");
        fs::write(
            &json,
            r#"{
              "schema_version": 2,
              "id": 9,
              "started_at_unix_ms": 1,
              "ended_at_unix_ms": 2,
              "duration_ms": 1,
              "microphone_id": "m",
              "system_audio_id": "s",
              "format": {"sample_rate": 48000, "channels": 2, "encoding": "pcm16"},
              "problems": []
            }"#,
        )
        .expect("v2");
        let bad = dir.join("bad.wav");
        fs::write(dir.join("bad.json"), "{not-json").expect("bad");

        // Act
        let v2 = load_sidecar(&wav);
        let malformed = load_sidecar(&bad);
        let missing = load_sidecar(&dir.join("gone.wav"));

        // Assert
        match v2 {
            SidecarLoad::Loaded(doc) => assert_eq!(doc.id.raw(), 9),
            other => panic!("v2 {other:?}"),
        }
        assert_eq!(malformed, SidecarLoad::Malformed);
        assert_eq!(missing, SidecarLoad::Missing);
        let _ = fs::remove_dir_all(dir);
    }
}
