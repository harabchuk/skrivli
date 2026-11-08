//! Scan a recordings directory into a catalog. No GPUI; safe for worker threads.

use std::fs;
use std::path::{Path, PathBuf};

use crate::common::error::AppError;
use crate::domain::ids::RecordingId;
use crate::domain::recording::{
    settled_transcript_status, synthetic_recording_id, RecordingContainer, RecordingInfo,
    SidecarState, TranscriptStatus,
};
use crate::infrastructure::storage::sidecar::{
    interrupt_in_progress_transcription, load_sidecar, sidecar_path_for, sidecar_state,
    transcript_path_for, LoadedSidecar, SidecarLoad,
};

/// One catalog row with filesystem paths attached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LibraryEntry {
    pub(crate) info: RecordingInfo,
    pub(crate) audio_path: PathBuf,
    pub(crate) sidecar_path: Option<PathBuf>,
    pub(crate) transcript_path: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LibraryCatalog {
    pub(crate) entries: Vec<LibraryEntry>,
    pub(crate) error: Option<&'static str>,
}

impl LibraryCatalog {
    pub(crate) fn empty() -> Self {
        Self {
            entries: Vec::new(),
            error: None,
        }
    }

    pub(crate) fn find(&self, id: RecordingId) -> Option<&LibraryEntry> {
        self.entries.iter().find(|entry| entry.info.id() == id)
    }
}

/// Scan `directory` for supported audio, join sidecars/transcripts, newest first.
pub(crate) fn scan_library(directory: &Path) -> LibraryCatalog {
    if !directory.exists() {
        return LibraryCatalog {
            entries: Vec::new(),
            error: None,
        };
    }
    let read = match fs::read_dir(directory) {
        Ok(read) => read,
        Err(_) => {
            return LibraryCatalog {
                entries: Vec::new(),
                error: Some("Could not read the recordings folder."),
            };
        }
    };

    let mut entries = Vec::new();
    for item in read.flatten() {
        let path = item.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        // `.flac.partial` files are skipped here: the extension `partial` maps to no container.
        let ext = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");
        let Some(container) = RecordingContainer::from_extension(&ext.to_ascii_lowercase()) else {
            continue;
        };
        let meta = item.metadata().ok();
        let size_bytes = meta.as_ref().map(|meta| meta.len()).unwrap_or(0);
        let mtime_ms = meta
            .as_ref()
            .and_then(|meta| meta.modified().ok())
            .and_then(|time| {
                time.duration_since(std::time::UNIX_EPOCH)
                    .ok()
                    .map(|d| d.as_millis() as u64)
            });
        let stem = display_stem(name);
        let _ = interrupt_in_progress_transcription(&path);
        let sidecar = load_sidecar(&path);
        let transcript_file = transcript_path_for(&path)
            .ok()
            .filter(|path| path.is_file());
        let sidecar_file = sidecar_path_for(&path).ok().filter(|path| path.is_file());
        let entry = match &sidecar {
            SidecarLoad::Loaded(doc) => entry_from_sidecar(
                path,
                stem,
                container,
                size_bytes,
                doc,
                transcript_file,
                sidecar_file,
            ),
            SidecarLoad::Missing | SidecarLoad::Malformed => entry_without_sidecar(
                path,
                stem,
                container,
                size_bytes,
                mtime_ms,
                sidecar_state(&sidecar),
                transcript_file,
                sidecar_file,
            ),
        };
        entries.push(entry);
    }

    entries.sort_by(|left, right| {
        right
            .info
            .sort_key()
            .cmp(&left.info.sort_key())
            .then_with(|| left.info.name().cmp(right.info.name()))
    });

    LibraryCatalog {
        entries,
        error: None,
    }
}

/// Audio path a rename would use, before any file moves.
pub(crate) fn renamed_audio_path(
    entry: &LibraryEntry,
    new_stem: &str,
) -> Result<PathBuf, AppError> {
    let stem = sanitize_stem(new_stem)?;
    let directory = entry
        .audio_path
        .parent()
        .ok_or_else(|| AppError::message("library", "recording path has no folder"))?;
    let ext = entry.info.container().extension();
    let new_audio = directory.join(format!("{stem}.{ext}"));
    if new_audio.exists() {
        return Err(AppError::message(
            "library",
            "a recording with that name already exists",
        ));
    }
    Ok(new_audio)
}

