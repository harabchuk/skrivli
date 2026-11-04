//! Microphone recording session.
//!
//! `Idle → Starting → Recording → Stopping → Idle`, or `Error` when startup
//! fails. Stop is idempotent. The capture port is injected, so tests never
//! open WASAPI. The host thread owns the session; the GPUI thread only sends
//! a command.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::common::error::AppError;
use crate::domain::audio::CivilTime;
use crate::domain::ids::RecordingId;
use crate::infrastructure::audio::queue::AudioBlock;
use crate::infrastructure::files::sidecar::{write_sidecar, Sidecar};
use crate::infrastructure::files::wav::{WavWrite, WavWriter};

/// Free space required before a recording starts.
pub(crate) const FREE_SPACE_FLOOR_BYTES: u64 = 200 * 1024 * 1024;

pub(crate) const NOTICE_SAVED: &str = "Recording saved.";
pub(crate) const NOTICE_PRIVACY: &str =
    "Microphone access is turned off in Windows privacy settings.";
pub(crate) const NOTICE_MIC: &str = "The microphone is not available.";
pub(crate) const NOTICE_FORMAT: &str = "The microphone format is not supported.";
pub(crate) const NOTICE_START: &str = "Could not start recording.";
pub(crate) const NOTICE_FOLDER: &str = "The recordings folder is not writable.";
pub(crate) const NOTICE_DISK: &str = "There is not enough free disk space to record.";
pub(crate) const NOTICE_SAVE: &str = "Could not save the recording.";
pub(crate) const NOTICE_LIMIT: &str = "Recording stopped because the file reached its size limit.";
pub(crate) const NOTICE_FALLBACK: &str =
    "The selected microphone is unavailable. Using the Windows default.";
pub(crate) const NOTICE_RECOVERED: &str = "A previous recording was recovered.";
pub(crate) const NOTICE_RECOVER_FAILED: &str = "A previous recording could not be recovered.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RecordPhase {
    Idle,
    Starting,
    Recording,
    Stopping,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RecordSnapshot {
    pub(crate) phase: RecordPhase,
    pub(crate) notice: Option<&'static str>,
    pub(crate) id: Option<RecordingId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CaptureError {
    PrivacyDenied,
    Unavailable,
    UnsupportedFormat,
    Failed,
}

pub(crate) fn capture_notice(error: CaptureError) -> &'static str {
    match error {
        CaptureError::PrivacyDenied => NOTICE_PRIVACY,
        CaptureError::Unavailable => NOTICE_MIC,
        CaptureError::UnsupportedFormat => NOTICE_FORMAT,
        CaptureError::Failed => NOTICE_START,
    }
}

pub(crate) trait CaptureStream: Send {
    fn pull(&mut self) -> Result<Option<AudioBlock>, CaptureError>;
    fn close(&mut self);
    fn overruns(&self) -> u64;
}

pub(crate) trait CapturePort: Send {
    fn open(&mut self, endpoint_id: &str) -> Result<Box<dyn CaptureStream>, CaptureError>;
}

pub(crate) trait RecordingClock: Send {
    fn unix_millis(&self) -> u64;
    fn local(&self) -> CivilTime;
}

pub(crate) trait FreeSpace: Send {
    fn available_bytes(&self, directory: &Path) -> Result<u64, AppError>;
}

pub(crate) struct StartRequest {
    pub(crate) directory: PathBuf,
    pub(crate) endpoint_id: String,
    pub(crate) microphone_id: String,
}

enum HostCommand {
    Start(StartRequest),
    Stop,
    Shutdown,
}

pub(crate) struct RecordingHost {
    tx: Sender<HostCommand>,
    stop: Arc<AtomicBool>,
    workers: Arc<AtomicUsize>,
}

impl Clone for RecordingHost {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            stop: Arc::clone(&self.stop),
            workers: Arc::clone(&self.workers),
        }
    }
}

impl RecordingHost {
    pub(crate) fn start(&self, request: StartRequest) {
        if self.workers.load(Ordering::Acquire) > 0 {
            return;
        }
        self.stop.store(false, Ordering::Release);
        let _ = self.tx.send(HostCommand::Start(request));
    }

    pub(crate) fn stop(&self) {
        self.stop.store(true, Ordering::Release);
        let _ = self.tx.send(HostCommand::Stop);
    }

