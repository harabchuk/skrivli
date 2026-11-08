//! Microphone recording session.
//!
//! `Idle → Starting → Recording → Stopping → Idle`, or `Error` when startup
//! fails. Stop is idempotent. The capture port is injected, so tests never
//! open WASAPI. The host thread owns the session; the GPUI thread only sends
//! a command.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::domain::audio::{AudioBlock, SessionOrigin, INTERNAL_SAMPLE_RATE};
use crate::domain::ids::RecordingId;
use crate::domain::recording::MaxMeetingDuration;
use crate::infrastructure::audio::levels::{GainControl, LevelSnapshot};
use crate::infrastructure::files::flac::{max_recording_frames, FlacWrite, FlacWriter};
use crate::infrastructure::files::writable::ensure_writable;
use crate::infrastructure::storage::sidecar::{write_sidecar, Sidecar};
use crate::services::audio_pipeline::{AudioSource, EchoStats, Pipeline, PipelineSetup};
use crate::services::ports::PipelineFactory;

use super::lease::EndpointLease;

pub(crate) use super::ports::{
    CaptureError, CapturePort, CaptureStream, FreeSpace, PerformanceCounter, RecordingClock,
};

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
pub(crate) const NOTICE_LIMIT: &str =
    "Recording stopped because it reached the maximum meeting duration.";
pub(crate) const NOTICE_FALLBACK: &str =
    "The selected microphone is unavailable. Using the Windows default.";
pub(crate) const NOTICE_SYSTEM_FALLBACK: &str =
    "The selected output is unavailable. Using the Windows default.";
pub(crate) const NOTICE_SYSTEM_MISSING: &str =
    "System audio is unavailable. Recording the microphone with silence on that side.";
pub(crate) const NOTICE_MIC_MISSING: &str =
    "The microphone is unavailable. Recording system audio with silence on that side.";
pub(crate) const NOTICE_WORKER: &str = "Recording stopped because a capture worker failed.";
pub(crate) const NOTICE_STALL: &str = "Audio capture appears stalled.";

pub(crate) use crate::domain::recording::RecordPhase;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RecordSnapshot {
    pub(crate) phase: RecordPhase,
    pub(crate) notice: Option<&'static str>,
    pub(crate) id: Option<RecordingId>,
    /// Finalized FLAC path after stop; drives auto-transcribe enqueue.
    pub(crate) finalized_path: Option<PathBuf>,
}

pub(crate) fn capture_notice(error: CaptureError) -> &'static str {
    match error {
        CaptureError::PrivacyDenied => NOTICE_PRIVACY,
        CaptureError::Unavailable => NOTICE_MIC,
        CaptureError::UnsupportedFormat => NOTICE_FORMAT,
        CaptureError::Failed => NOTICE_START,
    }
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
    /// Stops a forgotten recording at this length.
    pub(crate) max_duration: MaxMeetingDuration,
}

/// Frames the writer accepts for `duration`, never past the file format's ceiling.
pub(crate) fn writer_frame_cap(duration: MaxMeetingDuration) -> u64 {
    duration
        .frames(INTERNAL_SAMPLE_RATE)
        .min(max_recording_frames())
}

