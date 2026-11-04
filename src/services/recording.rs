//! Microphone recording session.
//!
//! `Idle → Starting → Recording → Stopping → Idle`, or `Error` when startup
//! fails. Stop is idempotent. The capture port is injected, so tests never
//! open WASAPI. The host thread owns the session; the GPUI thread only sends
//! a command.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::common::error::AppError;
use crate::domain::audio::{CivilTime, SessionOrigin, INTERNAL_SAMPLE_RATE};
use crate::domain::ids::RecordingId;
use crate::infrastructure::audio::levels::{GainControl, LevelSnapshot};
use crate::infrastructure::audio::mixer::Mixer;
use crate::infrastructure::audio::queue::AudioBlock;
use crate::infrastructure::files::rescue::rescue_partial;
use crate::infrastructure::files::sidecar::{write_sidecar, Sidecar};
use crate::infrastructure::files::wav::{WavWrite, WavWriter};

use super::lease::EndpointLease;

/// Free space required before a recording starts and during periodic checks.
pub(crate) const FREE_SPACE_FLOOR_BYTES: u64 = 200 * 1024 * 1024;
/// How often an active session rechecks destination free space and writability.
pub(crate) const SPACE_CHECK_INTERVAL: Duration = Duration::from_secs(30);
/// Idle pulls without progress before a stall notice (injectable in tests).
pub(crate) const STALL_IDLE: Duration = Duration::from_secs(8);

pub(crate) const NOTICE_SAVED: &str = "Recording saved.";
pub(crate) const NOTICE_PRIVACY: &str =
    "Microphone access is turned off in Windows privacy settings.";
pub(crate) const NOTICE_MIC: &str = "The microphone is not available.";
pub(crate) const NOTICE_FORMAT: &str = "The microphone format is not supported.";
pub(crate) const NOTICE_START: &str = "Could not start recording.";
pub(crate) const NOTICE_FOLDER: &str = "The recordings folder is not writable.";
pub(crate) const NOTICE_DISK: &str = "There is not enough free disk space to record.";
pub(crate) const NOTICE_LOW_DISK: &str = "Recording stopped because free disk space is low.";
pub(crate) const NOTICE_SAVE: &str = "Could not save the recording.";
pub(crate) const NOTICE_LIMIT: &str = "Recording stopped because the file reached its size limit.";
pub(crate) const NOTICE_FALLBACK: &str =
    "The selected microphone is unavailable. Using the Windows default.";
pub(crate) const NOTICE_SYSTEM_FALLBACK: &str =
    "The selected output is unavailable. Using the Windows default.";
pub(crate) const NOTICE_SYSTEM_MISSING: &str =
    "System audio is unavailable. Recording the microphone with silence on that side.";
pub(crate) const NOTICE_MIC_MISSING: &str =
    "The microphone is unavailable. Recording system audio with silence on that side.";
pub(crate) const NOTICE_RECOVERED: &str = "A previous recording was recovered.";
pub(crate) const NOTICE_RECOVER_FAILED: &str = "A previous recording could not be recovered.";
pub(crate) const NOTICE_RESCUED: &str =
    "The recordings folder failed. A copy was saved to the rescue folder.";
pub(crate) const NOTICE_PARTIAL_KEPT: &str =
    "The recording could not be saved fully. A partial file was kept.";
pub(crate) const NOTICE_WORKER: &str = "Recording stopped because a capture worker failed.";
pub(crate) const NOTICE_STALL: &str = "Audio capture appears stalled.";

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
    fn open(
        &mut self,
        endpoint_id: &str,
        origin: SessionOrigin,
        drift_error: Arc<std::sync::atomic::AtomicI64>,
    ) -> Result<Box<dyn CaptureStream>, CaptureError>;
}

/// QPC sample used to build the session origin and to decide mix lateness.
pub(crate) trait PerformanceCounter: Send {
    fn sample(&self) -> (u64, u64);
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
    /// `None` skips loopback and records silence on the system side.
    pub(crate) system_endpoint_id: Option<String>,
    pub(crate) system_audio_id: String,
    pub(crate) open_microphone: bool,
    /// Shown instead of the saved notice when a chosen device was substituted.
    pub(crate) degraded_notice: Option<&'static str>,
}