    pub(crate) fn shutdown(&self) {
        self.stop.store(true, Ordering::Release);
        let _ = self.tx.send(HostCommand::Shutdown);
    }

    pub(crate) fn workers(&self) -> usize {
        self.workers.load(Ordering::Acquire)
    }
}

pub(crate) fn spawn_recording_host(
    mut port: Box<dyn CapturePort>,
    clock: Box<dyn RecordingClock>,
    space: Box<dyn FreeSpace>,
    emit: Box<dyn Fn(RecordSnapshot) + Send>,
    tray_recording: Arc<AtomicBool>,
) -> (RecordingHost, JoinHandle<()>) {
    let (tx, rx) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let workers = Arc::new(AtomicUsize::new(0));
    let host = RecordingHost {
        tx,
        stop: Arc::clone(&stop),
        workers: Arc::clone(&workers),
    };
    let thread_stop = Arc::clone(&stop);
    let handle = thread::spawn(move || {
        host_loop(
            rx,
            &mut *port,
            &*clock,
            &*space,
            &*emit,
            &thread_stop,
            &workers,
            &tray_recording,
        );
    });
    (host, handle)
}

#[allow(clippy::too_many_arguments)]
fn host_loop(
    rx: Receiver<HostCommand>,
    port: &mut dyn CapturePort,
    clock: &dyn RecordingClock,
    space: &dyn FreeSpace,
    emit: &dyn Fn(RecordSnapshot),
    stop: &AtomicBool,
    workers: &AtomicUsize,
    tray_recording: &AtomicBool,
) {
    loop {
        match rx.recv() {
            Ok(HostCommand::Start(request)) => {
                if stop.load(Ordering::Acquire) {
                    continue;
                }
                workers.fetch_add(1, Ordering::AcqRel);
                tray_recording.store(true, Ordering::Release);
                let snapshot = run_session(port, clock, space, &request, stop, emit);
                workers.fetch_sub(1, Ordering::AcqRel);
                tray_recording.store(false, Ordering::Release);
                emit(snapshot);
            }
            Ok(HostCommand::Stop) => {}
            Ok(HostCommand::Shutdown) | Err(_) => break,
        }
    }
}

pub(crate) fn run_session(
    port: &mut dyn CapturePort,
    clock: &dyn RecordingClock,
    space: &dyn FreeSpace,
    request: &StartRequest,
    stop: &AtomicBool,
    emit: &dyn Fn(RecordSnapshot),
) -> RecordSnapshot {
    emit(RecordSnapshot {
        phase: RecordPhase::Starting,
        notice: None,
        id: None,
    });
    match session_body(port, clock, space, request, stop, emit) {
        Ok(snapshot) => snapshot,
        Err(notice) => RecordSnapshot {
            phase: RecordPhase::Error,
            notice: Some(notice),
            id: None,
        },
    }
}