/// Outcome of turning settings and a device catalog into a start request.
pub(crate) enum StartRequestBuild {
    Ready(StartRequest),
    Refused { notice: &'static str },
}

/// Resolve microphone and system endpoints through the domain picker.
pub(crate) fn build_start_request(
    directory: PathBuf,
    microphone: &crate::infrastructure::storage::settings::StoredEndpoint,
    system_audio: &crate::infrastructure::storage::settings::StoredEndpoint,
    catalog: &crate::infrastructure::audio::devices::DeviceCatalog,
    max_duration: MaxMeetingDuration,
) -> StartRequestBuild {
    use crate::domain::audio::{resolve_endpoint, EndpointChoice, EndpointFlow};

    let mic_choice = EndpointChoice::from(microphone);
    let system_choice = EndpointChoice::from(system_audio);
    let mic = resolve_endpoint(
        &mic_choice,
        EndpointFlow::Capture,
        &catalog.capture,
        catalog.default_capture.as_ref(),
    );
    let system = resolve_endpoint(
        &system_choice,
        EndpointFlow::Render,
        &catalog.render,
        catalog.default_render.as_ref(),
    );

    let (open_microphone, endpoint_id, microphone_id, mut notice) = match mic {
        Ok(resolved) => {
            tracing::debug!(endpoint = resolved.name(), "microphone resolved");
            let id = resolved.id().as_str().to_string();
            let label = if id.is_empty() {
                "default".to_string()
            } else {
                id.clone()
            };
            let notice = resolved.fell_back().then_some(NOTICE_FALLBACK);
            (true, id, label, notice)
        }
        Err(_) => (
            false,
            String::new(),
            "unavailable".to_string(),
            Some(NOTICE_MIC_MISSING),
        ),
    };
    let (system_endpoint_id, system_audio_id) = match system {
        Ok(resolved) => {
            tracing::debug!(endpoint = resolved.name(), "system audio resolved");
            if resolved.fell_back() && notice.is_none() {
                notice = Some(NOTICE_SYSTEM_FALLBACK);
            }
            let id = resolved.id().as_str().to_string();
            let label = if id.is_empty() {
                "default".to_string()
            } else {
                id.clone()
            };
            (Some(id), label)
        }
        Err(_) => {
            if notice.is_none() || !open_microphone {
                notice = Some(if open_microphone {
                    NOTICE_SYSTEM_MISSING
                } else {
                    NOTICE_MIC
                });
            }
            (None, "unavailable".to_string())
        }
    };
    if !open_microphone && system_endpoint_id.is_none() {
        return StartRequestBuild::Refused { notice: NOTICE_MIC };
    }
    StartRequestBuild::Ready(StartRequest {
        directory,
        endpoint_id,
        microphone_id,
        system_endpoint_id,
        system_audio_id,
        open_microphone,
        degraded_notice: notice,
        max_duration,
    })
}

pub(crate) struct RecordingEngine {
    pub(crate) mic: Box<dyn CapturePort>,
    pub(crate) system: Box<dyn CapturePort>,
    pub(crate) clock: Box<dyn RecordingClock>,
    pub(crate) space: Box<dyn FreeSpace>,
    pub(crate) performance: Box<dyn PerformanceCounter>,
    pub(crate) levels: Arc<LevelSnapshot>,
    pub(crate) gain: Arc<GainControl>,
    pub(crate) pipeline: Box<dyn PipelineFactory>,
    pub(crate) lease: Arc<EndpointLease>,
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
        finalized_path: None,
    });
    match session_body(engine, request, stop, emit) {
        Ok(snapshot) => snapshot,
        Err(notice) => RecordSnapshot {
            phase: RecordPhase::Error,
            notice: Some(notice),
            id: None,
            finalized_path: None,
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
    let mut flac = match FlacWriter::create(
        &request.directory,
        when,
        writer_frame_cap(request.max_duration),
    ) {
        Ok(flac) => flac,
        Err(error) => {
            tracing::error!(error = %error, "recording file was not created");
            return Err(error.user_message());
        }
    };
    let (ticks, frequency) = engine.performance.sample();
    let origin = match SessionOrigin::new(ticks, frequency, INTERNAL_SAMPLE_RATE) {
        Some(origin) => origin,
        None => {
            let _ = flac.abandon();
            return Err(NOTICE_START);
        }
    };
    let mic_result = if request.open_microphone {
        engine.mic.open(&request.endpoint_id, origin)
    } else {
        Err(CaptureError::Unavailable)
    };
    let system_result = match &request.system_endpoint_id {
        Some(id) => engine.system.open(id, origin),
        None => Err(CaptureError::Unavailable),
    };
    let mut degraded = request.degraded_notice;
    let mic_error = mic_result.as_ref().err().copied();
    let mut mic = match mic_result {
        Ok(stream) => Some(stream),
        Err(error) => {
            if system_result.is_err() {
                tracing::error!("microphone capture did not start");
                if let Err(abandon) = flac.abandon() {
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
                if let Err(abandon) = flac.abandon() {
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
        finalized_path: None,
    });
    let mut pipeline = match engine.pipeline.build(PipelineSetup {
        system_active: want_system,
        gain: Arc::clone(&engine.gain),
        levels: Arc::clone(&engine.levels),
    }) {
        Ok(pipeline) => pipeline,
        Err(error) => {
            tracing::error!(error = %error, "audio pipeline was not built");
            if let Some(stream) = mic.as_mut() {
                stream.close();
            }
            if let Some(stream) = system.as_mut() {
                stream.close();
            }
            if let Err(abandon) = flac.abandon() {
                tracing::error!(error = %abandon, "partial recording was not removed");
            }
            return Err(NOTICE_START);
        }
    };
    let mut frames = 0_u64;
    let mut problems = Vec::new();
    let mut failed: Option<&'static str> = None;
    let mut limited = false;
    let mut last_space_check = Instant::now();
    let mut last_progress = Instant::now();
    let mut last_reopen = Instant::now();
    let mut stall_noted = false;
    let mut wrote_any = false;
    let mut drained = false;
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
            // A closed stream still holds its last blocks. A forced stop
            // skips them because the destination is already failing or full.
            if stopping && !limited && !drained {
                drained = true;
                while let Some(block) = pull_alive(
                    &mut mic,
                    &mut problems,
                    "capture",
                    NOTICE_MIC_MISSING,
                    &mut degraded,
                    id,
                    emit,
                ) {
                    pipeline.push_mic(block);
                }
                while let Some(block) = pull_alive(
                    &mut system,
                    &mut problems,
                    "loopback",
                    NOTICE_SYSTEM_MISSING,
                    &mut degraded,
                    id,
                    emit,
                ) {
                    pipeline.push_system(block);
                }
                pipeline.finish_input();
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
            if mic_before && mic.is_none() {
                pipeline.flush_source(AudioSource::Mic);
            }
            if system_before && system.is_none() {
                pipeline.flush_source(AudioSource::System);
            }
            let lost_source = (mic_before && mic.is_none()) || (system_before && system.is_none());
            if lost_source {
                note(&mut problems, "gap");
            }
            if let Some(block) = mic_packet {
                pipeline.push_mic(block);
                last_progress = Instant::now();
            }
            if let Some(block) = system_packet {
                pipeline.push_system(block);
                last_progress = Instant::now();
            }
            let wake = engine.device_wake.swap(false, Ordering::AcqRel);
            let missing = (want_mic && mic.is_none()) || (want_system && system.is_none());
            let reopen_due = missing && last_reopen.elapsed() >= Duration::from_secs(2);
            if wake || lost_source || reopen_due {
                try_reopen_sources(
                    engine,
                    origin,
                    &mut pipeline,
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
                    finalized_path: None,
                });
            }
        }
        let now = frame_now(origin, engine.performance.as_ref());
        let mut wrote = false;
        while let Some(mixed) = pipeline.pull(now, stopping || limited) {
            match flac.write_samples(mixed) {
                Ok(FlacWrite::Wrote { frames: written }) => {
                    frames = frames.saturating_add(written);
                    wrote = true;
                    wrote_any = true;
                    last_progress = Instant::now();
                }
                Ok(FlacWrite::SafeStop { frames: written }) => {
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
        finalized_path: None,
    });
    let diagnostics = pipeline.diagnostics();
    if diagnostics.audio_processing {
        note(&mut problems, "audio_processing");
    }
    log_echo_statistics(diagnostics.echo_stats);
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
        flac,
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
                finalized_path: None,
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
                finalized_path: None,
            });
            None
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn try_reopen_sources(
    engine: &mut RecordingEngine,
    origin: SessionOrigin,
    pipeline: &mut Pipeline,
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
        match engine.mic.open("", origin) {
            Ok(stream) => {
                *mic = Some(stream);
                pipeline.source_reopened(AudioSource::Mic);
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
        match engine.system.open("", origin) {
            Ok(stream) => {
                *system = Some(stream);
                pipeline.source_reopened(AudioSource::System);
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
            finalized_path: None,
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn finish_session(
    flac: FlacWriter,
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
    if let Some(notice) = failed.or(if folder_ok { None } else { Some(NOTICE_FOLDER) }) {
        return keep_after_failure(
            flac,
            engine,
            id,
            started_ms,
            frames,
            microphone_id,
            system_audio_id,
            problems,
            notice,
        );
    }
    let path = match flac.finalize() {
        Ok(path) => path,
        Err(error) => {
            tracing::error!(error = %error, "recording was not finalized");
            return Err(NOTICE_SAVE);
        }
    };
    let ended_ms = engine.clock.unix_millis().max(started_ms);
    let duration_ms = frames.saturating_mul(1_000) / u64::from(INTERNAL_SAMPLE_RATE);
    let hit_duration_limit = problems.iter().any(|problem| problem == "safe_stop");
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
    let notice = if limited && (hit_duration_limit || degraded.is_none()) {
        NOTICE_LIMIT
    } else {
        degraded.unwrap_or(NOTICE_SAVED)
    };
    Ok(RecordSnapshot {
        phase: RecordPhase::Idle,
        notice: Some(notice),
        id: Some(id),
        finalized_path: Some(path),
    })
}

/// Finalizes in place after a failure. If the rename fails the partial stays
/// on disk untouched. The original failure is always the notice.
#[allow(clippy::too_many_arguments)]
fn keep_after_failure(
    flac: FlacWriter,
    engine: &RecordingEngine,
    id: RecordingId,
    started_ms: u64,
    frames: u64,
    microphone_id: String,
    system_audio_id: String,
    mut problems: Vec<String>,
    failure: &'static str,
) -> Result<RecordSnapshot, &'static str> {
    note(&mut problems, "save_error");
    match flac.finalize() {
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
        }
        Err(error) => {
            tracing::error!(error = %error, "finalize after failure did not complete");
        }
    }
    Err(failure)
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

/// Logs the capped internal statistic once. Skips the line when the canceller
/// was not built or never completed a capture, and does not invent a zero for
/// a missing estimate. The fields are not full-band ERLE. Samples are not logged.
fn log_echo_statistics(stats: Option<EchoStats>) {
    let Some(stats) = stats else {
        return;
    };
    match (stats.echo_return_loss_enhancement, stats.delay_ms) {
        (Some(echo_return_loss_enhancement), Some(delay_ms)) => {
            tracing::info!(
                echo_return_loss_enhancement,
                delay_ms,
                "echo canceller statistics"
            );
        }
        (Some(echo_return_loss_enhancement), None) => {
            tracing::info!(echo_return_loss_enhancement, "echo canceller statistics");
        }
        (None, Some(delay_ms)) => {
            tracing::info!(delay_ms, "echo canceller statistics");
        }
        (None, None) => {}
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    use std::time::Duration;

    use super::{
        capture_notice, run_session, CaptureError, FreeSpace, RecordPhase, RecordingClock,
        StartRequest, NOTICE_DISK,
    };
    use crate::domain::audio::AudioBlock;
    use crate::infrastructure::files::writable::ensure_writable;

    #[test]
    fn writer_cap_follows_the_chosen_meeting_duration() {
        // Arrange
        let two = crate::domain::recording::MaxMeetingDuration::from_hours(2).expect("two");

        // Act
        let frames = super::writer_frame_cap(two);

        // Assert
        assert_eq!(frames, 2 * 3_600 * 48_000);
        assert_eq!(
            super::NOTICE_LIMIT,
            "Recording stopped because it reached the maximum meeting duration."
        );
    }
    use crate::services::audio_pipeline::PassThroughPipelineFactory;

    struct OneShot {
        block: Option<AudioBlock>,
        opened: std::sync::Arc<AtomicBool>,
    }

    impl super::CapturePort for OneShot {
        fn open(
            &mut self,
            _: &str,
            _: crate::domain::audio::SessionOrigin,
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

        fn local(&self) -> crate::domain::audio::CivilTime {
            crate::domain::audio::CivilTime::new(2026, 1, 2, 3, 4, 5).expect("time")
        }
    }

    struct FixedCounter;

    impl super::PerformanceCounter for FixedCounter {
        fn sample(&self) -> (u64, u64) {
            (0, 48_000)
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
            max_duration: crate::domain::recording::MaxMeetingDuration::default(),
        };
        let mut engine = super::RecordingEngine {
            mic: Box::new(port),
            system: Box::new(system),
            clock: Box::new(Clock),
            space: Box::new(Space(1024)),
            performance: Box::new(FixedCounter),
            levels: std::sync::Arc::new(crate::infrastructure::audio::levels::LevelSnapshot::new()),
            gain: std::sync::Arc::new(crate::infrastructure::audio::levels::GainControl::new(1.0)),
            pipeline: Box::new(PassThroughPipelineFactory),
            lease: std::sync::Arc::new(crate::services::lease::EndpointLease::new()),
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

    /// Yields its block only after `close`, like a worker's final chunks.
    struct TailPort {
        block: Option<AudioBlock>,
    }

    struct TailStream {
        block: Option<AudioBlock>,
        closed: bool,
    }

    impl super::CapturePort for TailPort {
        fn open(
            &mut self,
            _: &str,
            _: crate::domain::audio::SessionOrigin,
        ) -> Result<Box<dyn super::CaptureStream>, CaptureError> {
            Ok(Box::new(TailStream {
                block: self.block.take(),
                closed: false,
            }))
        }
    }

    impl super::CaptureStream for TailStream {
        fn pull(&mut self) -> Result<Option<AudioBlock>, CaptureError> {
            if self.closed {
                Ok(self.block.take())
            } else {
                Ok(None)
            }
        }

        fn close(&mut self) {
            self.closed = true;
        }

        fn overruns(&self) -> u64 {
            0
        }
    }

    #[test]
    fn stop_writes_blocks_that_are_still_queued_when_the_stream_closes() {
        // Arrange
        let dir = std::env::temp_dir().join(format!("skrivli-tail-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let stop = AtomicBool::new(true);
        let request = StartRequest {
            directory: dir.clone(),
            endpoint_id: "mic".to_string(),
            microphone_id: "mic".to_string(),
            system_endpoint_id: None,
            system_audio_id: "unavailable".to_string(),
            open_microphone: true,
            degraded_notice: None,
            max_duration: crate::domain::recording::MaxMeetingDuration::default(),
        };
        let mut engine = super::RecordingEngine {
            mic: Box::new(TailPort {
                block: Some(AudioBlock {
                    samples: vec![0.25; 960],
                    timeline_frame: None,
                    discontinuity: false,
                }),
            }),
            system: Box::new(TailPort { block: None }),
            clock: Box::new(Clock),
            space: Box::new(Space(u64::MAX)),
            performance: Box::new(FixedCounter),
            levels: Arc::new(crate::infrastructure::audio::levels::LevelSnapshot::new()),
            gain: Arc::new(crate::infrastructure::audio::levels::GainControl::new(1.0)),
            pipeline: Box::new(PassThroughPipelineFactory),
            lease: Arc::new(crate::services::lease::EndpointLease::new()),
            space_check_every: Duration::from_secs(30),
            write_fault: Arc::new(AtomicBool::new(false)),
            folder_fault: Arc::new(AtomicBool::new(false)),
            device_wake: Arc::new(AtomicBool::new(false)),
            stall_after: Duration::from_secs(8),
        };

        // Act
        let snapshot = run_session(&mut engine, &request, &stop, &|_| {});

        // Assert
        let path = snapshot.finalized_path.expect("saved recording");
        let reader = flac_codec::decode::FlacSampleReader::open(path).expect("flac");
        assert_eq!(
            flac_codec::metadata::Metadata::total_samples(&reader),
            Some(480)
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    fn capture(id: &str) -> crate::domain::audio::EndpointDescriptor {
        crate::domain::audio::EndpointDescriptor::new(
            crate::domain::audio::EndpointId::new(id).expect("id"),
            id,
            crate::domain::audio::EndpointFlow::Capture,
            crate::domain::audio::EndpointAvailability::Active,
        )
    }

    fn render(id: &str) -> crate::domain::audio::EndpointDescriptor {
        crate::domain::audio::EndpointDescriptor::new(
            crate::domain::audio::EndpointId::new(id).expect("id"),
            id,
            crate::domain::audio::EndpointFlow::Render,
            crate::domain::audio::EndpointAvailability::Active,
        )
    }

    fn catalog_with(
        capture: Vec<crate::domain::audio::EndpointDescriptor>,
        render: Vec<crate::domain::audio::EndpointDescriptor>,
        default_capture: Option<&str>,
        default_render: Option<&str>,
    ) -> crate::infrastructure::audio::devices::DeviceCatalog {
        crate::infrastructure::audio::devices::DeviceCatalog {
            capture,
            render,
            default_capture: default_capture
                .map(|id| crate::domain::audio::EndpointId::new(id).expect("id")),
            default_render: default_render
                .map(|id| crate::domain::audio::EndpointId::new(id).expect("id")),
        }
    }

    #[test]
    fn start_request_uses_defaults_without_fallback_notice() {
        // Arrange
        use super::{build_start_request, StartRequestBuild};
        use crate::infrastructure::storage::settings::StoredEndpoint;

        let catalog = catalog_with(
            vec![capture("mic-a")],
            vec![render("spk-a")],
            Some("mic-a"),
            Some("spk-a"),
        );

        // Act
        let built = build_start_request(
            std::path::PathBuf::from("rec"),
            &StoredEndpoint::Default,
            &StoredEndpoint::Default,
            &catalog,
            crate::domain::recording::MaxMeetingDuration::default(),
        );

        // Assert
        let StartRequestBuild::Ready(request) = built else {
            panic!("expected ready");
        };
        assert!(request.open_microphone);
        assert_eq!(request.endpoint_id, "mic-a");
        assert_eq!(request.system_endpoint_id.as_deref(), Some("spk-a"));
        assert_eq!(request.degraded_notice, None);
    }

    #[test]
    fn start_request_reports_mic_fallback_when_explicit_is_missing() {
        // Arrange
        use super::{build_start_request, StartRequestBuild, NOTICE_FALLBACK};
        use crate::infrastructure::storage::settings::StoredEndpoint;

        let catalog = catalog_with(
            vec![capture("mic-a")],
            vec![render("spk-a")],
            Some("mic-a"),
            Some("spk-a"),
        );

        // Act
        let built = build_start_request(
            std::path::PathBuf::from("rec"),
            &StoredEndpoint::explicit("gone"),
            &StoredEndpoint::Default,
            &catalog,
            crate::domain::recording::MaxMeetingDuration::default(),
        );

        // Assert
        let StartRequestBuild::Ready(request) = built else {
            panic!("expected ready");
        };
        assert_eq!(request.endpoint_id, "mic-a");
        assert_eq!(request.degraded_notice, Some(NOTICE_FALLBACK));
    }

    #[test]
    fn start_request_default_falls_through_to_first_active_device() {
        // Arrange
        use super::{build_start_request, StartRequestBuild, NOTICE_FALLBACK};
        use crate::infrastructure::storage::settings::StoredEndpoint;

        let catalog = catalog_with(vec![capture("mic-a")], vec![render("spk-a")], None, None);

        // Act
        let built = build_start_request(
            std::path::PathBuf::from("rec"),
            &StoredEndpoint::Default,
            &StoredEndpoint::Default,
            &catalog,
            crate::domain::recording::MaxMeetingDuration::default(),
        );

        // Assert
        let StartRequestBuild::Ready(request) = built else {
            panic!("expected ready");
        };
        assert_eq!(request.endpoint_id, "mic-a");
        assert_eq!(request.system_endpoint_id.as_deref(), Some("spk-a"));
        assert_eq!(request.degraded_notice, Some(NOTICE_FALLBACK));
    }

    #[test]
    fn start_request_continues_with_system_when_mic_is_missing() {
        // Arrange
        use super::{build_start_request, StartRequestBuild, NOTICE_MIC_MISSING};
        use crate::infrastructure::storage::settings::StoredEndpoint;

        let catalog = catalog_with(Vec::new(), vec![render("spk-a")], None, Some("spk-a"));

        // Act
        let built = build_start_request(
            std::path::PathBuf::from("rec"),
            &StoredEndpoint::Default,
            &StoredEndpoint::Default,
            &catalog,
            crate::domain::recording::MaxMeetingDuration::default(),
        );

        // Assert
        let StartRequestBuild::Ready(request) = built else {
            panic!("expected ready");
        };
        assert!(!request.open_microphone);
        assert_eq!(request.system_endpoint_id.as_deref(), Some("spk-a"));
        assert_eq!(request.degraded_notice, Some(NOTICE_MIC_MISSING));
    }

    #[test]
    fn start_request_refuses_when_both_sources_are_missing() {
        // Arrange
        use super::{build_start_request, StartRequestBuild, NOTICE_MIC};
        use crate::infrastructure::storage::settings::StoredEndpoint;

        let catalog = catalog_with(Vec::new(), Vec::new(), None, None);

        // Act
        let built = build_start_request(
            std::path::PathBuf::from("rec"),
            &StoredEndpoint::Default,
            &StoredEndpoint::Default,
            &catalog,
            crate::domain::recording::MaxMeetingDuration::default(),
        );

        // Assert
        match built {
            StartRequestBuild::Refused { notice } => assert_eq!(notice, NOTICE_MIC),
            StartRequestBuild::Ready(_) => panic!("expected refuse"),
        }
    }

    fn saved_sidecar(
        label: &str,
        pipeline: Box<dyn crate::services::ports::PipelineFactory>,
    ) -> String {
        let dir = std::env::temp_dir().join(format!("skrivli-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let stop = AtomicBool::new(true);
        let request = StartRequest {
            directory: dir.clone(),
            endpoint_id: "mic".to_string(),
            microphone_id: "mic".to_string(),
            system_endpoint_id: None,
            system_audio_id: "unavailable".to_string(),
            open_microphone: true,
            degraded_notice: None,
            max_duration: crate::domain::recording::MaxMeetingDuration::default(),
        };
        let mut engine = super::RecordingEngine {
            mic: Box::new(TailPort {
                block: Some(AudioBlock {
                    samples: vec![0.2; 960],
                    timeline_frame: Some(0),
                    discontinuity: false,
                }),
            }),
            system: Box::new(TailPort { block: None }),
            clock: Box::new(Clock),
            space: Box::new(Space(u64::MAX)),
            performance: Box::new(FixedCounter),
            levels: Arc::new(crate::infrastructure::audio::levels::LevelSnapshot::new()),
            gain: Arc::new(crate::infrastructure::audio::levels::GainControl::new(1.0)),
            pipeline,
            lease: Arc::new(crate::services::lease::EndpointLease::new()),
            space_check_every: Duration::from_secs(30),
            write_fault: Arc::new(AtomicBool::new(false)),
            folder_fault: Arc::new(AtomicBool::new(false)),
            device_wake: Arc::new(AtomicBool::new(false)),
            stall_after: Duration::from_secs(8),
        };
        let snapshot = run_session(&mut engine, &request, &stop, &|_| {});
        let path = snapshot.finalized_path.expect("saved recording");
        let text = std::fs::read_to_string(path.with_extension("json")).expect("sidecar");
        let _ = std::fs::remove_dir_all(dir);
        text
    }

    #[test]
    fn skipped_sonora_constructor_records_audio_processing_once() {
        // Arrange
        use crate::infrastructure::audio::sonora_steps::FailedMicGainFactory;

        // Act
        let sidecar = saved_sidecar("gain-skip", Box::new(FailedMicGainFactory));

        // Assert
        assert_eq!(sidecar.matches("audio_processing").count(), 1);
    }

    #[test]
    fn system_audio_off_omits_audio_processing() {
        // Arrange
        use crate::infrastructure::audio::sonora_steps::SonoraPipelineFactory;

        // Act
        let sidecar = saved_sidecar("system-off", Box::new(SonoraPipelineFactory));

        // Assert
        assert!(!sidecar.contains("audio_processing"));
    }
}
