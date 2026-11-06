//! Injected boundaries for OS adapters and recording I/O.
//!
//! Concrete Windows types live under `infrastructure/platform/windows/`.
//! Callers depend on these traits and on the free functions re-exported from
//! `infrastructure::platform`.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use crate::common::error::AppError;
use crate::domain::audio::{AudioBlock, CivilTime, SessionOrigin};

/// Debounce used by the production directory watcher.
pub(crate) const DEFAULT_WATCH_DEBOUNCE: Duration = Duration::from_millis(350);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CaptureError {
    PrivacyDenied,
    Unavailable,
    UnsupportedFormat,
    Failed,
}

pub(crate) trait CaptureStream: Send {
    /// The next block, if any. After `close`, this returns the blocks still
    /// buffered and then `Ok(None)` without waiting.
    fn pull(&mut self) -> Result<Option<AudioBlock>, CaptureError>;
    fn close(&mut self);
    fn overruns(&self) -> u64;
}

pub(crate) trait CapturePort: Send {
    fn open(
        &mut self,
        endpoint_id: &str,
        origin: SessionOrigin,
        drift_error: Arc<std::sync::atomic::AtomicI64>,
    ) -> Result<Box<dyn CaptureStream>, CaptureError>;
}

/// High-resolution ticks used to build the session origin and to decide mix lateness.
pub(crate) trait PerformanceCounter: Send + Sync {
    fn sample(&self) -> (u64, u64);
}

pub(crate) trait RecordingClock: Send {
    fn unix_millis(&self) -> u64;
    fn local(&self) -> CivilTime;
}

pub(crate) trait FreeSpace: Send {
    fn available_bytes(&self, directory: &Path) -> Result<u64, AppError>;
}

pub(crate) trait DesktopShell: Send + Sync {
    fn set_window_visible(&self, visible: bool) -> bool;
    fn window_is_minimized(&self) -> bool;
    fn pick_recordings_folder(&self) -> Result<Option<PathBuf>, AppError>;
    fn open_recordings_folder(&self, path: &Path) -> Result<(), AppError>;
}

pub(crate) trait Trash: Send + Sync {
    fn discard(&self, paths: &[PathBuf]) -> Result<(), AppError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WatchEvent {
    /// Debounced directory change; caller should rescan.
    Changed,
    /// Periodic fallback when notifications are quiet or unavailable.
    Poll,
}

pub(crate) trait DirectoryWatch: Send + Sync {
    fn spawn(
        &self,
        directory: PathBuf,
        stop: Arc<AtomicBool>,
        debounce: Duration,
    ) -> (Receiver<WatchEvent>, JoinHandle<()>);
}

/// Commands the tray may emit into the app bridge. Mapped to `AppCommand` at the edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HostCommand {
    Show,
    ToggleVisibility,
    StartRecording,
    StopRecording,
    OpenRecordingsFolder,
    OpenSettings,
    Exit,
    SessionEnding,
}

pub(crate) trait Tray: Send + Sync {
    fn run(
        &self,
        stop: Arc<AtomicBool>,
        recording: Arc<AtomicBool>,
        on_command: Box<dyn Fn(HostCommand) + Send>,
    ) -> Result<(), AppError>;
}

/// Result of trying to become the single running instance.
pub(crate) enum InstanceAcquire {
    /// Owns the OS lock. Drop `guard` on the acquiring thread. Not `Send`.
    /// `activation` is waited on a listener thread.
    Primary {
        guard: Box<dyn PrimaryInstance>,
        activation: Box<dyn ActivationWatch>,
    },
    /// Another process holds the lock; that process has been asked to show itself.
    Secondary,
}

/// Guard for the winning process. Drop releases the OS lock on this thread.
pub(crate) trait PrimaryInstance {}

/// Second-process show signal. Safe to wait from a dedicated listener thread.
pub(crate) trait ActivationWatch: Send {
    fn wait_timeout(&self, timeout: Duration) -> bool;
}