fn session_body(
    port: &mut dyn CapturePort,
    clock: &dyn RecordingClock,
    space: &dyn FreeSpace,
    request: &StartRequest,
    stop: &AtomicBool,
    emit: &dyn Fn(RecordSnapshot),
) -> Result<RecordSnapshot, &'static str> {
    let available = space.available_bytes(&request.directory).map_err(|error| {
        tracing::error!(error = %error, "free space was not read");
        NOTICE_DISK
    })?;
    if available < FREE_SPACE_FLOOR_BYTES {
        return Err(NOTICE_DISK);
    }
    if let Err(error) = ensure_writable(&request.directory) {
        tracing::error!(error = %error, "recordings folder is not writable");
        return Err(NOTICE_FOLDER);
    }
    let started_ms = clock.unix_millis();
    let when = clock.local();
    let mut wav = match WavWriter::create(&request.directory, when) {
        Ok(wav) => wav,
        Err(error) => {
            tracing::error!(error = %error, "recording file was not created");
            return Err(error.user_message());
        }
    };
    let mut stream = match port.open(&request.endpoint_id) {
        Ok(stream) => stream,
        Err(error) => {
            tracing::error!("microphone capture did not start");
            if let Err(abandon) = wav.abandon() {
                tracing::error!(error = %abandon, "partial recording was not removed");
            }
            return Err(capture_notice(error));
        }
    };
    let id = RecordingId::from_raw(started_ms.max(1));
    emit(RecordSnapshot {
        phase: RecordPhase::Recording,
        notice: None,
        id: Some(id),
    });
    let mut frames = 0_u64;
    let mut problems = Vec::new();
    let mut failed: Option<&'static str> = None;
    let mut limited = false;
    loop {
        match stream.pull() {
            Ok(Some(block)) => {
                note_problem(&mut problems, &block);
                let samples = samples_for_block(&block, frames, &mut problems);
                match wav.write_samples(&samples) {
                    Ok(WavWrite::Wrote { frames: written }) => {
                        frames = frames.saturating_add(written);
                    }
                    Ok(WavWrite::SafeStop { frames: written }) => {
                        frames = frames.saturating_add(written);
                        note(&mut problems, "safe_stop");
                        limited = true;
                        break;
                    }
                    Err(error) => {
                        tracing::error!(error = %error, "recording write failed");
                        failed = Some(NOTICE_SAVE);
                        break;
                    }
                }
                if limited {
                    break;
                }
            }
            Ok(None) => {
                if stop.load(Ordering::Acquire) {
                    break;
                }
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) => {
                tracing::error!("capture stopped during recording");
                note(&mut problems, "capture");
                failed = Some(capture_notice(error));
                break;
            }
        }
    }
    emit(RecordSnapshot {
        phase: RecordPhase::Stopping,
        notice: None,
        id: Some(id),
    });
    if stream.overruns() > 0 {
        note(&mut problems, "overrun");
    }
    stream.close();
    if let Some(notice) = failed {
        let _ = wav.finalize();
        return Err(notice);
    }
    let path = match wav.finalize() {
        Ok(path) => path,
        Err(error) => {
            tracing::error!(error = %error, "recording was not finalized");
            return Err(NOTICE_SAVE);
        }
    };
    let ended_ms = clock.unix_millis().max(started_ms);
    let duration_ms =
        frames.saturating_mul(1_000) / u64::from(crate::domain::audio::INTERNAL_SAMPLE_RATE);
    let sidecar = Sidecar {
        id: id.raw(),
        started_at_unix_ms: started_ms,
        ended_at_unix_ms: ended_ms,
        duration_ms,
        microphone_id: request.microphone_id.clone(),
        problems,
    };
    if let Err(error) = write_sidecar(&path, &sidecar) {
        tracing::error!(error = %error, "recording sidecar was not written");
        return Err(NOTICE_SAVE);
    }
    let notice = if limited { NOTICE_LIMIT } else { NOTICE_SAVED };
    Ok(RecordSnapshot {
        phase: RecordPhase::Idle,
        notice: Some(notice),
        id: Some(id),
    })
}

fn samples_for_block(block: &AudioBlock, frames: u64, problems: &mut Vec<String>) -> Vec<f32> {
    let mut output = Vec::new();
    let mut samples = block.samples.as_slice();
    if let Some(index) = block.timeline_frame {
        if index > frames {
            let gap = (index - frames).min(48_000 * 5);
            note(problems, "gap");
            output.resize((gap as usize).saturating_mul(2), 0.0);
        } else if index < frames {
            let skip = ((frames - index) as usize).saturating_mul(2);
            let skip = skip.min(samples.len());
            samples = &samples[skip..];
        }
    }
    output.extend_from_slice(samples);
    output
}

fn note_problem(problems: &mut Vec<String>, block: &AudioBlock) {
    if block.discontinuity {
        note(problems, "discontinuity");
    }
}

fn note(problems: &mut Vec<String>, code: &str) {
    if problems.iter().any(|problem| problem == code) {
        return;
    }
    problems.push(code.to_string());
}

pub(crate) fn ensure_writable(directory: &Path) -> Result<(), AppError> {
    std::fs::create_dir_all(directory)
        .map_err(|source| AppError::io("recordings folder", source))?;
    let probe = directory.join(".skrivli-write-probe");
    std::fs::write(&probe, b"ok").map_err(|source| AppError::io("recordings folder", source))?;
    let _ = std::fs::remove_file(&probe);
    Ok(())
}

/// Shared fake capture. `open` takes the queued blocks and a stand-in thread
/// that `close` joins, so a failed start cannot leak that thread.
#[derive(Clone)]
struct SharedCapture {
    inner: Arc<Mutex<FakeState>>,
    live: Arc<AtomicUsize>,
}

