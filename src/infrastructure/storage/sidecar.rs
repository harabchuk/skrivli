//! Versioned recording sidecar (`<basename>.json`).
//!
//! Writes use a temp sibling, `sync_all`, then replace. Corrupt or missing
//! sidecars are never deleted automatically — the library presents them as
//! malformed/missing metadata and keeps the audio entry.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::common::error::AppError;
use crate::domain::ids::RecordingId;
use crate::domain::recording::{RecordingFormat, SidecarState, TranscriptStatus};
use crate::infrastructure::platform;

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
    /// The stored status was `"cancelled"` (which `transcript` reports as Failed).
    pub(crate) transcript_cancelled: bool,
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

/// Atomically write UTF-8 transcript text beside the audio file.
///
/// Uses a temp sibling, `sync_all`, then [`platform::replace_file`]. A failed
/// write leaves any previous `.txt` and the FLAC bytes unchanged.
pub(crate) fn write_transcript_text(audio_path: &Path, text: &str) -> Result<PathBuf, AppError> {
    let path = transcript_path_for(audio_path)?;
    let temp = temp_sibling(&path);
    {
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&temp)
            .map_err(|source| AppError::io("save transcript", source))?;
        file.write_all(text.as_bytes())
            .map_err(|source| AppError::io("save transcript", source))?;
        file.sync_all()
            .map_err(|source| AppError::io("save transcript", source))?;
    }
    platform::replace_file(&temp, &path, "save transcript")?;
    Ok(path)
}

pub(crate) fn write_sidecar(audio_path: &Path, sidecar: &Sidecar) -> Result<PathBuf, AppError> {
    let path = sidecar_path_for(audio_path)?;
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
    write_sidecar_body(&path, &body)?;
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
        transcript_cancelled: body.transcription.status.eq_ignore_ascii_case("cancelled"),
    })
}

