//! macOS host: the only module that may call AppKit / Foundation / Core Audio.
//!
//! Phase 1 scaffold: every adapter is a stub so the crate compiles and the default
//! tests run on macOS. Real adapters replace these one at a time (see
//! `.specs/skrivli-implementation-plan.md`, macOS port).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::common::error::AppError;
use crate::common::paths::AppPaths;
use crate::domain::audio::CivilTime;
use crate::domain::ids::RecordingId;
use crate::infrastructure::audio::devices::DeviceCatalog;
use crate::services::ports::{
    ActivationWatch, DesktopShell, DirectoryWatch, FreeSpace, HostCommand, InstanceAcquire,
    PerformanceCounter, PrimaryInstance, RecordingClock, SharedTrayText, SingleInstance,
    TranscriptNotifier, Trash, Tray, WatchEvent,
};

/// How often the stub loops re-check their stop flag.
const STUB_SLICE: Duration = Duration::from_millis(100);

pub(crate) struct PlatformHost {
    pub(crate) shell: Arc<dyn DesktopShell>,
    pub(crate) trash: Arc<dyn Trash>,
    pub(crate) watch: Arc<dyn DirectoryWatch>,
    pub(crate) single_instance: Arc<dyn SingleInstance>,
    pub(crate) tray: Arc<dyn Tray>,
}

pub(crate) fn host() -> PlatformHost {
    PlatformHost {
        shell: Arc::new(StubShell),
        trash: Arc::new(StubTrash),
        watch: Arc::new(StubDirectoryWatch),
        single_instance: Arc::new(StubSingleInstance),
        tray: Arc::new(StubTray),
    }
}

fn unsupported(context: &'static str) -> AppError {
    AppError::message(context, "unsupported on macOS")
}

struct StubShell;

impl DesktopShell for StubShell {
    fn set_window_visible(&self, _visible: bool) -> bool {
        false
    }

    fn window_is_minimized(&self) -> bool {
        false
    }

    fn pick_recordings_folder(&self) -> Result<Option<PathBuf>, AppError> {
        Err(unsupported("open recordings folder"))
    }

    fn pick_model_file(&self) -> Result<Option<PathBuf>, AppError> {
        Err(unsupported("open recordings folder"))
    }

    fn open_recordings_folder(&self, _path: &Path) -> Result<(), AppError> {
        Err(unsupported("open recordings folder"))
    }
}

struct StubTrash;

impl Trash for StubTrash {
    fn discard(&self, _paths: &[PathBuf]) -> Result<(), AppError> {
        Err(unsupported("library delete"))
    }
}

/// Reports no change events; the library falls back to its periodic refresh.
struct StubDirectoryWatch;

impl DirectoryWatch for StubDirectoryWatch {
    fn spawn(
        &self,
        _directory: PathBuf,
        stop: Arc<AtomicBool>,
        _debounce: Duration,
    ) -> (Receiver<WatchEvent>, JoinHandle<()>) {
        let (tx, rx) = channel();
        let handle = thread::spawn(move || {
            let mut since_poll = Duration::ZERO;
            while !stop.load(Ordering::Acquire) {
                thread::sleep(STUB_SLICE);
                since_poll += STUB_SLICE;
                if since_poll >= Duration::from_secs(2) {
                    since_poll = Duration::ZERO;
                    if tx.send(WatchEvent::Poll).is_err() {
                        break;
                    }
                }
            }
        });
        (rx, handle)
    }
}

struct StubSingleInstance;

impl SingleInstance for StubSingleInstance {
    fn acquire(&self) -> Result<InstanceAcquire, AppError> {
        Ok(InstanceAcquire::Primary {
            guard: Box::new(StubPrimary),
            activation: Box::new(StubActivation),
        })
    }
}

struct StubPrimary;

impl PrimaryInstance for StubPrimary {}

struct StubActivation;

impl ActivationWatch for StubActivation {
    fn wait_timeout(&self, timeout: Duration) -> bool {
        thread::sleep(timeout);
        false
    }
}

struct StubTray;

impl Tray for StubTray {
    fn run(
        &self,
        stop: Arc<AtomicBool>,
        _recording: Arc<AtomicBool>,
        notices: Receiver<RecordingId>,
        _text: SharedTrayText,
        _on_command: Box<dyn Fn(HostCommand) + Send>,
    ) -> Result<(), AppError> {
        while !stop.load(Ordering::Acquire) {
            let _ = notices.recv_timeout(STUB_SLICE);
        }
        Ok(())
    }
}

struct QueuedNotifier {
    tx: Sender<RecordingId>,
}

impl TranscriptNotifier for QueuedNotifier {
    fn transcript_ready(&self, recording_id: RecordingId) {
        let _ = self.tx.send(recording_id);
    }
}

pub(crate) fn completion_notifier(tx: Sender<RecordingId>) -> Arc<dyn TranscriptNotifier> {
    Arc::new(QueuedNotifier { tx })
}

pub(crate) fn replace_file(
    replacement: &Path,
    replaced: &Path,
    context: &'static str,
) -> Result<(), AppError> {
    // rename(2) replaces an existing target atomically on APFS. Settings, sidecars and
    // recordings are all written through this call, so it cannot stay a stub.
    std::fs::rename(replacement, replaced).map_err(|error| AppError::io(context, error))
}

pub(crate) fn user_paths() -> Result<AppPaths, AppError> {
    Err(unsupported("app paths"))
}

pub(crate) fn spawn_device_watch(
    _emit: Box<dyn Fn(DeviceCatalog) + Send>,
    stop: Arc<AtomicBool>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        while !stop.load(Ordering::Acquire) {
            thread::sleep(STUB_SLICE);
        }
    })
}

struct StubClock;

impl RecordingClock for StubClock {
    fn unix_millis(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
            .unwrap_or(0)
    }

    fn local(&self) -> CivilTime {
        CivilTime {
            year: 1970,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
        }
    }
}

pub(crate) fn clock() -> Box<dyn RecordingClock> {
    Box::new(StubClock)
}

struct StubDisk;

impl FreeSpace for StubDisk {
    fn available_bytes(&self, _directory: &Path) -> Result<u64, AppError> {
        Err(unsupported("free space"))
    }
}

pub(crate) fn disk() -> Box<dyn FreeSpace> {
    Box::new(StubDisk)
}

struct StubCounter;

impl PerformanceCounter for StubCounter {
    fn sample(&self) -> (u64, u64) {
        (0, 0)
    }
}

pub(crate) fn performance_counter() -> Box<dyn PerformanceCounter> {
    Box::new(StubCounter)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::StubTrash;
    use crate::services::ports::Trash;

    #[test]
    fn stub_trash_returns_unsupported_error() {
        // Arrange
        let trash = StubTrash;
        let paths = [PathBuf::from("recording.flac")];

        // Act
        let result = trash.discard(&paths);

        // Assert
        let error = result.expect_err("stub trash must refuse");
        assert!(format!("{error:?}").contains("unsupported on macOS"));
    }
}