struct FakeState {
    error: Option<CaptureError>,
    blocks: std::collections::VecDeque<AudioBlock>,
}

struct FakeStream {
    blocks: std::collections::VecDeque<AudioBlock>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    overruns: u64,
}

impl CapturePort for SharedCapture {
    fn open(&mut self, _endpoint_id: &str) -> Result<Box<dyn CaptureStream>, CaptureError> {
        let mut state = lock_fake(&self.inner);
        if let Some(error) = state.error.take() {
            return Err(error);
        }
        let blocks = std::mem::take(&mut state.blocks);
        drop(state);
        let stop = Arc::new(AtomicBool::new(false));
        let live = Arc::clone(&self.live);
        live.fetch_add(1, Ordering::AcqRel);
        let stop_thread = Arc::clone(&stop);
        let worker = thread::spawn(move || {
            while !stop_thread.load(Ordering::Acquire) {
                thread::sleep(Duration::from_millis(5));
            }
            live.fetch_sub(1, Ordering::AcqRel);
        });
        Ok(Box::new(FakeStream {
            blocks,
            stop,
            worker: Some(worker),
            overruns: 0,
        }))
    }
}

impl CaptureStream for FakeStream {
    fn pull(&mut self) -> Result<Option<AudioBlock>, CaptureError> {
        Ok(self.blocks.pop_front())
    }

    fn close(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }

    fn overruns(&self) -> u64 {
        self.overruns
    }
}

impl Drop for FakeStream {
    fn drop(&mut self) {
        self.close();
    }
}

struct SharedClock {
    unix_ms: u64,
    local: CivilTime,
}

impl RecordingClock for SharedClock {
    fn unix_millis(&self) -> u64 {
        self.unix_ms
    }

    fn local(&self) -> CivilTime {
        self.local
    }
}

struct SharedSpace {
    bytes: Arc<AtomicU64>,
}

impl FreeSpace for SharedSpace {
    fn available_bytes(&self, _directory: &Path) -> Result<u64, AppError> {
        Ok(self.bytes.load(Ordering::Acquire))
    }
}

fn lock_fake(mutex: &Mutex<FakeState>) -> std::sync::MutexGuard<'_, FakeState> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordingHarnessPhase {
    Idle,
    Starting,
    Recording,
    Stopping,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HarnessNotice {
    pub phase: RecordingHarnessPhase,
    pub notice: Option<&'static str>,
}

pub struct RecordingHarness {
    host: RecordingHost,
    join: Option<JoinHandle<()>>,
    capture: SharedCapture,
    space: Arc<AtomicU64>,
    events: Receiver<RecordSnapshot>,
    directory: PathBuf,
}

impl RecordingHarness {
    pub fn new(directory: &Path) -> Self {
        let capture = SharedCapture {
            inner: Arc::new(Mutex::new(FakeState {
                error: None,
                blocks: std::collections::VecDeque::new(),
            })),
            live: Arc::new(AtomicUsize::new(0)),
        };
        let space = Arc::new(AtomicU64::new(u64::MAX));
        let (tx, rx) = mpsc::channel();
        let (host, join) = spawn_recording_host(
            Box::new(capture.clone()),
            Box::new(SharedClock {
                unix_ms: 1_700_000_000_000,
                local: CivilTime {
                    year: 2026,
                    month: 11,
                    day: 4,
                    hour: 13,
                    minute: 5,
                    second: 6,
                },
            }),
            Box::new(SharedSpace {
                bytes: Arc::clone(&space),
            }),
            Box::new(move |snapshot| {
                let _ = tx.send(snapshot);
            }),
            Arc::new(AtomicBool::new(false)),
        );
        Self {
            host,
            join: Some(join),
            capture,
            space,
            events: rx,
            directory: directory.to_path_buf(),
        }
    }

    pub fn set_free_bytes(&self, bytes: u64) {
        self.space.store(bytes, Ordering::Release);
    }

    pub fn fail_open(&self, privacy_denied: bool) {
        let mut state = lock_fake(&self.capture.inner);
        state.error = Some(if privacy_denied {
            CaptureError::PrivacyDenied
        } else {
            CaptureError::Unavailable
        });
    }