pub(crate) trait SingleInstance: Send + Sync {
    fn acquire(&self) -> Result<InstanceAcquire, AppError>;
}

/// One transcribed segment as plain values. No whisper.cpp types cross this boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InferenceSegment {
    pub(crate) start_ms: u64,
    pub(crate) end_ms: u64,
    pub(crate) text: String,
}

/// Local speech inference. The real adapter lives in `infrastructure/ai/whisper.rs`.
/// Progress is 0–100. Cancel is cooperative via `cancel`; implementations must not
/// expose engine pointers or log transcript text / audio samples.
pub(crate) trait InferencePort: Send {
    fn load(&mut self, model_path: &Path) -> Result<(), AppError>;

    fn unload(&mut self);

    fn transcribe_chunk(
        &mut self,
        samples_f32_16k_mono: &[f32],
        cancel: &Arc<AtomicBool>,
        on_progress: Arc<dyn Fn(u8) + Send + Sync>,
    ) -> Result<Vec<InferenceSegment>, AppError>;
}

#[cfg(test)]
mod inference_port_tests {
    use super::{InferencePort, InferenceSegment};
    use crate::common::error::AppError;
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    struct FakeInference {
        loaded: bool,
        fail_load: bool,
        segments: Vec<InferenceSegment>,
    }

    impl InferencePort for FakeInference {
        fn load(&mut self, _model_path: &Path) -> Result<(), AppError> {
            if self.fail_load {
                return Err(AppError::message("model unsupported", "fake reject"));
            }
            self.loaded = true;
            Ok(())
        }

        fn unload(&mut self) {
            self.loaded = false;
        }

        fn transcribe_chunk(
            &mut self,
            samples_f32_16k_mono: &[f32],
            cancel: &Arc<AtomicBool>,
            on_progress: Arc<dyn Fn(u8) + Send + Sync>,
        ) -> Result<Vec<InferenceSegment>, AppError> {
            if !self.loaded {
                return Err(AppError::message("inference", "model not loaded"));
            }
            if cancel.load(Ordering::Acquire) {
                return Err(AppError::message("transcription cancelled", "cancel"));
            }
            if samples_f32_16k_mono.is_empty() {
                on_progress(100);
                return Ok(Vec::new());
            }
            on_progress(50);
            on_progress(100);
            Ok(self.segments.clone())
        }
    }

    #[test]
    fn fake_port_reports_progress_and_segments() {
        // Arrange
        let mut fake = FakeInference {
            loaded: false,
            fail_load: false,
            segments: vec![InferenceSegment {
                start_ms: 0,
                end_ms: 1_000,
                text: "hello".into(),
            }],
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let percents = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&percents);

        // Act
        fake.load(Path::new("ggml-base-q5_1.bin")).expect("load");
        let segments = fake
            .transcribe_chunk(
                &[0.0; 160],
                &cancel,
                Arc::new(move |p| sink.lock().expect("lock").push(p)),
            )
            .expect("transcribe");

        // Assert
        assert_eq!(*percents.lock().expect("lock"), vec![50, 100]);
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].text, "hello");
        fake.unload();
        assert!(!fake.loaded);
    }

    #[test]
    fn fake_port_honors_cancel_flag() {
        // Arrange
        let mut fake = FakeInference {
            loaded: true,
            fail_load: false,
            segments: Vec::new(),
        };
        let cancel = Arc::new(AtomicBool::new(true));
        let progress = Arc::new(AtomicBool::new(false));
        let saw_progress = Arc::clone(&progress);

        // Act
        let error = fake
            .transcribe_chunk(
                &[0.0],
                &cancel,
                Arc::new(move |_| {
                    saw_progress.store(true, Ordering::Release);
                }),
            )
            .expect_err("cancelled");

        // Assert
        assert_eq!(error.user_message(), "Transcription was cancelled.");
        assert!(!progress.load(Ordering::Acquire));
    }
}
