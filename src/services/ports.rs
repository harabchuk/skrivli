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