pub(crate) struct RecordingEngine {
    pub(crate) mic: Box<dyn CapturePort>,
    pub(crate) system: Box<dyn CapturePort>,
    pub(crate) clock: Box<dyn RecordingClock>,
    pub(crate) space: Box<dyn FreeSpace>,
    pub(crate) performance: Box<dyn PerformanceCounter>,
    pub(crate) levels: Arc<LevelSnapshot>,
    pub(crate) gain: Arc<GainControl>,
    pub(crate) lease: Arc<EndpointLease>,
    /// Destination for bounded rescue copies when the recordings folder fails.
    pub(crate) rescue_dir: PathBuf,
    /// How often to recheck free space and writability while recording.
    pub(crate) space_check_every: Duration,
    /// When set, the next sample write is treated as a disk error (tests).
    pub(crate) write_fault: Arc<AtomicBool>,
    /// When set, periodic storage checks treat the destination as unwritable.
    pub(crate) folder_fault: Arc<AtomicBool>,
    /// Set by the shell when `IMMNotificationClient` wakes re-enumeration.
    pub(crate) device_wake: Arc<AtomicBool>,
    /// Idle duration without mix progress before a stall notice.
    pub(crate) stall_after: Duration,
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
    device_wake: Arc<AtomicBool>,
}

impl Clone for RecordingHost {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            stop: Arc::clone(&self.stop),
            workers: Arc::clone(&self.workers),
            device_wake: Arc::clone(&self.device_wake),
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

    /// Wake an active session so it can reopen a missing default endpoint.
    pub(crate) fn devices_changed(&self) {
        self.device_wake.store(true, Ordering::Release);
    }

    pub(crate) fn workers(&self) -> usize {
        self.workers.load(Ordering::Acquire)
    }
}

pub(crate) fn spawn_recording_host(
    mut engine: RecordingEngine,
    emit: Box<dyn Fn(RecordSnapshot) + Send>,
    tray_recording: Arc<AtomicBool>,
) -> (RecordingHost, JoinHandle<()>) {
    let (tx, rx) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let workers = Arc::new(AtomicUsize::new(0));
    let device_wake = Arc::clone(&engine.device_wake);
    let host = RecordingHost {
        tx,
        stop: Arc::clone(&stop),
        workers: Arc::clone(&workers),
        device_wake,
    };
    let thread_stop = Arc::clone(&stop);
    let handle = thread::spawn(move || {
        host_loop(
            rx,
            &mut engine,
            &*emit,
            &thread_stop,
            &workers,
            &tray_recording,
        );
    });
    (host, handle)
}

fn host_loop(
    rx: Receiver<HostCommand>,
    engine: &mut RecordingEngine,
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
                let snapshot = run_session(engine, &request, stop, emit);
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
    engine: &mut RecordingEngine,
    request: &StartRequest,
    stop: &AtomicBool,
    emit: &dyn Fn(RecordSnapshot),
) -> RecordSnapshot {
    emit(RecordSnapshot {
        phase: RecordPhase::Starting,
        notice: request.degraded_notice,
        id: None,
    });
    match session_body(engine, request, stop, emit) {
        Ok(snapshot) => snapshot,
        Err(notice) => RecordSnapshot {
            phase: RecordPhase::Error,
            notice: Some(notice),
            id: None,
        },
    }
}

struct LeaseGuard(Arc<EndpointLease>);

impl Drop for LeaseGuard {
    fn drop(&mut self) {
        self.0.release_recording();
    }
}

