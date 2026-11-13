//! Skrivli application library. The GPUI thread must not block on audio, disk, or inference.

// Translations live in `locales/ui.json`. App text is under `app.`; Swedish and Spanish text for
// Kit's own controls is under `gpui_component.` (see `app::bootstrap`).
rust_i18n::i18n!("locales", fallback = "en");

/// Translated text for the active language, as a GPUI `SharedString`.
macro_rules! ts {
    ($($args:tt)*) => {
        gpui_kit::SharedString::from(rust_i18n::t!($($args)*).into_owned())
    };
}

pub(crate) mod app;

/// Process entry used by the binary. Returns a process exit code.
pub fn run() -> i32 {
    app::run()
}
pub(crate) mod common;
pub(crate) mod domain;
pub(crate) mod infrastructure;
pub(crate) mod services;
pub(crate) mod ui;
pub(crate) mod views;

/// In-process recording harness. It drives the microphone service with a fake
/// capture port so `cargo test` does not open a WASAPI device.
pub mod testing {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{self, Receiver, Sender};
    use std::sync::{Arc, Mutex};
    use std::thread::JoinHandle;
    use std::time::Duration;

    pub use crate::services::recording_harness::{
        HarnessNotice, RecordingHarness, RecordingHarnessPhase,
    };

    use crate::common::error::AppError;
    use crate::domain::ids::RecordingId;
    use crate::infrastructure::audio::energy_pause::EnergyPauseDetectorFactory;
    use crate::infrastructure::storage::sidecar::{
        load_sidecar, write_sidecar, Sidecar, SidecarLoad,
    };
    use crate::services::ports::{InferencePort, InferenceSegment};
    use crate::services::transcription::{
        JobPhase, TranscriptionConfig, TranscriptionHost, TranscriptionSnapshot,
    };

    /// Format timed segments as UTF-8 transcript text (integration tests).
    pub fn format_transcript_segments(segments: &[(u64, u64, &str)]) -> String {
        let owned: Vec<_> = segments
            .iter()
            .map(|(start, end, text)| {
                crate::domain::transcript::TranscriptSegment::new(*start, *end, (*text).to_string())
            })
            .collect();
        crate::domain::transcript::format_transcript(&owned)
    }

    /// Atomic `<basename>.txt` write beside `audio_path` (integration tests).
    pub fn write_transcript_text(audio_path: &Path, text: &str) -> Result<PathBuf, String> {
        crate::infrastructure::storage::sidecar::write_transcript_text(audio_path, text)
            .map_err(|error| error.to_string())
    }

    /// Write `frames` of stereo 48 kHz silence as a finalized FLAC (integration tests).
    pub fn write_silent_flac(path: &Path, frames: usize) -> Result<(), String> {
        crate::infrastructure::files::flac::write_silence(path, frames)
            .map_err(|error| error.to_string())
    }

    /// Resolve `<basename>.txt` for an audio path (integration tests).
    pub fn transcript_path_for(audio_path: &Path) -> Result<PathBuf, String> {
        crate::infrastructure::storage::sidecar::transcript_path_for(audio_path)
            .map_err(|error| error.to_string())
    }