    pub fn push_frames(&self, frames: usize, sample: f32) {
        let mut samples = Vec::with_capacity(frames.saturating_mul(2));
        for _ in 0..frames {
            samples.push(sample);
            samples.push(sample);
        }
        let mut state = lock_fake(&self.capture.inner);
        state.blocks.push_back(AudioBlock {
            samples,
            timeline_frame: None,
            discontinuity: false,
        });
    }

    pub fn start_default(&self) {
        self.host.start(StartRequest {
            directory: self.directory.clone(),
            endpoint_id: String::new(),
            microphone_id: "default".to_string(),
        });
    }

    pub fn stop(&self) {
        self.host.stop();
    }

    pub fn workers(&self) -> usize {
        self.host.workers()
    }

    pub fn capture_threads(&self) -> usize {
        self.capture.live.load(Ordering::Acquire)
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    pub fn wait_phase(&self, phase: RecordingHarnessPhase) -> Result<HarnessNotice, &'static str> {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return Err("timed out waiting for recording state");
            }
            match self.events.recv_timeout(remaining) {
                Ok(snapshot) if map_phase(snapshot.phase) == phase => {
                    return Ok(HarnessNotice {
                        phase,
                        notice: snapshot.notice,
                    });
                }
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) => {
                    return Err("timed out waiting for recording state");
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err("recording host stopped");
                }
            }
        }
    }
}

impl Drop for RecordingHarness {
    fn drop(&mut self) {
        self.host.shutdown();
        if let Some(handle) = self.join.take() {
            let _ = handle.join();
        }
    }
}

fn map_phase(phase: RecordPhase) -> RecordingHarnessPhase {
    match phase {
        RecordPhase::Idle => RecordingHarnessPhase::Idle,
        RecordPhase::Starting => RecordingHarnessPhase::Starting,
        RecordPhase::Recording => RecordingHarnessPhase::Recording,
        RecordPhase::Stopping => RecordingHarnessPhase::Stopping,
        RecordPhase::Error => RecordingHarnessPhase::Error,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use super::{
        capture_notice, ensure_writable, run_session, CaptureError, FreeSpace, RecordPhase,
        RecordingClock, StartRequest, NOTICE_DISK,
    };
    use crate::domain::audio::CivilTime;
    use crate::infrastructure::audio::queue::AudioBlock;

    struct OneShot {
        block: Option<AudioBlock>,
        opened: bool,
    }

    impl super::CapturePort for OneShot {
        fn open(&mut self, _: &str) -> Result<Box<dyn super::CaptureStream>, CaptureError> {
            self.opened = true;
            let block = self.block.take();
            Ok(Box::new(ShotStream {
                block,
                closed: false,
            }))
        }
    }

    struct ShotStream {
        block: Option<AudioBlock>,
        closed: bool,
    }

    impl super::CaptureStream for ShotStream {
        fn pull(&mut self) -> Result<Option<AudioBlock>, CaptureError> {
            Ok(self.block.take())
        }

        fn close(&mut self) {
            self.closed = true;
        }

        fn overruns(&self) -> u64 {
            0
        }
    }

    struct Clock;

    impl RecordingClock for Clock {
        fn unix_millis(&self) -> u64 {
            50
        }

        fn local(&self) -> CivilTime {
            CivilTime::new(2026, 1, 2, 3, 4, 5).expect("time")
        }
    }

    struct Space(u64);

    impl FreeSpace for Space {
        fn available_bytes(
            &self,
            _: &std::path::Path,
        ) -> Result<u64, crate::common::error::AppError> {
            Ok(self.0)
        }
    }

    #[test]
    fn low_space_does_not_open_capture() {
        // Arrange
        let dir = std::env::temp_dir().join(format!("skrivli-low-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let mut port = OneShot {
            block: None,
            opened: false,
        };
        let stop = AtomicBool::new(false);
        let request = StartRequest {
            directory: dir.clone(),
            endpoint_id: "mic".to_string(),
            microphone_id: "mic".to_string(),
        };

        // Act
        let snapshot = run_session(&mut port, &Clock, &Space(1024), &request, &stop, &|_| {});

        // Assert
        assert_eq!(snapshot.phase, RecordPhase::Error);
        assert_eq!(snapshot.notice, Some(NOTICE_DISK));
        assert!(!port.opened);
        assert!(capture_notice(CaptureError::PrivacyDenied).contains("privacy"));
        assert!(ensure_writable(&dir).is_ok());
        let _ = std::fs::remove_dir_all(dir);
    }
}