fn session_body(
    engine: &mut RecordingEngine,
    request: &StartRequest,
    stop: &AtomicBool,
    emit: &dyn Fn(RecordSnapshot),
) -> Result<RecordSnapshot, &'static str> {
    let available = engine
        .space
        .available_bytes(&request.directory)
        .map_err(|error| {
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
    let lease = Arc::clone(&engine.lease);
    lease.acquire_recording();
    let _lease = LeaseGuard(lease);
    let started_ms = engine.clock.unix_millis();
    let when = engine.clock.local();
    let mut wav = match WavWriter::create(&request.directory, when) {
        Ok(wav) => wav,
        Err(error) => {
            tracing::error!(error = %error, "recording file was not created");
            return Err(error.user_message());
        }
    };
    let (ticks, frequency) = engine.performance.sample();
    let origin = match SessionOrigin::new(ticks, frequency, INTERNAL_SAMPLE_RATE) {
        Some(origin) => origin,
        None => {
            let _ = wav.abandon();
            return Err(NOTICE_START);
        }
    };
    let mic_drift = Arc::new(AtomicI64::new(0));
    let system_drift = Arc::new(AtomicI64::new(0));
    let mic_result = if request.open_microphone {
        engine
            .mic
            .open(&request.endpoint_id, origin, Arc::clone(&mic_drift))
    } else {
        Err(CaptureError::Unavailable)
    };
    let system_result = match &request.system_endpoint_id {
        Some(id) => engine.system.open(id, origin, Arc::clone(&system_drift)),
        None => Err(CaptureError::Unavailable),
    };
    let mut degraded = request.degraded_notice;
    let mic_error = mic_result.as_ref().err().copied();
    let mut mic = match mic_result {
        Ok(stream) => Some(stream),
        Err(error) => {
            if system_result.is_err() {
                tracing::error!("microphone capture did not start");
                if let Err(abandon) = wav.abandon() {
                    tracing::error!(error = %abandon, "partial recording was not removed");
                }
                return Err(capture_notice(error));
            }
            degraded = Some(NOTICE_MIC_MISSING);
            let _ = error;
            None
        }
    };
    let want_system = request.system_endpoint_id.is_some();
    let mut system = match system_result {
        Ok(stream) => Some(stream),
        Err(error) => {
            if mic.is_none() {
                tracing::error!("system audio capture did not start");
                if let Err(abandon) = wav.abandon() {
                    tracing::error!(error = %abandon, "partial recording was not removed");
                }
                return Err(mic_error.map(capture_notice).unwrap_or(NOTICE_START));
            }
            tracing::error!("system audio capture did not start");
            let _ = error;
            if degraded.is_none() {
                degraded = Some(NOTICE_SYSTEM_MISSING);
            }
            None
        }
    };
    let mut microphone_id = request.microphone_id.clone();
    let mut system_audio_id = request.system_audio_id.clone();
    let want_mic = request.open_microphone;
    let id = RecordingId::from_raw(started_ms.max(1));
    emit(RecordSnapshot {
        phase: RecordPhase::Recording,
        notice: degraded,
        id: Some(id),
    });
    let mut mixer = Mixer::new(
        want_system,
        Arc::clone(&engine.gain),
        Arc::clone(&engine.levels),
        Arc::clone(&mic_drift),
        Arc::clone(&system_drift),
    );
    let mut frames = 0_u64;
    let mut problems = Vec::new();
    let mut failed: Option<&'static str> = None;
    let mut limited = false;
    let mut last_space_check = Instant::now();
    let mut last_progress = Instant::now();
    let mut last_reopen = Instant::now();
    let mut stall_noted = false;
    let mut wrote_any = false;
    loop {
        let stopping = stop.load(Ordering::Acquire);
        if !stopping && !limited {
            if let Some(notice) = recheck_storage(engine, &request.directory, &mut last_space_check)
            {
                note(&mut problems, "disk");
                failed = Some(notice);
                limited = true;
            }
            if engine.write_fault.load(Ordering::Acquire) {
                engine.write_fault.store(false, Ordering::Release);
                tracing::error!("recording write fault injected");
                note(&mut problems, "disk");
                failed = Some(NOTICE_SAVE);
                limited = true;
            }
        }
        if stopping || limited {
            if let Some(stream) = mic.as_mut() {
                stream.close();
            }
            if let Some(stream) = system.as_mut() {
                stream.close();
            }
        } else {
            let mic_before = mic.is_some();
            let system_before = system.is_some();
            let mic_packet = pull_alive(
                &mut mic,
                &mut problems,
                "capture",
                NOTICE_MIC_MISSING,
                &mut degraded,
                id,
                emit,
            );
            let system_packet = pull_alive(
                &mut system,
                &mut problems,
                "loopback",
                NOTICE_SYSTEM_MISSING,
                &mut degraded,
                id,
                emit,
            );
            let lost_source = (mic_before && mic.is_none()) || (system_before && system.is_none());
            if lost_source {
                note(&mut problems, "gap");
            }
            if let Some(block) = mic_packet {
                mixer.push_mic(block);
                last_progress = Instant::now();
            }
            if let Some(block) = system_packet {
                mixer.push_system(block);
                last_progress = Instant::now();
            }
            let wake = engine.device_wake.swap(false, Ordering::AcqRel);
            let missing = (want_mic && mic.is_none()) || (want_system && system.is_none());
            let reopen_due = missing && last_reopen.elapsed() >= Duration::from_secs(2);
            if wake || lost_source || reopen_due {
                try_reopen_sources(
                    engine,
                    origin,
                    &mic_drift,
                    &system_drift,
                    want_mic,
                    want_system,
                    &mut mic,
                    &mut system,
                    &mut microphone_id,
                    &mut system_audio_id,
                    &mut problems,
                    &mut degraded,
                    id,
                    emit,
                );
                last_reopen = Instant::now();
            }
            if !stall_noted
                && (mic.is_some() || system.is_some())
                && last_progress.elapsed() >= engine.stall_after
            {
                note(&mut problems, "stall");
                degraded = Some(NOTICE_STALL);
                stall_noted = true;
                emit(RecordSnapshot {
                    phase: RecordPhase::Recording,
                    notice: degraded,
                    id: Some(id),
                });
            }
        }
        let now = frame_now(origin, engine.performance.as_ref());
        let mut wrote = false;
        while let Some(mixed) = mixer.pull(now, stopping || limited) {
            debug_assert_eq!(mixed.frames as usize * 2, mixed.samples.len());
            match wav.write_samples(&mixed.samples) {
                Ok(WavWrite::Wrote { frames: written }) => {
                    frames = frames.saturating_add(written);
                    wrote = true;
                    wrote_any = true;
                    last_progress = Instant::now();
                }
                Ok(WavWrite::SafeStop { frames: written }) => {
                    frames = frames.saturating_add(written);
                    wrote_any = wrote_any || written > 0;
                    note(&mut problems, "safe_stop");
                    limited = true;
                    break;
                }
                Err(error) => {
                    tracing::error!(error = %error, "recording write failed");
                    failed = Some(NOTICE_SAVE);
                    limited = true;
                    break;
                }
            }
        }
        if limited {
            break;
        }
        let mic_open = mic.is_some();
        let system_open = system.is_some();
        if stopping && !wrote {
            break;
        }
        if !mic_open && !system_open && !wrote {
            if wrote_any || frames > 0 {
                break;
            }
            break;
        }
        if !wrote {
            thread::sleep(Duration::from_millis(5));
        }
    }
    emit(RecordSnapshot {
        phase: RecordPhase::Stopping,
        notice: degraded,
        id: Some(id),
    });
    let diagnostics = mixer.diagnostics();
    if diagnostics.gaps > 0 {
        note(&mut problems, "gap");
    }
    if diagnostics.discontinuities > 0 {
        note(&mut problems, "discontinuity");
    }
    if diagnostics.drift_adjustments > 0 {
        note(&mut problems, "drift");
    }
    let overruns = mic.as_ref().map(|stream| stream.overruns()).unwrap_or(0)
        + system.as_ref().map(|stream| stream.overruns()).unwrap_or(0);
    if overruns > 0 {
        note(&mut problems, "overrun");
    }
    if let Some(stream) = mic.as_mut() {
        stream.close();
    }
    if let Some(stream) = system.as_mut() {
        stream.close();
    }
    finish_session(
        wav,
        engine,
        &request.directory,
        id,
        started_ms,
        frames,
        microphone_id,
        system_audio_id,
        problems,
        failed,
        limited,
        degraded,
    )
}

fn recheck_storage(
    engine: &RecordingEngine,
    directory: &Path,
    last_check: &mut Instant,
) -> Option<&'static str> {
    if last_check.elapsed() < engine.space_check_every {
        return None;
    }
    *last_check = Instant::now();
    if engine.folder_fault.load(Ordering::Acquire) {
        tracing::error!("recordings folder became unwritable");
        return Some(NOTICE_FOLDER);
    }
    match engine.space.available_bytes(directory) {
        Ok(available) if available < FREE_SPACE_FLOOR_BYTES => {
            tracing::warn!("free space dropped below the recording floor");
            Some(NOTICE_LOW_DISK)
        }
        Err(error) => {
            tracing::error!(error = %error, "free space was not read during recording");
            Some(NOTICE_LOW_DISK)
        }
        Ok(_) => {
            if let Err(error) = ensure_writable(directory) {
                tracing::error!(error = %error, "recordings folder became unwritable");
                Some(NOTICE_FOLDER)
            } else {
                None
            }
        }
    }
}