/// Rename audio + sidecar + transcript as a group. Partial failures are reported.
pub(crate) fn rename_recording(
    catalog: &LibraryCatalog,
    id: RecordingId,
    new_stem: &str,
) -> Result<RenameResult, AppError> {
    let entry = catalog
        .find(id)
        .ok_or_else(|| AppError::message("library", "recording was not found"))?;
    let new_audio = renamed_audio_path(entry, new_stem)?;
    let stem = new_audio
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| AppError::message("library", "name is invalid"))?
        .to_string();
    let directory = new_audio
        .parent()
        .ok_or_else(|| AppError::message("library", "recording path has no folder"))?;
    let mut result = RenameResult {
        id,
        new_stem: stem.clone(),
        audio_ok: false,
        sidecar_ok: false,
        transcript_ok: false,
        warnings: Vec::new(),
    };

    fs::rename(&entry.audio_path, &new_audio)
        .map_err(|source| AppError::io("library rename", source))?;
    result.audio_ok = true;

    if let Some(old_sidecar) = &entry.sidecar_path {
        let new_sidecar = directory.join(format!("{stem}.json"));
        match fs::rename(old_sidecar, &new_sidecar) {
            Ok(()) => result.sidecar_ok = true,
            Err(error) => {
                tracing::warn!(error = %error, "sidecar was not renamed with audio");
                result
                    .warnings
                    .push("Metadata file was not renamed with the recording.");
            }
        }
    } else {
        result.sidecar_ok = true;
    }

    if let Some(old_transcript) = &entry.transcript_path {
        let new_transcript = directory.join(format!("{stem}.txt"));
        match fs::rename(old_transcript, &new_transcript) {
            Ok(()) => result.transcript_ok = true,
            Err(error) => {
                tracing::warn!(error = %error, "transcript was not renamed with audio");
                result
                    .warnings
                    .push("Transcript file was not renamed with the recording.");
            }
        }
    } else {
        result.transcript_ok = true;
    }

    Ok(result)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RenameResult {
    pub(crate) id: RecordingId,
    pub(crate) new_stem: String,
    pub(crate) audio_ok: bool,
    pub(crate) sidecar_ok: bool,
    pub(crate) transcript_ok: bool,
    pub(crate) warnings: Vec<&'static str>,
}

impl RenameResult {
    pub(crate) fn partial_failure(&self) -> bool {
        !self.warnings.is_empty()
    }
}

/// Paths that belong to a recording and should go to the Recycle Bin together.
///
/// A `.txt` or sidecar that exists beside the audio is included even when the
/// catalog row was scanned before that file was promoted.
pub(crate) fn associated_paths(entry: &LibraryEntry) -> Vec<PathBuf> {
    let mut paths = vec![entry.audio_path.clone()];
    push_unique(&mut paths, entry.sidecar_path.clone());
    if let Ok(path) = sidecar_path_for(&entry.audio_path) {
        if path.is_file() {
            push_unique(&mut paths, Some(path));
        }
    }
    push_unique(&mut paths, entry.transcript_path.clone());
    if let Ok(path) = transcript_path_for(&entry.audio_path) {
        if path.is_file() {
            push_unique(&mut paths, Some(path));
        }
    }
    paths
}

fn push_unique(paths: &mut Vec<PathBuf>, path: Option<PathBuf>) {
    let Some(path) = path else {
        return;
    };
    if !paths.iter().any(|existing| existing == &path) {
        paths.push(path);
    }
}

/// Detect duplicate recording ids after a scan (sidecar collisions).
#[cfg(test)]
pub(crate) fn colliding_ids(catalog: &LibraryCatalog) -> Vec<RecordingId> {
    use std::collections::HashMap;

    let mut counts: HashMap<RecordingId, usize> = HashMap::new();
    for entry in &catalog.entries {
        *counts.entry(entry.info.id()).or_insert(0) += 1;
    }
    counts
        .into_iter()
        .filter_map(|(id, count)| (count > 1).then_some(id))
        .collect()
}

fn entry_from_sidecar(
    audio_path: PathBuf,
    stem: String,
    container: RecordingContainer,
    size_bytes: u64,
    doc: &LoadedSidecar,
    transcript_path: Option<PathBuf>,
    sidecar_path: Option<PathBuf>,
) -> LibraryEntry {
    let has_transcript_file = transcript_path.is_some();
    let transcript = if has_transcript_file && doc.transcript == TranscriptStatus::None {
        TranscriptStatus::Ready
    } else {
        settled_transcript_status(
            doc.transcript,
            doc.transcript_cancelled,
            has_transcript_file,
        )
    };
    LibraryEntry {
        info: RecordingInfo::new(
            doc.id,
            stem,
            container,
            Some(doc.started_at_unix_ms),
            Some(doc.duration_ms),
            size_bytes,
            Some(doc.format),
            transcript,
            SidecarState::Present,
            doc.problems.clone(),
        ),
        audio_path,
        sidecar_path,
        transcript_path,
    }
}

