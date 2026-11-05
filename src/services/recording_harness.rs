//! In-process recording harness with fake capture ports.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::common::error::AppError;
use crate::domain::audio::{AudioBlock, CivilTime, SessionOrigin};
use crate::domain::recording::RecordPhase;
use crate::infrastructure::audio::levels::{GainControl, LevelSnapshot};
use crate::services::lease::EndpointLease;
use crate::services::ports::{
    CaptureError, CapturePort, CaptureStream, FreeSpace, PerformanceCounter, RecordingClock,
};
use crate::services::recording::{
    spawn_recording_host, RecordSnapshot, RecordingEngine, RecordingHost, StartRequest,
};

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
    overruns: u64,
    /// When set, `pull` keeps producing blocks after the queued ones so stop
    /// must close the stream instead of waiting for silence.
    live: Option<(usize, f32)>,
    pull_error: Option<CaptureError>,
    panic_on_pull: bool,
    silent: bool,
}

struct FakeStream {
    shared: Arc<Mutex<FakeState>>,
    blocks: std::collections::VecDeque<AudioBlock>,
    live: Option<(usize, f32)>,
    next_frame: u64,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    overruns: u64,
}

impl CapturePort for SharedCapture {
    fn open(
        &mut self,
        _endpoint_id: &str,
        _origin: SessionOrigin,
        _drift_error: Arc<AtomicI64>,
    ) -> Result<Box<dyn CaptureStream>, CaptureError> {
        let mut state = lock_fake(&self.inner);
        if let Some(error) = state.error.take() {
            return Err(error);
        }
        let blocks = std::mem::take(&mut state.blocks);
        let live_tone = state.live;
        let overruns = state.overruns;
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
            shared: Arc::clone(&self.inner),
            blocks,
            live: live_tone,
            next_frame: 0,
            stop,
            worker: Some(worker),
            overruns,
        }))
    }
}