fn pull_alive(
    stream: &mut Option<Box<dyn CaptureStream>>,
    problems: &mut Vec<String>,
    code: &str,
    lost_notice: &'static str,
    degraded: &mut Option<&'static str>,
    id: RecordingId,
    emit: &dyn Fn(RecordSnapshot),
) -> Option<AudioBlock> {
    let active = stream.as_mut()?;
    let pulled = catch_unwind(AssertUnwindSafe(|| active.pull()));
    match pulled {
        Ok(Ok(block)) => block,
        Ok(Err(error)) => {
            tracing::error!(?error, "capture stopped during recording");
            note(problems, code);
            active.close();
            *stream = None;
            *degraded = Some(match error {
                CaptureError::PrivacyDenied => NOTICE_PRIVACY,
                CaptureError::UnsupportedFormat => NOTICE_FORMAT,
                CaptureError::Unavailable | CaptureError::Failed => lost_notice,
            });
            if matches!(error, CaptureError::Failed) {
                note(problems, "worker");
                *degraded = Some(NOTICE_WORKER);
            }
            emit(RecordSnapshot {
                phase: RecordPhase::Recording,
                notice: *degraded,
                id: Some(id),
            });
            None
        }
        Err(_) => {
            tracing::error!("capture worker panicked");
            note(problems, "worker_panic");
            active.close();
            *stream = None;
            *degraded = Some(NOTICE_WORKER);
            emit(RecordSnapshot {
                phase: RecordPhase::Recording,
                notice: *degraded,
                id: Some(id),
            });
            None
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn try_reopen_sources(
    engine: &mut RecordingEngine,
    origin: SessionOrigin,
    mic_drift: &Arc<AtomicI64>,
    system_drift: &Arc<AtomicI64>,
    want_mic: bool,
    want_system: bool,
    mic: &mut Option<Box<dyn CaptureStream>>,
    system: &mut Option<Box<dyn CaptureStream>>,
    microphone_id: &mut String,
    system_audio_id: &mut String,
    problems: &mut Vec<String>,
    degraded: &mut Option<&'static str>,
    id: RecordingId,
    emit: &dyn Fn(RecordSnapshot),
) {
    let mut changed = false;
    if want_mic && mic.is_none() {
        match engine.mic.open("", origin, Arc::clone(mic_drift)) {
            Ok(stream) => {
                *mic = Some(stream);
                *microphone_id = "default".to_string();
                note(problems, "source_change");
                *degraded = if system.is_some() {
                    Some(NOTICE_FALLBACK)
                } else {
                    Some(NOTICE_MIC_MISSING)
                };
                changed = true;
            }
            Err(_) => {
                if *degraded != Some(NOTICE_PRIVACY) {
                    *degraded = Some(NOTICE_MIC_MISSING);
                }
            }
        }
    }
    if want_system && system.is_none() {
        match engine.system.open("", origin, Arc::clone(system_drift)) {
            Ok(stream) => {
                *system = Some(stream);
                *system_audio_id = "default".to_string();
                note(problems, "source_change");
                *degraded = if mic.is_some() {
                    Some(NOTICE_SYSTEM_FALLBACK)
                } else {
                    Some(NOTICE_SYSTEM_MISSING)
                };
                changed = true;
            }
            Err(_) => {
                if mic.is_some()
                    && *degraded != Some(NOTICE_PRIVACY)
                    && *degraded != Some(NOTICE_FALLBACK)
                {
                    *degraded = Some(NOTICE_SYSTEM_MISSING);
                }
            }
        }
    }
    if changed {
        emit(RecordSnapshot {
            phase: RecordPhase::Recording,
            notice: *degraded,
            id: Some(id),
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn finish_session(
    wav: WavWriter,
    engine: &RecordingEngine,
    directory: &Path,
    id: RecordingId,
    started_ms: u64,
    frames: u64,
    microphone_id: String,
    system_audio_id: String,
    problems: Vec<String>,
    failed: Option<&'static str>,
    limited: bool,
    degraded: Option<&'static str>,
) -> Result<RecordSnapshot, &'static str> {
    let folder_ok =
        !engine.folder_fault.load(Ordering::Acquire) && ensure_writable(directory).is_ok();
    if let Some(notice) = failed {
        return preserve_or_rescue(
            wav,
            engine,
            directory,
            folder_ok,
            id,
            started_ms,
            frames,
            microphone_id,
            system_audio_id,
            problems,
            notice,
        );
    }
    if !folder_ok {
        return preserve_or_rescue(
            wav,
            engine,
            directory,
            false,
            id,
            started_ms,
            frames,
            microphone_id,
            system_audio_id,
            problems,
            NOTICE_FOLDER,
        );
    }
    let path = match wav.finalize() {
        Ok(path) => path,
        Err(error) => {
            tracing::error!(error = %error, "recording was not finalized");
            return Err(NOTICE_SAVE);
        }
    };
    let ended_ms = engine.clock.unix_millis().max(started_ms);
    let duration_ms = frames.saturating_mul(1_000) / u64::from(INTERNAL_SAMPLE_RATE);
    let hit_size_limit = problems.iter().any(|problem| problem == "safe_stop");
    let sidecar = Sidecar {
        id: id.raw(),
        started_at_unix_ms: started_ms,
        ended_at_unix_ms: ended_ms,
        duration_ms,
        microphone_id,
        system_audio_id,
        problems,
    };
    if let Err(error) = write_sidecar(&path, &sidecar) {
        tracing::error!(error = %error, "recording sidecar was not written");
        return Err(NOTICE_SAVE);
    }
    let notice = if limited && (hit_size_limit || degraded.is_none()) {
        NOTICE_LIMIT
    } else {
        degraded.unwrap_or(NOTICE_SAVED)
    };
    Ok(RecordSnapshot {
        phase: RecordPhase::Idle,
        notice: Some(notice),
        id: Some(id),
    })
}

#[allow(clippy::too_many_arguments)]
fn preserve_or_rescue(
    wav: WavWriter,
    engine: &RecordingEngine,
    _directory: &Path,
    folder_ok: bool,
    id: RecordingId,
    started_ms: u64,
    frames: u64,
    microphone_id: String,
    system_audio_id: String,
    mut problems: Vec<String>,
    failure: &'static str,
) -> Result<RecordSnapshot, &'static str> {
    note(&mut problems, "save_error");
    let partial_hint = wav.partial_path().to_path_buf();
    if folder_ok {
        match wav.finalize() {
            Ok(path) => {
                let ended_ms = engine.clock.unix_millis().max(started_ms);
                let duration_ms = frames.saturating_mul(1_000) / u64::from(INTERNAL_SAMPLE_RATE);
                let sidecar = Sidecar {
                    id: id.raw(),
                    started_at_unix_ms: started_ms,
                    ended_at_unix_ms: ended_ms,
                    duration_ms,
                    microphone_id,
                    system_audio_id,
                    problems,
                };
                let _ = write_sidecar(&path, &sidecar);
                return Err(failure);
            }
            Err(error) => {
                tracing::error!(error = %error, "finalize after failure did not complete");
                return rescue_existing_partial(&partial_hint, &engine.rescue_dir, failure);
            }
        }
    }
    let partial = match wav.release_partial() {
        Ok(path) => path,
        Err(error) => {
            tracing::error!(error = %error, "partial recording handle was not released");
            return rescue_existing_partial(&partial_hint, &engine.rescue_dir, failure);
        }
    };
    rescue_existing_partial(&partial, &engine.rescue_dir, failure)
}

fn rescue_existing_partial(
    partial: &Path,
    rescue_dir: &Path,
    failure: &'static str,
) -> Result<RecordSnapshot, &'static str> {
    if !partial.exists() {
        tracing::error!("partial recording bytes are not readable");
        return Err(failure);
    }
    match rescue_partial(partial, rescue_dir) {
        Ok(_) => Err(NOTICE_RESCUED),
        Err(error) => {
            tracing::error!(error = %error, "rescue copy was not possible");
            Err(if partial.exists() {
                NOTICE_PARTIAL_KEPT
            } else {
                failure
            })
        }
    }
}

fn frame_now(origin: SessionOrigin, performance: &dyn PerformanceCounter) -> u64 {
    let (ticks, _) = performance.sample();
    origin.sample_index(ticks).unwrap_or(0)
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

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicI64};
    use std::sync::Arc;
    use std::time::Duration;

    use super::{
        capture_notice, ensure_writable, run_session, CaptureError, FreeSpace, RecordPhase,
        RecordingClock, StartRequest, NOTICE_DISK,
    };
    use crate::domain::audio::CivilTime;
    use crate::infrastructure::audio::queue::AudioBlock;

    struct OneShot {
        block: Option<AudioBlock>,
        opened: std::sync::Arc<AtomicBool>,
    }

    impl super::CapturePort for OneShot {
        fn open(
            &mut self,
            _: &str,
            _: crate::domain::audio::SessionOrigin,
            _: Arc<AtomicI64>,
        ) -> Result<Box<dyn super::CaptureStream>, CaptureError> {
            self.opened
                .store(true, std::sync::atomic::Ordering::Release);
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
        let mic_opened = std::sync::Arc::new(AtomicBool::new(false));
        let system_opened = std::sync::Arc::new(AtomicBool::new(false));
        let port = OneShot {
            block: None,
            opened: std::sync::Arc::clone(&mic_opened),
        };
        let system = OneShot {
            block: None,
            opened: std::sync::Arc::clone(&system_opened),
        };
        let stop = AtomicBool::new(false);
        let request = StartRequest {
            directory: dir.clone(),
            endpoint_id: "mic".to_string(),
            microphone_id: "mic".to_string(),
            system_endpoint_id: Some("system".to_string()),
            system_audio_id: "system".to_string(),
            open_microphone: true,
            degraded_notice: None,
        };
        let mut engine = super::RecordingEngine {
            mic: Box::new(port),
            system: Box::new(system),
            clock: Box::new(Clock),
            space: Box::new(Space(1024)),
            performance: Box::new(super::FixedCounter),
            levels: std::sync::Arc::new(crate::infrastructure::audio::levels::LevelSnapshot::new()),
            gain: std::sync::Arc::new(crate::infrastructure::audio::levels::GainControl::new(
                1.0, false,
            )),
            lease: std::sync::Arc::new(crate::services::lease::EndpointLease::new()),
            rescue_dir: dir.join("rescue"),
            space_check_every: Duration::from_secs(30),
            write_fault: std::sync::Arc::new(AtomicBool::new(false)),
            folder_fault: std::sync::Arc::new(AtomicBool::new(false)),
            device_wake: std::sync::Arc::new(AtomicBool::new(false)),
            stall_after: Duration::from_secs(8),
        };

        // Act
        let snapshot = run_session(&mut engine, &request, &stop, &|_| {});

        // Assert
        assert_eq!(snapshot.phase, RecordPhase::Error);
        assert_eq!(snapshot.notice, Some(NOTICE_DISK));
        assert!(!mic_opened.load(std::sync::atomic::Ordering::Acquire));
        assert!(!system_opened.load(std::sync::atomic::Ordering::Acquire));
        assert!(capture_notice(CaptureError::PrivacyDenied).contains("privacy"));
        assert!(ensure_writable(&dir).is_ok());
        let _ = std::fs::remove_dir_all(dir);
    }
}