#[allow(clippy::too_many_arguments)]
fn entry_without_sidecar(
    audio_path: PathBuf,
    stem: String,
    container: RecordingContainer,
    size_bytes: u64,
    mtime_ms: Option<u64>,
    sidecar: SidecarState,
    transcript_path: Option<PathBuf>,
    sidecar_path: Option<PathBuf>,
) -> LibraryEntry {
    let transcript = if transcript_path.is_some() {
        TranscriptStatus::Ready
    } else {
        TranscriptStatus::None
    };
    let id = synthetic_recording_id(&stem);
    LibraryEntry {
        info: RecordingInfo::new(
            id,
            stem,
            container,
            mtime_ms,
            None,
            size_bytes,
            None,
            transcript,
            sidecar,
            Vec::new(),
        ),
        audio_path,
        sidecar_path,
        transcript_path,
    }
}

fn display_stem(file_name: &str) -> String {
    file_name
        .strip_suffix(".flac")
        .unwrap_or(file_name)
        .to_string()
}

fn sanitize_stem(raw: &str) -> Result<String, AppError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(AppError::message("library", "name is empty"));
    }
    if trimmed.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|']) {
        return Err(AppError::message(
            "library",
            "name contains invalid characters",
        ));
    }
    if trimmed.ends_with('.') {
        return Err(AppError::message("library", "name is invalid"));
    }
    Ok(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::{associated_paths, colliding_ids, rename_recording, scan_library, LibraryCatalog};
    use crate::domain::ids::RecordingId;
    use crate::domain::recording::{SidecarState, TranscriptStatus};
    use crate::infrastructure::storage::sidecar::{
        update_transcription, write_sidecar, Sidecar, TranscriptionWrite,
    };

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("skrivli-lib-{tag}-{id}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("dir");
        dir
    }

    fn write_flac(path: &std::path::Path, bytes: &[u8]) {
        fs::write(path, bytes).expect("audio_file");
    }

    #[test]
    fn scan_settles_failed_and_cancelled_against_the_transcript_file() {
        // Arrange
        let dir = temp_dir("settle");
        let cases = [
            ("failed-with", 21_u64, TranscriptionWrite::Failed, true),
            ("failed-without", 22, TranscriptionWrite::Failed, false),
            ("cancel-with", 23, TranscriptionWrite::Cancelled, true),
            ("cancel-without", 24, TranscriptionWrite::Cancelled, false),
        ];
        for (name, id, write, has_txt) in cases {
            let audio_file = dir.join(format!("{name}.flac"));
            write_flac(&audio_file, b"fLaC");
            write_sidecar(
                &audio_file,
                &Sidecar {
                    id,
                    started_at_unix_ms: 100,
                    ended_at_unix_ms: 200,
                    duration_ms: 100,
                    microphone_id: "m".into(),
                    system_audio_id: "s".into(),
                    problems: Vec::new(),
                },
            )
            .expect("side");
            update_transcription(&audio_file, write, None).expect("status");
            if has_txt {
                fs::write(dir.join(format!("{name}.txt")), "old").expect("txt");
            }
        }

        // Act
        let catalog = scan_library(&dir);

        // Assert
        let status = |name: &str| {
            catalog
                .entries
                .iter()
                .find(|entry| entry.info.name() == name)
                .map(|entry| entry.info.transcript())
        };
        assert_eq!(status("failed-with"), Some(TranscriptStatus::Ready));
        assert_eq!(status("failed-without"), Some(TranscriptStatus::Failed));
        assert_eq!(status("cancel-with"), Some(TranscriptStatus::Ready));
        assert_eq!(status("cancel-without"), Some(TranscriptStatus::None));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn scan_skips_partials_and_lists_old_recovered_as_plain_flac() {
        // Arrange
        let dir = temp_dir("scan");
        write_flac(&dir.join("keep.flac"), b"fLaC");
        write_flac(&dir.join("busy.flac.partial"), b"partial");
        write_flac(&dir.join("crash.recovered.flac"), b"fLaC");
        fs::write(dir.join("orphan.txt"), "transcript only").expect("txt");
        write_sidecar(
            &dir.join("keep.flac"),
            &Sidecar {
                id: 11,
                started_at_unix_ms: 100,
                ended_at_unix_ms: 200,
                duration_ms: 100,
                microphone_id: "m".into(),
                system_audio_id: "s".into(),
                problems: Vec::new(),
            },
        )
        .expect("side");

        // Act
        let catalog = scan_library(&dir);

        // Assert
        assert!(catalog.error.is_none());
        assert_eq!(catalog.entries.len(), 2);
        assert!(catalog
            .entries
            .iter()
            .any(|entry| entry.info.name() == "keep" && entry.info.id().raw() == 11));
        assert!(catalog
            .entries
            .iter()
            .any(|entry| entry.info.name() == "crash.recovered"));
        assert!(!catalog
            .entries
            .iter()
            .any(|entry| entry.audio_path.ends_with("busy.flac.partial")));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn scan_preserves_malformed_sidecar_and_sorts_newest_first() {
        // Arrange
        let dir = temp_dir("bad");
        write_flac(&dir.join("new.flac"), b"fLaC-new");
        write_flac(&dir.join("old.flac"), b"fLaC-old");
        write_sidecar(
            &dir.join("new.flac"),
            &Sidecar {
                id: 2,
                started_at_unix_ms: 200,
                ended_at_unix_ms: 300,
                duration_ms: 100,
                microphone_id: "m".into(),
                system_audio_id: "s".into(),
                problems: Vec::new(),
            },
        )
        .expect("new");
        write_sidecar(
            &dir.join("old.flac"),
            &Sidecar {
                id: 1,
                started_at_unix_ms: 50,
                ended_at_unix_ms: 60,
                duration_ms: 10,
                microphone_id: "m".into(),
                system_audio_id: "s".into(),
                problems: Vec::new(),
            },
        )
        .expect("old");

        // Act
        let sorted = scan_library(&dir);
        fs::write(dir.join("new.json"), "{broken").expect("corrupt");
        let broken_catalog = scan_library(&dir);

        // Assert
        assert_eq!(sorted.entries[0].info.name(), "new");
        assert_eq!(sorted.entries[1].info.name(), "old");
        let broken = broken_catalog
            .entries
            .iter()
            .find(|entry| entry.info.name() == "new")
            .expect("new");
        assert_eq!(broken.info.sidecar(), SidecarState::Malformed);
        assert_eq!(broken_catalog.entries.len(), 2);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn collisions_and_external_delete_are_visible() {
        // Arrange
        let dir = temp_dir("col");
        write_flac(&dir.join("a.flac"), b"a");
        write_flac(&dir.join("b.flac"), b"b");
        for name in ["a.flac", "b.flac"] {
            write_sidecar(
                &dir.join(name),
                &Sidecar {
                    id: 42,
                    started_at_unix_ms: 1,
                    ended_at_unix_ms: 2,
                    duration_ms: 1,
                    microphone_id: "m".into(),
                    system_audio_id: "s".into(),
                    problems: Vec::new(),
                },
            )
            .expect("side");
        }

        // Act
        let catalog = scan_library(&dir);
        let collisions = colliding_ids(&catalog);
        fs::remove_file(dir.join("a.flac")).expect("delete");
        let after = scan_library(&dir);

        // Assert
        assert_eq!(collisions, vec![RecordingId::from_raw(42)]);
        assert_eq!(catalog.entries.len(), 2);
        assert_eq!(after.entries.len(), 1);
        assert_eq!(after.entries[0].info.name(), "b");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn rename_groups_associated_files() {
        // Arrange
        let dir = temp_dir("ren");
        let audio_file = dir.join("alpha.flac");
        write_flac(&audio_file, b"fLaC");
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
        .expect("side");
        fs::write(dir.join("alpha.txt"), "hi").expect("txt");
        let catalog = scan_library(&dir);
        let id = catalog.entries[0].info.id();

        // Act
        let result = rename_recording(&catalog, id, "beta").expect("rename");
        let refreshed = scan_library(&dir);

        // Assert
        assert!(result.audio_ok && result.sidecar_ok && result.transcript_ok);
        assert!(!result.partial_failure());
        assert!(dir.join("beta.flac").is_file());
        assert!(dir.join("beta.json").is_file());
        assert!(dir.join("beta.txt").is_file());
        assert!(!dir.join("alpha.flac").exists());
        assert_eq!(refreshed.entries[0].info.name(), "beta");
        assert_eq!(refreshed.entries[0].info.id().raw(), 5);
        let paths = associated_paths(&refreshed.entries[0]);
        assert_eq!(paths.len(), 3);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn empty_directory_is_not_an_error() {
        // Arrange
        let dir = temp_dir("empty");

        // Act
        let catalog = scan_library(&dir);

        // Assert
        assert_eq!(catalog, LibraryCatalog::empty());
        let _ = fs::remove_dir_all(dir);
    }
}