    /// Write a minimal sidecar for transcription integration tests.
    pub fn write_recording_sidecar(audio_path: &Path, id: u64) -> Result<(), String> {
        write_sidecar(
            audio_path,
            &Sidecar {
                id,
                started_at_unix_ms: 1,
                ended_at_unix_ms: 2,
                duration_ms: 1,
                microphone_id: "m".into(),
                system_audio_id: "s".into(),
                problems: Vec::new(),
            },
        )
        .map(|_| ())
        .map_err(|error| error.to_string())
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct TranscriptionFlowConfig {
        pub read_frames: usize,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum TranscriptionFlowPhase {
        Queued,
        Preparing,
        Transcribing,
        Done,
        Failed,
        Cancelled,
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct TranscriptionFlowResult {
        pub phase: TranscriptionFlowPhase,
        pub sidecar_status: Option<String>,
    }

    /// Configurable fake inference for integration tests (no speech engine). The first piece
    /// returns the scripted text.
    pub struct FakeInferenceEngine {
        fail_load_name: Option<&'static str>,
        fail_chunk: bool,
        hold_ms: u64,
        calls: Arc<Mutex<Vec<usize>>>,
        text: Option<String>,
    }

    impl FakeInferenceEngine {
        pub fn success(text: &str) -> Self {
            Self {
                fail_load_name: Some("invalid.bin"),
                fail_chunk: false,
                hold_ms: 0,
                calls: Arc::new(Mutex::new(Vec::new())),
                text: Some(text.to_string()),
            }
        }

        pub fn success_with_calls(text: &str, calls: Arc<Mutex<Vec<usize>>>) -> Self {
            let mut engine = Self::success(text);
            engine.calls = calls;
            engine
        }

        pub fn fail_chunk() -> Self {
            let mut engine = Self::success("");
            engine.fail_chunk = true;
            engine
        }

        pub fn slow(hold_ms: u64) -> Self {
            let mut engine = Self::success("slow");
            engine.hold_ms = hold_ms;
            engine
        }

        pub fn race_cancel() -> Self {
            let mut engine = Self::success("maybe");
            engine.hold_ms = 50;
            engine
        }
    }

    impl InferencePort for FakeInferenceEngine {
        fn load(&mut self, path: &Path) -> Result<(), AppError> {
            if self
                .fail_load_name
                .is_some_and(|name| path.file_name().and_then(|n| n.to_str()) == Some(name))
            {
                return Err(AppError::message("model unsupported", "fake reject"));
            }
            Ok(())
        }

        fn unload(&mut self) {}

        fn transcribe(&mut self, samples: &[f32]) -> Result<Vec<InferenceSegment>, AppError> {
            self.calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(samples.len());
            if self.hold_ms > 0 {
                std::thread::sleep(Duration::from_millis(self.hold_ms));
            }
            if self.fail_chunk {
                return Err(AppError::message("inference", "engine boom"));
            }
            Ok(self
                .text
                .take()
                .map(|text| InferenceSegment {
                    start_ms: 0,
                    end_ms: 500,
                    text,
                })
                .into_iter()
                .collect())
        }
    }

    /// Run one transcription job to a terminal phase (integration tests).
    pub fn run_transcription_flow(
        engine: FakeInferenceEngine,
        config: TranscriptionFlowConfig,
        recording_id: u64,
        audio_path: &Path,
        model_path: &Path,
        model_id: &str,
        cancel_after: Option<Duration>,
    ) -> TranscriptionFlowResult {
        let (host, handle) = TranscriptionHost::spawn_with_config(
            Box::new(engine),
            Box::new(EnergyPauseDetectorFactory),
            TranscriptionConfig {
                read_frames: config.read_frames,
            },
            Box::new(|_| {}),
        );
        host.enqueue(
            RecordingId::from_raw(recording_id),
            audio_path.to_path_buf(),
            model_path.to_path_buf(),
            model_id,
        );
        if let Some(delay) = cancel_after {
            std::thread::sleep(delay);
            // The worker may not have picked the job up yet on a slow machine;
            // wait for a job id (or a terminal phase) instead of skipping the cancel.
            for _ in 0..200 {
                let snapshot = host.snapshot();
                if let Some(job) = snapshot.job_id {
                    host.cancel(job);
                    break;
                }
                if matches!(
                    snapshot.phase,
                    JobPhase::Done | JobPhase::Failed | JobPhase::Cancelled
                ) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        wait_terminal(&host);
        let phase = map_phase(host.snapshot().phase);
        let sidecar_status = match load_sidecar(audio_path) {
            SidecarLoad::Loaded(doc) => Some(match doc.transcript {
                crate::domain::recording::TranscriptStatus::Ready => "ready".into(),
                crate::domain::recording::TranscriptStatus::Failed => {
                    // Distinguish cancelled vs failed via raw JSON when present.
                    std::fs::read_to_string(
                        crate::infrastructure::storage::sidecar::sidecar_path_for(audio_path)
                            .unwrap_or_default(),
                    )
                    .ok()
                    .map(|text| {
                        if text.contains("\"cancelled\"") {
                            "cancelled".into()
                        } else {
                            "failed".into()
                        }
                    })
                    .unwrap_or_else(|| "failed".into())
                }
                crate::domain::recording::TranscriptStatus::Queued => "queued".into(),
                crate::domain::recording::TranscriptStatus::Progress(_) => "progress".into(),
                crate::domain::recording::TranscriptStatus::None => "none".into(),
            }),
            _ => None,
        };
        host.shutdown();
        let _ = handle.join();
        TranscriptionFlowResult {
            phase,
            sidecar_status,
        }
    }

    /// Enqueue two recordings and return Preparing order by recording id.
    pub fn run_transcription_fifo(
        engine: FakeInferenceEngine,
        config: TranscriptionFlowConfig,
        jobs: &[(u64, &Path)],
        model_path: &Path,
        model_id: &str,
        order: Arc<Mutex<Vec<u64>>>,
    ) -> Vec<u64> {
        let sink = Arc::clone(&order);
        let (host, handle) = TranscriptionHost::spawn_with_config(
            Box::new(engine),
            Box::new(EnergyPauseDetectorFactory),
            TranscriptionConfig {
                read_frames: config.read_frames,
            },
            Box::new(move |snap: TranscriptionSnapshot| {
                if snap.phase == JobPhase::Preparing {
                    if let Some(id) = snap.recording_id {
                        sink.lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .push(id.raw());
                    }
                }
            }),
        );
        for (id, path) in jobs {
            host.enqueue(
                RecordingId::from_raw(*id),
                path.to_path_buf(),
                model_path.to_path_buf(),
                model_id,
            );
        }
        if let Some((_, last)) = jobs.last() {
            let txt =
                crate::infrastructure::storage::sidecar::transcript_path_for(last).expect("path");
            for _ in 0..200 {
                if order
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .len()
                    >= jobs.len()
                    && txt.is_file()
                {
                    break;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        }
        host.shutdown();
        let _ = handle.join();
        order
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn wait_terminal(host: &TranscriptionHost) {
        for _ in 0..200 {
            let phase = host.snapshot().phase;
            if matches!(
                phase,
                JobPhase::Done | JobPhase::Failed | JobPhase::Cancelled
            ) {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!(
            "timeout waiting for transcription terminal state: {:?}",
            host.snapshot()
        );
    }

    fn map_phase(phase: JobPhase) -> TranscriptionFlowPhase {
        match phase {
            JobPhase::Queued => TranscriptionFlowPhase::Queued,
            JobPhase::Preparing => TranscriptionFlowPhase::Preparing,
            JobPhase::Transcribing => TranscriptionFlowPhase::Transcribing,
            JobPhase::Done => TranscriptionFlowPhase::Done,
            JobPhase::Failed => TranscriptionFlowPhase::Failed,
            JobPhase::Cancelled => TranscriptionFlowPhase::Cancelled,
        }
    }

    /// Where the fake inference blocks. No microphone and no speech engine.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum InferenceHold {
        /// Block in `load`, before the audio file is opened, so a rename can move the file.
        BeforeOpen,
        /// Block inside the first piece, then return text after release (a late completion).
        DuringPiece,
    }

    /// Library plus transcription, with a gate on the fake inference port.
    pub struct IdentitySession {
        transcription: TranscriptionHost,
        transcription_thread: Option<JoinHandle<()>>,
        library: crate::services::library::LibraryHost,
        library_thread: Option<JoinHandle<()>>,
        stop: Arc<AtomicBool>,
        trash: Arc<RecordingTrash>,
        entered: Mutex<Option<Receiver<()>>>,
        release: Sender<()>,
        model: PathBuf,
        library_snapshots: Arc<Mutex<Vec<crate::services::library::LibrarySnapshot>>>,
    }

    impl IdentitySession {
        pub fn start(directory: &Path, hold: InferenceHold) -> Self {
            let (entered_tx, entered_rx) = mpsc::channel();
            let (release_tx, release_rx) = mpsc::channel();
            let engine = HoldEngine {
                entered: Mutex::new(Some(entered_tx)),
                release: Mutex::new(Some(release_rx)),
                hold_before_open: matches!(hold, InferenceHold::BeforeOpen),
                held: false,
            };
            let (transcription, transcription_thread) = TranscriptionHost::spawn_with_config(
                Box::new(engine),
                Box::new(EnergyPauseDetectorFactory),
                TranscriptionConfig { read_frames: 1_000 },
                Box::new(|_| {}),
            );
            let trash = Arc::new(RecordingTrash {
                discarded: Mutex::new(Vec::new()),
            });
            let library_snapshots = Arc::new(Mutex::new(Vec::new()));
            let sink = Arc::clone(&library_snapshots);
            let stop = Arc::new(AtomicBool::new(false));
            let model = directory.join("ggml-test.bin");
            let _ = std::fs::write(&model, b"ggml");
            let (library, library_thread) = crate::services::library::LibraryHost::spawn(
                directory.to_path_buf(),
                trash.clone(),
                Arc::new(QuietWatch),
                Arc::new(transcription.clone()),
                Box::new(move |snapshot| {
                    sink.lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .push(snapshot);
                }),
                Arc::clone(&stop),
            );
            Self {
                transcription,
                transcription_thread: Some(transcription_thread),
                library,
                library_thread: Some(library_thread),
                stop,
                trash,
                entered: Mutex::new(Some(entered_rx)),
                release: release_tx,
                model,
                library_snapshots,
            }
        }

        pub fn enqueue(&self, id: u64, audio: &Path) {
            self.transcription.enqueue(
                RecordingId::from_raw(id),
                audio.to_path_buf(),
                self.model.clone(),
                "base-q5_1",
            );
        }

        pub fn wait_until_held(&self) {
            let rx = self
                .entered
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take()
                .expect("hold gate");
            rx.recv_timeout(Duration::from_secs(5))
                .expect("inference did not block");
        }

        pub fn release(&self) {
            let _ = self.release.send(());
        }

        pub fn rename(&self, id: u64, stem: &str) {
            self.library
                .rename(RecordingId::from_raw(id), stem.to_string());
        }

        pub fn delete(&self, id: u64) {
            self.library.delete(RecordingId::from_raw(id));
        }

        pub fn wait_for_library_id(&self, id: u64) {
            let deadline = std::time::Instant::now() + Duration::from_secs(3);
            while std::time::Instant::now() < deadline {
                let guard = self
                    .library_snapshots
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if guard.iter().any(|snapshot| {
                    snapshot
                        .catalog
                        .entries
                        .iter()
                        .any(|entry| entry.info.id().raw() == id)
                }) {
                    return;
                }
                drop(guard);
                std::thread::sleep(Duration::from_millis(20));
            }
            panic!("library did not list recording {id}");
        }

        pub fn discarded(&self) -> Vec<PathBuf> {
            self.trash
                .discarded
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
        }

        pub fn wait_terminal(&self) {
            wait_terminal(&self.transcription);
        }

        pub fn phase(&self) -> TranscriptionFlowPhase {
            map_phase(self.transcription.snapshot().phase)
        }

        pub fn shutdown(mut self) {
            let _ = self.release.send(());
            self.transcription.shutdown();
            self.library.shutdown();
            self.stop.store(true, Ordering::Release);
            if let Some(handle) = self.transcription_thread.take() {
                let _ = handle.join();
            }
            if let Some(handle) = self.library_thread.take() {
                let _ = handle.join();
            }
        }
    }

    struct HoldEngine {
        entered: Mutex<Option<Sender<()>>>,
        release: Mutex<Option<Receiver<()>>>,
        hold_before_open: bool,
        held: bool,
    }

    impl HoldEngine {
        fn block(&mut self) {
            if self.held {
                return;
            }
            self.held = true;
            if let Some(tx) = self
                .entered
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take()
            {
                let _ = tx.send(());
            }
            if let Some(rx) = self
                .release
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take()
            {
                let _ = rx.recv();
            }
        }
    }

    impl InferencePort for HoldEngine {
        fn load(&mut self, _model_path: &Path) -> Result<(), AppError> {
            if self.hold_before_open {
                self.block();
            }
            Ok(())
        }

        fn unload(&mut self) {}

        fn transcribe(&mut self, _samples: &[f32]) -> Result<Vec<InferenceSegment>, AppError> {
            if !self.hold_before_open {
                self.block();
            }
            Ok(vec![InferenceSegment {
                start_ms: 0,
                end_ms: 400,
                text: "noted".into(),
            }])
        }
    }

    struct RecordingTrash {
        discarded: Mutex<Vec<PathBuf>>,
    }

    impl crate::services::ports::Trash for RecordingTrash {
        fn discard(&self, paths: &[PathBuf]) -> Result<(), AppError> {
            self.discarded
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .extend_from_slice(paths);
            Ok(())
        }
    }

    struct QuietWatch;

    impl crate::services::ports::DirectoryWatch for QuietWatch {
        fn spawn(
            &self,
            _directory: PathBuf,
            _stop: Arc<AtomicBool>,
            _debounce: Duration,
        ) -> (Receiver<crate::services::ports::WatchEvent>, JoinHandle<()>) {
            let (tx, rx) = mpsc::channel();
            drop(tx);
            let handle = std::thread::spawn(|| {});
            (rx, handle)
        }
    }
}