fn parse_transcript(body: &TranscriptionBody) -> TranscriptStatus {
    match body.status.to_ascii_lowercase().as_str() {
        "" | "none" | "—" | "-" => TranscriptStatus::None,
        "queued" => TranscriptStatus::Queued,
        "preparing" => TranscriptStatus::Progress(0),
        "ready" => TranscriptStatus::Ready,
        "failed" | "cancelled" | "interrupted" => TranscriptStatus::Failed,
        "progress" | "transcribing" => {
            TranscriptStatus::Progress(body.progress_percent.unwrap_or(0).min(100))
        }
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

/// Values written into the sidecar `transcription` body by the worker.
/// Variants beyond those exercised in unit tests are for the M6.3 host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum TranscriptionWrite {
    Queued,
    Preparing,
    Progress(u8),
    Ready,
    Failed,
    Cancelled,
}

impl TranscriptionWrite {
    fn to_body(self) -> TranscriptionBody {
        match self {
            Self::Queued => TranscriptionBody {
                status: "queued".into(),
                progress_percent: None,
                model: None,
            },
            Self::Preparing => TranscriptionBody {
                status: "preparing".into(),
                progress_percent: Some(0),
                model: None,
            },
            Self::Progress(percent) => TranscriptionBody {
                status: "progress".into(),
                progress_percent: Some(percent.min(100)),
                model: None,
            },
            Self::Ready => TranscriptionBody {
                status: "ready".into(),
                progress_percent: Some(100),
                model: None,
            },
            Self::Failed => TranscriptionBody {
                status: "failed".into(),
                progress_percent: None,
                model: None,
            },
            Self::Cancelled => TranscriptionBody {
                status: "cancelled".into(),
                progress_percent: None,
                model: None,
            },
        }
    }

    #[allow(dead_code)] // Mapped by the M6.3 transcription host.
    pub(crate) fn to_status(self) -> TranscriptStatus {
        match self {
            Self::Queued => TranscriptStatus::Queued,
            Self::Preparing => TranscriptStatus::Progress(0),
            Self::Progress(percent) => TranscriptStatus::Progress(percent.min(100)),
            Self::Ready => TranscriptStatus::Ready,
            Self::Failed | Self::Cancelled => TranscriptStatus::Failed,
        }
    }
}

/// Whether a stored transcript status means a job was mid-flight when the process exited.
pub(crate) fn transcript_is_in_progress(status: TranscriptStatus) -> bool {
    matches!(
        status,
        TranscriptStatus::Queued | TranscriptStatus::Progress(_)
    )
}

/// Persist a transcription status (and optional model id) into the sidecar beside `audio_path`.
///
/// Leaves audio bytes untouched. Preserves unrelated sidecar fields. A missing or
/// malformed sidecar returns an error without creating a new one.
pub(crate) fn update_transcription(
    audio_path: &Path,
    status: TranscriptionWrite,
    model: Option<&str>,
) -> Result<(), AppError> {
    let path = sidecar_path_for(audio_path)?;
    let text =
        std::fs::read_to_string(&path).map_err(|source| AppError::io("save recording", source))?;
    let mut body: SidecarBody = serde_json::from_str(&text)
        .map_err(|error| AppError::message("save recording", error.to_string()))?;
    let mut next = status.to_body();
    if let Some(model) = model {
        next.model = Some(model.to_string());
    } else if body
        .transcription
        .model
        .as_ref()
        .is_some_and(|value| !value.is_empty())
    {
        next.model = body.transcription.model.take();
    }
    body.transcription = next;
    body.schema_version = SIDECAR_SCHEMA;
    write_sidecar_body(&path, &body)
}

/// On launch / library scan: in-progress jobs become Failed (retryable). Does not resume.
pub(crate) fn interrupt_in_progress_transcription(audio_path: &Path) -> bool {
    match load_sidecar(audio_path) {
        SidecarLoad::Loaded(doc) if transcript_is_in_progress(doc.transcript) => {
            update_transcription(audio_path, TranscriptionWrite::Failed, None).is_ok()
        }
        _ => false,
    }
}

fn write_sidecar_body(path: &Path, body: &SidecarBody) -> Result<(), AppError> {
    let bytes = serde_json::to_vec_pretty(body)
        .map_err(|error| AppError::message("save recording", error.to_string()))?;
    let temp = temp_sibling(path);
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
    platform::replace_file(&temp, path, "save recording")?;
    Ok(())
}

fn audio_stem(audio_path: &Path) -> Result<String, AppError> {
    let name = audio_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| AppError::message("library", "recording name is not unicode"))?;
    let stem = name.strip_suffix(".flac").unwrap_or(name);
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::{
        interrupt_in_progress_transcription, load_sidecar, update_transcription, write_sidecar,
        write_transcript_text, Sidecar, SidecarLoad, TranscriptionWrite, SIDECAR_SCHEMA,
    };
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
        let audio_file = dir.join("Skrivli_2026-11-04_13-01-02.flac");
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
        let path = write_sidecar(&audio_file, &sidecar).expect("write");
        let text = fs::read_to_string(&path).expect("read");
        let loaded = load_sidecar(&audio_file);

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
        let audio_file = dir.join("clip.flac");
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
        let bad = dir.join("bad.flac");
        fs::write(dir.join("bad.json"), "{not-json").expect("bad");

        // Act
        let v2 = load_sidecar(&audio_file);
        let malformed = load_sidecar(&bad);
        let missing = load_sidecar(&dir.join("gone.flac"));

        // Assert
        match v2 {
            SidecarLoad::Loaded(doc) => assert_eq!(doc.id.raw(), 9),
            other => panic!("v2 {other:?}"),
        }
        assert_eq!(malformed, SidecarLoad::Malformed);
        assert_eq!(missing, SidecarLoad::Missing);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn update_transcription_persists_progress_and_leaves_flac() {
        // Arrange
        let dir = temp_dir("tx");
        let audio_file = dir.join("meeting.flac");
        fs::write(&audio_file, b"fLaC").expect("audio_file");
        let wav_before = fs::read(&audio_file).expect("before");
        write_sidecar(
            &audio_file,
            &Sidecar {
                id: 3,
                started_at_unix_ms: 1,
                ended_at_unix_ms: 2,
                duration_ms: 1,
                microphone_id: "m".into(),
                system_audio_id: "s".into(),
                problems: Vec::new(),
            },
        )
        .expect("sidecar");

        // Act
        update_transcription(
            &audio_file,
            TranscriptionWrite::Progress(42),
            Some("base-q5_1"),
        )
        .expect("update");
        let loaded = load_sidecar(&audio_file);

        // Assert
        assert_eq!(fs::read(&audio_file).expect("after"), wav_before);
        match loaded {
            SidecarLoad::Loaded(doc) => {
                assert_eq!(doc.transcript, TranscriptStatus::Progress(42));
            }
            other => panic!("expected loaded, got {other:?}"),
        }
        let text = fs::read_to_string(dir.join("meeting.json")).expect("json");
        assert!(text.contains("\"model\": \"base-q5_1\""));
        assert!(text.contains("42"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn interrupt_marks_queued_as_failed_without_resume() {
        // Arrange
        let dir = temp_dir("int");
        let audio_file = dir.join("clip.flac");
        fs::write(&audio_file, b"fLaC").expect("audio_file");
        write_sidecar(
            &audio_file,
            &Sidecar {
                id: 4,
                started_at_unix_ms: 1,
                ended_at_unix_ms: 2,
                duration_ms: 1,
                microphone_id: "m".into(),
                system_audio_id: "s".into(),
                problems: Vec::new(),
            },
        )
        .expect("sidecar");
        update_transcription(&audio_file, TranscriptionWrite::Queued, None).expect("queued");
        write_transcript_text(&audio_file, "keep me\n").expect("txt");

        // Act
        let changed = interrupt_in_progress_transcription(&audio_file);
        let again = interrupt_in_progress_transcription(&audio_file);
        let loaded = load_sidecar(&audio_file);

        // Assert
        assert!(changed);
        assert!(!again);
        match loaded {
            SidecarLoad::Loaded(doc) => assert_eq!(doc.transcript, TranscriptStatus::Failed),
            other => panic!("expected loaded, got {other:?}"),
        }
        assert_eq!(
            fs::read_to_string(dir.join("clip.txt")).expect("txt"),
            "keep me\n"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn cancelled_status_loads_apart_from_failed() {
        // Arrange
        let dir = temp_dir("cancelled");
        let audio_file = dir.join("clip.flac");
        fs::write(&audio_file, b"fLaC").expect("audio_file");
        write_sidecar(
            &audio_file,
            &Sidecar {
                id: 5,
                started_at_unix_ms: 1,
                ended_at_unix_ms: 2,
                duration_ms: 1,
                microphone_id: "m".into(),
                system_audio_id: "s".into(),
                problems: Vec::new(),
            },
        )
        .expect("sidecar");

        // Act
        update_transcription(&audio_file, TranscriptionWrite::Cancelled, None).expect("cancelled");
        let cancelled = load_sidecar(&audio_file);
        update_transcription(&audio_file, TranscriptionWrite::Failed, None).expect("failed");
        let failed = load_sidecar(&audio_file);

        // Assert
        match (cancelled, failed) {
            (SidecarLoad::Loaded(c), SidecarLoad::Loaded(f)) => {
                assert_eq!(c.transcript, TranscriptStatus::Failed);
                assert!(c.transcript_cancelled);
                assert_eq!(f.transcript, TranscriptStatus::Failed);
                assert!(!f.transcript_cancelled);
            }
            other => panic!("expected loaded, got {other:?}"),
        }
        let _ = fs::remove_dir_all(dir);
    }
}
