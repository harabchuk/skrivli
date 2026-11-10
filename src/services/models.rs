//! User-initiated speech-model download and install. No GPUI, no network in tests.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use crate::common::error::AppError;
use crate::infrastructure::ai::models::{
    discover, install_from_reader, install_picked_file, path_for_variant, validate_model_file,
    DownloadProgress, ModelManifest,
};
use crate::services::ports::ModelSource;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModelPhase {
    Idle,
    Downloading,
    Ready,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ModelFile {
    pub(crate) filename: String,
    pub(crate) variant_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ModelSnapshot {
    pub(crate) phase: ModelPhase,
    pub(crate) progress_percent: u8,
    pub(crate) failure: Option<&'static str>,
    pub(crate) notice: Option<&'static str>,
    pub(crate) total_bytes: u64,
    pub(crate) models: Vec<ModelFile>,
}

enum ModelCommand {
    Download(String),
    Cancel,
    Retry,
    #[cfg_attr(not(test), allow(dead_code))] // `accept_file`; the prompt no longer picks a file.
    Accept(PathBuf),
    Refresh,
    Shutdown,
}

/// Owns the download worker. Bootstrap constructs it; shutdown joins it.
#[derive(Clone)]
pub(crate) struct ModelHost {
    tx: Sender<ModelCommand>,
    cancel: Arc<AtomicBool>,
}

impl ModelHost {
    pub(crate) fn spawn(
        models_dir: PathBuf,
        source: Box<dyn ModelSource>,
        on_change: Box<dyn Fn(ModelSnapshot) + Send>,
    ) -> (Self, JoinHandle<()>) {
        match ModelManifest::load_bundled() {
            Ok(manifest) => Self::spawn_with_manifest(models_dir, manifest, source, on_change),
            Err(error) => spawn_manifest_failure(error, on_change),
        }
    }

    pub(crate) fn spawn_with_manifest(
        models_dir: PathBuf,
        manifest: ModelManifest,
        source: Box<dyn ModelSource>,
        on_change: Box<dyn Fn(ModelSnapshot) + Send>,
    ) -> (Self, JoinHandle<()>) {
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let handle = thread::spawn(move || {
            worker_loop(rx, models_dir, manifest, source, worker_cancel, on_change);
        });
        (Self { tx, cancel }, handle)
    }

    pub(crate) fn download(&self, id: impl Into<String>) {
        let _ = self.tx.send(ModelCommand::Download(id.into()));
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn download_default(&self) {
        self.download(String::new());
    }

    pub(crate) fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
        let _ = self.tx.send(ModelCommand::Cancel);
    }

    pub(crate) fn retry(&self) {
        let _ = self.tx.send(ModelCommand::Retry);
    }

    #[cfg_attr(not(test), allow(dead_code))] // Local-file install. The prompt downloads a catalog variant.
    pub(crate) fn accept_file(&self, path: PathBuf) {
        let _ = self.tx.send(ModelCommand::Accept(path));
    }

    pub(crate) fn refresh(&self) {
        let _ = self.tx.send(ModelCommand::Refresh);
    }

    pub(crate) fn shutdown(&self) {
        let _ = self.tx.send(ModelCommand::Shutdown);
    }
}

fn spawn_manifest_failure(
    error: AppError,
    on_change: Box<dyn Fn(ModelSnapshot) + Send>,
) -> (ModelHost, JoinHandle<()>) {
    let (tx, rx) = mpsc::channel();
    let message = error.user_message();
    let handle = thread::spawn(move || {
        on_change(snapshot(
            ModelPhase::Failed,
            0,
            Some(message),
            None,
            0,
            Vec::new(),
        ));
        while let Ok(command) = rx.recv() {
            if matches!(command, ModelCommand::Shutdown) {
                break;
            }
        }
    });
    (
        ModelHost {
            tx,
            cancel: Arc::new(AtomicBool::new(false)),
        },
        handle,
    )
}

fn worker_loop(
    rx: mpsc::Receiver<ModelCommand>,
    models_dir: PathBuf,
    manifest: ModelManifest,
    mut source: Box<dyn ModelSource>,
    cancel: Arc<AtomicBool>,
    on_change: Box<dyn Fn(ModelSnapshot) + Send>,
) {
    let mut listed = Vec::new();
    let mut requested = String::new();
    while let Ok(command) = rx.recv() {
        match command {
            ModelCommand::Shutdown => break,
            ModelCommand::Cancel => cancel.store(true, Ordering::Release),
            ModelCommand::Refresh => {
                listed = accepted_models(&models_dir, &manifest);
                on_change(snapshot(ModelPhase::Idle, 0, None, None, 0, listed.clone()));
            }
            ModelCommand::Accept(path) => match install_picked_file(&path, &models_dir) {
                Ok(_) => {
                    listed = accepted_models(&models_dir, &manifest);
                    on_change(snapshot(
                        ModelPhase::Ready,
                        100,
                        None,
                        None,
                        0,
                        listed.clone(),
                    ));
                }
                Err(error) => {
                    tracing::error!(error = %error, "picked transcription model was not accepted");
                    on_change(snapshot(
                        ModelPhase::Failed,
                        0,
                        Some(error.user_message()),
                        None,
                        0,
                        listed.clone(),
                    ));
                }
            },
            ModelCommand::Download(id) => {
                requested = id;
                listed = run_download(
                    &models_dir,
                    &manifest,
                    &requested,
                    source.as_mut(),
                    &cancel,
                    &on_change,
                    listed,
                );
            }
            ModelCommand::Retry => {
                listed = run_download(
                    &models_dir,
                    &manifest,
                    &requested,
                    source.as_mut(),
                    &cancel,
                    &on_change,
                    listed,
                );
            }
        }
    }
}

fn run_download(
    models_dir: &Path,
    manifest: &ModelManifest,
    variant_id: &str,
    source: &mut dyn ModelSource,
    cancel: &AtomicBool,
    on_change: &dyn Fn(ModelSnapshot),
    mut listed: Vec<ModelFile>,
) -> Vec<ModelFile> {
    cancel.store(false, Ordering::Release);
    let entry = if variant_id.is_empty() {
        manifest.default_entry()
    } else {
        manifest.by_id(variant_id)
    };
    let Some(entry) = entry else {
        on_change(snapshot(
            ModelPhase::Failed,
            0,
            Some(AppError::message("model manifest", "no default model").user_message()),
            None,
            0,
            listed.clone(),
        ));
        return listed;
    };
    let total = entry.bytes();
    on_change(snapshot(
        ModelPhase::Downloading,
        0,
        None,
        None,
        total,
        listed.clone(),
    ));
    let url = entry.download_url(manifest.source_base());
    let mut reader = match source.open(&url) {
        Ok(reader) => reader,
        Err(error) => {
            tracing::error!(error = %error, "transcription model download did not start");
            on_change(snapshot(
                ModelPhase::Failed,
                0,
                Some(error.user_message()),
                None,
                total,
                listed.clone(),
            ));
            return listed;
        }
    };
    let destination = path_for_variant(models_dir, entry);
    let mut last_percent = 0_u8;
    let mut saw_percent = false;
    let outcome = install_from_reader(
        reader.as_mut(),
        &destination,
        entry.bytes(),
        entry.sha256(),
        Some(entry.bytes()),
        cancel,
        &mut |progress| {
            let percent = percent_of(&progress);
            if saw_percent && percent == last_percent {
                return;
            }
            saw_percent = true;
            last_percent = percent;
            on_change(snapshot(
                ModelPhase::Downloading,
                percent,
                None,
                None,
                total,
                listed.clone(),
            ));
        },
    );
    match outcome {
        Ok(()) => {
            listed = accepted_models(models_dir, manifest);
            on_change(snapshot(
                ModelPhase::Ready,
                100,
                None,
                None,
                total,
                listed.clone(),
            ));
        }
        Err(error) if error.user_message() == "The model download was cancelled." => {
            tracing::info!("transcription model download cancelled");
            on_change(snapshot(
                ModelPhase::Cancelled,
                last_percent,
                Some(error.user_message()),
                None,
                total,
                listed.clone(),
            ));
        }
        Err(error) => {
            tracing::error!(error = %error, "transcription model download failed");
            on_change(snapshot(
                ModelPhase::Failed,
                last_percent,
                Some(error.user_message()),
                None,
                total,
                listed.clone(),
            ));
        }
    }
    listed
}

fn accepted_models(models_dir: &Path, manifest: &ModelManifest) -> Vec<ModelFile> {
    discover(models_dir, None, manifest)
        .into_iter()
        .filter(|model| validate_model_file(&model.path).is_ok())
        .map(|model| ModelFile {
            filename: model.filename,
            variant_id: model.variant_id,
        })
        .collect()
}

fn percent_of(progress: &DownloadProgress) -> u8 {
    let Some(total) = progress.total_bytes.filter(|total| *total > 0) else {
        return 0;
    };
    let percent = progress.bytes_downloaded.saturating_mul(100) / total;
    u8::try_from(percent.min(100)).unwrap_or(100)
}

fn snapshot(
    phase: ModelPhase,
    progress_percent: u8,
    failure: Option<&'static str>,
    notice: Option<&'static str>,
    total_bytes: u64,
    models: Vec<ModelFile>,
) -> ModelSnapshot {
    ModelSnapshot {
        phase,
        progress_percent,
        failure,
        notice,
        total_bytes,
        models,
    }
}

#[cfg(test)]
mod tests {
    use std::io::{self, Cursor, Read};
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use sha2::{Digest, Sha256};

    use super::{accepted_models, ModelHost, ModelPhase, ModelSnapshot};
    use crate::common::error::AppError;
    use crate::infrastructure::ai::models::{validate_model_file, ModelManifest};
    use crate::services::ports::ModelSource;

    struct CountingRead {
        inner: Cursor<Vec<u8>>,
        reads: Arc<Mutex<usize>>,
    }

    impl Read for CountingRead {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let take = 1.min(buf.len());
            let n = self.inner.read(&mut buf[..take])?;
            if n > 0 {
                *self.reads.lock().expect("reads") += 1;
            }
            Ok(n)
        }
    }

    struct ScriptedSource {
        bodies: Vec<Vec<u8>>,
        reads: Arc<Mutex<usize>>,
        next: usize,
    }

    impl ModelSource for ScriptedSource {
        fn open(&mut self, _url: &str) -> Result<Box<dyn Read + Send>, AppError> {
            let body = self
                .bodies
                .get(self.next)
                .cloned()
                .unwrap_or_else(|| self.bodies.last().cloned().unwrap_or_default());
            self.next += 1;
            Ok(Box::new(CountingRead {
                inner: Cursor::new(body),
                reads: Arc::clone(&self.reads),
            }))
        }
    }

    fn two_variant_manifest(bytes: u64, sha: &str) -> ModelManifest {
        let json = format!(
            r#"{{
                "schema_version": 1,
                "source_base": "https://example.invalid/models",
                "license": "MIT",
                "models": [
                    {{
                        "id": "model-a",
                        "filename": "model-a.gguf",
                        "bytes": {bytes},
                        "sha256": "{sha}",
                        "default": true,
                        "name": "Base",
                        "description": "Test model",
                        "speed": 2,
                        "accuracy": 2
                    }},
                    {{
                        "id": "model-b",
                        "filename": "model-b.gguf",
                        "bytes": {bytes},
                        "sha256": "{sha}",
                        "default": false,
                        "name": "Tiny",
                        "description": "Test model",
                        "speed": 3,
                        "accuracy": 1
                    }}
                ]
            }}"#
        );
        ModelManifest::parse(&json).expect("manifest")
    }

    fn tiny_manifest(bytes: u64, sha: &str) -> ModelManifest {
        let json = format!(
            r#"{{
                "schema_version": 1,
                "source_base": "https://example.invalid/models",
                "license": "MIT",
                "models": [{{
                    "id": "model-a",
                    "filename": "model-a.gguf",
                    "bytes": {bytes},
                    "sha256": "{sha}",
                    "default": true,
                    "name": "Base",
                    "description": "Test model",
                    "speed": 2,
                    "accuracy": 2
                }}]
            }}"#
        );
        ModelManifest::parse(&json).expect("manifest")
    }

    fn wait_phase(snaps: &Mutex<Vec<ModelSnapshot>>, phase: ModelPhase) -> ModelSnapshot {
        wait_after(snaps, 0, phase)
    }

    fn wait_after(
        snaps: &Mutex<Vec<ModelSnapshot>>,
        seen: usize,
        phase: ModelPhase,
    ) -> ModelSnapshot {
        for _ in 0..100 {
            if let Some(snap) = snaps
                .lock()
                .expect("snaps")
                .iter()
                .skip(seen)
                .rev()
                .find(|snap| snap.phase == phase)
            {
                return snap.clone();
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("timeout waiting for {phase:?}");
    }

    #[test]
    fn download_installs_the_requested_variant_not_the_default() {
        // Arrange
        let dir =
            std::env::temp_dir().join(format!("skrivli-model-pick-id-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let body = b"GGUF".repeat(40);
        let sha = hex::encode(Sha256::digest(&body));
        let manifest = two_variant_manifest(body.len() as u64, &sha);
        let snaps = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&snaps);
        let (host, handle) = ModelHost::spawn_with_manifest(
            dir.clone(),
            manifest,
            Box::new(ScriptedSource {
                bodies: vec![body],
                reads: Arc::new(Mutex::new(0)),
                next: 0,
            }),
            Box::new(move |snap| {
                sink.lock().expect("snaps").push(snap);
            }),
        );

        // Act
        host.download("model-b");
        let _snap = wait_phase(&snaps, ModelPhase::Ready);
        host.shutdown();
        let _ = handle.join();

        // Assert
        assert!(dir.join("model-b.gguf").is_file());
        assert!(!dir.join("model-a.gguf").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn download_installs_verified_bytes_and_coalesces_progress() {
        // Arrange
        let dir = std::env::temp_dir().join(format!("skrivli-model-ok-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let body = b"GGUF".repeat(80);
        let sha = hex::encode(Sha256::digest(&body));
        let manifest = tiny_manifest(body.len() as u64, &sha);
        let reads = Arc::new(Mutex::new(0_usize));
        let snaps = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&snaps);
        let (host, handle) = ModelHost::spawn_with_manifest(
            dir.clone(),
            manifest,
            Box::new(ScriptedSource {
                bodies: vec![body.clone()],
                reads: Arc::clone(&reads),
                next: 0,
            }),
            Box::new(move |snap| {
                sink.lock().expect("snaps").push(snap);
            }),
        );

        // Act
        host.download_default();
        let snap = wait_phase(&snaps, ModelPhase::Ready);
        host.shutdown();
        let _ = handle.join();

        // Assert
        let installed = dir.join("model-a.gguf");
        assert!(installed.is_file());
        assert!(validate_model_file(&installed).is_ok());
        assert_eq!(snap.progress_percent, 100);
        let read_count = *reads.lock().expect("reads");
        let notify_count = snaps.lock().expect("snaps").len();
        assert!(read_count > 20, "reader was called {read_count} times");
        assert!(
            notify_count < read_count,
            "notifies {notify_count} should be fewer than reads {read_count}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn hash_mismatch_reports_verify_model_and_leaves_no_file() {
        // Arrange
        let dir = std::env::temp_dir().join(format!("skrivli-model-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let body = b"ggml-nope".to_vec();
        let manifest = tiny_manifest(body.len() as u64, "deadbeef");
        let snaps = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&snaps);
        let (host, handle) = ModelHost::spawn_with_manifest(
            dir.clone(),
            manifest,
            Box::new(ScriptedSource {
                bodies: vec![body],
                reads: Arc::new(Mutex::new(0)),
                next: 0,
            }),
            Box::new(move |snap| {
                sink.lock().expect("snaps").push(snap);
            }),
        );

        // Act
        host.download_default();
        let snap = wait_phase(&snaps, ModelPhase::Failed);
        host.shutdown();
        let _ = handle.join();

        // Assert
        assert_eq!(
            snap.failure,
            Some("The downloaded transcription model could not be verified.")
        );
        assert!(!dir.join("model-a.gguf").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn spawn_does_not_start_a_download() {
        // Arrange
        let dir = std::env::temp_dir().join(format!("skrivli-model-idle-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let snaps = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&snaps);
        let (host, handle) = ModelHost::spawn(
            dir.clone(),
            Box::new(ScriptedSource {
                bodies: vec![b"ggml".to_vec()],
                reads: Arc::new(Mutex::new(0)),
                next: 0,
            }),
            Box::new(move |snap| {
                sink.lock().expect("snaps").push(snap);
            }),
        );

        // Act
        std::thread::sleep(Duration::from_millis(50));
        host.shutdown();
        let _ = handle.join();

        // Assert
        assert!(snaps.lock().expect("snaps").is_empty());
        assert!(std::fs::read_dir(&dir).expect("dir").next().is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn accept_file_distinguishes_missing_and_unsupported() {
        // Arrange
        let dir = std::env::temp_dir().join(format!("skrivli-model-pick-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let bad = dir.join("bad.gguf");
        std::fs::write(&bad, b"nope").expect("bad");
        let good = dir.join("picked.gguf");
        std::fs::write(&good, b"GGUF-ok").expect("good");
        let snaps = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&snaps);
        let (host, handle) = ModelHost::spawn(
            dir.join("models"),
            Box::new(ScriptedSource {
                bodies: Vec::new(),
                reads: Arc::new(Mutex::new(0)),
                next: 0,
            }),
            Box::new(move |snap| {
                sink.lock().expect("snaps").push(snap);
            }),
        );

        // Act
        host.accept_file(PathBuf::from("missing.gguf"));
        let missing = wait_after(&snaps, 0, ModelPhase::Failed);
        let after_missing = snaps.lock().expect("snaps").len();
        host.accept_file(bad);
        let unsupported = wait_after(&snaps, after_missing, ModelPhase::Failed);
        let after_bad = snaps.lock().expect("snaps").len();
        host.accept_file(good);
        let ready = wait_after(&snaps, after_bad, ModelPhase::Ready);
        host.shutdown();
        let _ = handle.join();

        // Assert
        assert_eq!(
            missing.failure,
            Some("The transcription model file is missing.")
        );
        assert_eq!(
            unsupported.failure,
            Some("The transcription model file is not supported.")
        );
        assert!(ready
            .models
            .iter()
            .any(|model| model.filename.contains("picked")));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn old_whisper_files_are_ignored_and_left_on_disk() {
        // Arrange
        let dir = std::env::temp_dir().join(format!("skrivli-model-old-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let old = dir.join("ggml-base-q5_1.bin");
        std::fs::write(&old, [0x6c, 0x6d, 0x67, 0x67, 0, 0, 0, 0]).expect("old model");
        let manifest = ModelManifest::load_bundled().expect("manifest");

        // Act
        let accepted = accepted_models(&dir, &manifest);

        // Assert
        assert!(accepted.is_empty());
        assert!(old.is_file());
        let _ = std::fs::remove_dir_all(dir);
    }
}