impl CaptureStream for FakeStream {
    fn pull(&mut self) -> Result<Option<AudioBlock>, CaptureError> {
        if self.worker.is_none() {
            return Ok(None);
        }
        {
            let mut state = lock_fake(&self.shared);
            if state.panic_on_pull {
                state.panic_on_pull = false;
                drop(state);
                panic!("injected capture worker panic");
            }
            if let Some(error) = state.pull_error.take() {
                return Err(error);
            }
            if state.silent {
                return Ok(None);
            }
            if self.live.is_none() {
                self.live = state.live;
            }
            while let Some(block) = state.blocks.pop_front() {
                self.blocks.push_back(block);
            }
        }
        if let Some(block) = self.blocks.pop_front() {
            return Ok(Some(block));
        }
        let Some((frames, sample)) = self.live else {
            return Ok(None);
        };
        let mut samples = Vec::with_capacity(frames.saturating_mul(2));
        for _ in 0..frames {
            samples.push(sample);
            samples.push(sample);
        }
        let timeline_frame = Some(self.next_frame);
        self.next_frame = self.next_frame.saturating_add(frames as u64);
        Ok(Some(AudioBlock {
            samples,
            timeline_frame,
            discontinuity: false,
        }))
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
    system: SharedCapture,
    space: Arc<AtomicU64>,
    write_fault: Arc<AtomicBool>,
    folder_fault: Arc<AtomicBool>,
    events: Receiver<RecordSnapshot>,
    directory: PathBuf,
    rescue_dir: PathBuf,
}

impl RecordingHarness {
    pub fn new(directory: &Path) -> Self {
        let rescue_dir = directory.join("rescue");
        let _ = std::fs::create_dir_all(&rescue_dir);
        let capture = SharedCapture {
            inner: Arc::new(Mutex::new(FakeState {
                error: None,
                blocks: std::collections::VecDeque::new(),
                overruns: 0,
                live: None,
                pull_error: None,
                panic_on_pull: false,
                silent: false,
            })),
            live: Arc::new(AtomicUsize::new(0)),
        };
        let system = SharedCapture {
            inner: Arc::new(Mutex::new(FakeState {
                error: None,
                blocks: std::collections::VecDeque::new(),
                overruns: 0,
                live: None,
                pull_error: None,
                panic_on_pull: false,
                silent: false,
            })),
            live: Arc::new(AtomicUsize::new(0)),
        };
        let space = Arc::new(AtomicU64::new(u64::MAX));
        let write_fault = Arc::new(AtomicBool::new(false));
        let folder_fault = Arc::new(AtomicBool::new(false));
        let device_wake = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let (host, join) = spawn_recording_host(
            RecordingEngine {
                mic: Box::new(capture.clone()),
                system: Box::new(system.clone()),
                clock: Box::new(SharedClock {
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
                space: Box::new(SharedSpace {
                    bytes: Arc::clone(&space),
                }),
                performance: Box::new(FixedCounter),
                levels: Arc::new(LevelSnapshot::new()),
                gain: Arc::new(GainControl::new(1.0, false)),
                lease: Arc::new(EndpointLease::new()),
                rescue_dir: rescue_dir.clone(),
                space_check_every: Duration::ZERO,
                write_fault: Arc::clone(&write_fault),
                folder_fault: Arc::clone(&folder_fault),
                device_wake,
                stall_after: Duration::from_millis(80),
            },
            Box::new(move |snapshot| {
                let _ = tx.send(snapshot);
            }),
            Arc::new(AtomicBool::new(false)),
        );
        Self {
            host,
            join: Some(join),
            capture,
            system,
            space,
            write_fault,
            folder_fault,
            events: rx,
            directory: directory.to_path_buf(),
            rescue_dir,
        }
    }

    pub fn set_free_bytes(&self, bytes: u64) {
        self.space.store(bytes, Ordering::Release);
    }

    pub fn inject_write_fault(&self) {
        self.write_fault.store(true, Ordering::Release);
    }

    pub fn inject_folder_fault(&self) {
        self.folder_fault.store(true, Ordering::Release);
    }

    pub fn signal_devices_changed(&self) {
        self.host.devices_changed();
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
        push_into(&self.capture, frames, sample);
    }

    pub fn push_system_frames(&self, frames: usize, sample: f32) {
        push_into(&self.system, frames, sample);
    }

    pub fn stream_mic(&self, frames: usize, sample: f32) {
        let mut state = lock_fake(&self.capture.inner);
        state.live = Some((frames, sample));
    }

    pub fn stream_system(&self, frames: usize, sample: f32) {
        let mut state = lock_fake(&self.system.inner);
        state.live = Some((frames, sample));
    }

    pub fn fail_system(&self) {
        let mut state = lock_fake(&self.system.inner);
        state.error = Some(CaptureError::Unavailable);
    }

    pub fn fail_mic_pull(&self) {
        let mut state = lock_fake(&self.capture.inner);
        state.pull_error = Some(CaptureError::Unavailable);
    }

    pub fn fail_next_mic_open(&self) {
        let mut state = lock_fake(&self.capture.inner);
        state.error = Some(CaptureError::Unavailable);
    }

    pub fn panic_mic_pull(&self) {
        let mut state = lock_fake(&self.capture.inner);
        state.panic_on_pull = true;
    }

    pub fn silent_mic(&self) {
        let mut state = lock_fake(&self.capture.inner);
        state.silent = true;
        state.live = None;
    }

    pub fn set_overruns(&self, count: u64) {
        let mut state = lock_fake(&self.capture.inner);
        state.overruns = count;
    }

    pub fn start_default(&self) {
        self.host.start(StartRequest {
            directory: self.directory.clone(),
            endpoint_id: String::new(),
            microphone_id: "default".to_string(),
            system_endpoint_id: Some(String::new()),
            system_audio_id: "system".to_string(),
            open_microphone: true,
            degraded_notice: None,
        });
    }

    pub fn stop(&self) {
        self.host.stop();
    }

    pub fn workers(&self) -> usize {
        self.host.workers()
    }

    pub fn capture_threads(&self) -> usize {
        self.capture
            .live
            .load(Ordering::Acquire)
            .saturating_add(self.system.live.load(Ordering::Acquire))
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    pub fn rescue_dir(&self) -> &Path {
        &self.rescue_dir
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

    pub fn wait_notice_contains(&self, text: &str) -> Result<HarnessNotice, &'static str> {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return Err("timed out waiting for notice");
            }
            match self.events.recv_timeout(remaining) {
                Ok(snapshot) => {
                    if snapshot.notice.is_some_and(|notice| notice.contains(text)) {
                        return Ok(HarnessNotice {
                            phase: map_phase(snapshot.phase),
                            notice: snapshot.notice,
                        });
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    return Err("timed out waiting for notice");
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

fn push_into(port: &SharedCapture, frames: usize, sample: f32) {
    let mut samples = Vec::with_capacity(frames.saturating_mul(2));
    for _ in 0..frames {
        samples.push(sample);
        samples.push(sample);
    }
    let mut state = lock_fake(&port.inner);
    state.blocks.push_back(AudioBlock {
        samples,
        timeline_frame: None,
        discontinuity: false,
    });
}

struct FixedCounter;

impl PerformanceCounter for FixedCounter {
    fn sample(&self) -> (u64, u64) {
        (0, 48_000)
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
