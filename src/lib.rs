//! Skrivli application library. The GPUI thread must not block on audio, disk, or inference.

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
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    pub use crate::services::recording_harness::{
        HarnessNotice, RecordingHarness, RecordingHarnessPhase,
    };

    use crate::common::error::AppError;
    use crate::domain::ids::RecordingId;
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
        pub window_ms: u64,
        pub overlap_ms: u64,
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

    /// Configurable fake inference for integration tests (no whisper.cpp).
    pub struct FakeInferenceEngine {
        fail_load_name: Option<&'static str>,
        fail_chunk: bool,
        hold_ms: u64,
        race_complete_then_cancel: Arc<AtomicBool>,
        calls: Arc<Mutex<Vec<usize>>>,
        text: String,
    }

    impl FakeInferenceEngine {
        pub fn success(text: &str) -> Self {
            Self {
                fail_load_name: Some("invalid.bin"),
                fail_chunk: false,
                hold_ms: 0,
                race_complete_then_cancel: Arc::new(AtomicBool::new(false)),
                calls: Arc::new(Mutex::new(Vec::new())),
                text: text.to_string(),
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

        pub fn race_cancel(flag: Arc<AtomicBool>) -> Self {
            let mut engine = Self::success("maybe");
            engine.hold_ms = 50;
            engine.race_complete_then_cancel = flag;
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

        fn transcribe_chunk(
            &mut self,
            samples_f32_16k_mono: &[f32],
            cancel: &Arc<AtomicBool>,
            on_progress: Arc<dyn Fn(u8) + Send + Sync>,
        ) -> Result<Vec<InferenceSegment>, AppError> {
            self.calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(samples_f32_16k_mono.len());
            if self.hold_ms > 0 {
                let steps = (self.hold_ms / 20).max(1);
                for _ in 0..steps {
                    if cancel.load(Ordering::Acquire) {
                        return Err(AppError::message("transcription cancelled", "cancel"));
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
            if self.race_complete_then_cancel.swap(false, Ordering::AcqRel) {
                on_progress(100);
                if cancel.load(Ordering::Acquire) {
                    return Err(AppError::message("transcription cancelled", "cancel"));
                }
                return Ok(vec![InferenceSegment {
                    start_ms: 0,
                    end_ms: 10,
                    text: self.text.clone(),
                }]);
            }
            if self.fail_chunk {
                return Err(AppError::message("inference", "engine boom"));
            }
            if cancel.load(Ordering::Acquire) {
                return Err(AppError::message("transcription cancelled", "cancel"));
            }
            on_progress(50);
            on_progress(100);
            Ok(vec![InferenceSegment {
                start_ms: 0,
                end_ms: 500,
                text: self.text.clone(),
            }])
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
            TranscriptionConfig {
                window: Duration::from_millis(config.window_ms),
                overlap: Duration::from_millis(config.overlap_ms),
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
            if let Some(job) = host.snapshot().job_id {
                host.cancel(job);
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
            TranscriptionConfig {
                window: Duration::from_millis(config.window_ms),
                overlap: Duration::from_millis(config.overlap_ms),
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
}
