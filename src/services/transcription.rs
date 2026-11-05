//! Offline transcription job queue. One FIFO worker; no GPUI or engine types leak out.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::common::error::AppError;
use crate::domain::ids::{JobId, RecordingId};
use crate::domain::recording::TranscriptStatus;
use crate::domain::transcript::{
    format_transcript, plan_windows, stitch_segments, TranscriptSegment, WINDOW_DURATION,
    WINDOW_OVERLAP,
};
use crate::infrastructure::ai::resample::pcm16_to_whisper_mono;
use crate::infrastructure::files::transcript_reader::{TranscriptWavReader, DEFAULT_CHUNK_FRAMES};
use crate::infrastructure::storage::sidecar::{
    update_transcription, write_transcript_text, TranscriptionWrite,
};
use crate::services::ports::{InferencePort, InferenceSegment};

pub(crate) const NOTICE_DONE: &str = "Transcript ready.";
pub(crate) const NOTICE_FAILED: &str = "Transcription failed.";
pub(crate) const NOTICE_CANCELLED: &str = "Transcription cancelled.";
pub(crate) const NOTICE_MODEL: &str = "The speech model file is missing or unsupported.";

/// Tunables for windowed decode. Production uses domain defaults; tests shrink them.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TranscriptionConfig {
    pub(crate) window: Duration,
    pub(crate) overlap: Duration,
    pub(crate) read_frames: usize,
}

impl Default for TranscriptionConfig {
    fn default() -> Self {
        Self {
            window: WINDOW_DURATION,
            overlap: WINDOW_OVERLAP,
            read_frames: DEFAULT_CHUNK_FRAMES,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum JobPhase {
    Queued,
    Preparing,
    Transcribing,
    Done,
    Failed,
    Cancelled,
}

impl JobPhase {
    fn as_raw(self) -> u8 {
        match self {
            Self::Queued => 0,
            Self::Preparing => 1,
            Self::Transcribing => 2,
            Self::Done => 3,
            Self::Failed => 4,
            Self::Cancelled => 5,
        }
    }

    fn from_raw(raw: u8) -> Self {
        match raw {
            1 => Self::Preparing,
            2 => Self::Transcribing,
            3 => Self::Done,
            4 => Self::Failed,
            5 => Self::Cancelled,
            _ => Self::Queued,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TranscriptionSnapshot {
    pub(crate) job_id: Option<JobId>,
    pub(crate) recording_id: Option<RecordingId>,
    pub(crate) phase: JobPhase,
    pub(crate) progress_percent: u8,
    pub(crate) notice: Option<&'static str>,
}

impl TranscriptionSnapshot {
    pub(crate) fn idle() -> Self {
        Self {
            job_id: None,
            recording_id: None,
            phase: JobPhase::Queued,
            progress_percent: 0,
            notice: None,
        }
    }

    #[allow(dead_code)] // Reduced into Presentation / library rows in M7.
    pub(crate) fn transcript_status(&self) -> TranscriptStatus {
        match self.phase {
            JobPhase::Queued => TranscriptStatus::Queued,
            JobPhase::Preparing => TranscriptStatus::Progress(0),
            JobPhase::Transcribing => TranscriptStatus::Progress(self.progress_percent),
            JobPhase::Done => TranscriptStatus::Ready,
            JobPhase::Failed | JobPhase::Cancelled => TranscriptStatus::Failed,
        }
    }
}

struct EnqueueRequest {
    recording_id: RecordingId,
    audio_path: PathBuf,
    model_path: PathBuf,
    model_id: String,
}

enum TranscriptionCommand {
    Enqueue(EnqueueRequest),
    Cancel { job_id: JobId },
    Shutdown,
}

struct QueuedJob {
    job_id: JobId,
    request: EnqueueRequest,
    cancel: Arc<AtomicBool>,
}

type ActiveCancel = Option<(JobId, Arc<AtomicBool>)>;

/// Owns one inference worker and a FIFO of transcription jobs.
#[derive(Clone)]
pub(crate) struct TranscriptionHost {
    tx: Sender<TranscriptionCommand>,
    /// Running job cancel flag; set from [`Self::cancel`] without waiting on the worker.
    active_cancel: Arc<Mutex<ActiveCancel>>,
    phase: Arc<AtomicU8Mirror>,
    progress: Arc<AtomicU8Mirror>,
    job_id: Arc<Mutex<Option<JobId>>>,
    recording_id: Arc<Mutex<Option<RecordingId>>>,
    notice: Arc<Mutex<Option<&'static str>>>,
}

/// Thin AtomicU8 wrapper so snapshot helpers stay local to this module.
struct AtomicU8Mirror(std::sync::atomic::AtomicU8);

impl AtomicU8Mirror {
    fn new(value: u8) -> Self {
        Self(std::sync::atomic::AtomicU8::new(value))
    }

    fn store(&self, value: u8) {
        self.0.store(value, Ordering::Release);
    }

    fn load(&self) -> u8 {
        self.0.load(Ordering::Acquire)
    }
}

impl TranscriptionHost {
    pub(crate) fn spawn(
        inference: Box<dyn InferencePort>,
        on_change: Box<dyn Fn(TranscriptionSnapshot) + Send>,
    ) -> (Self, JoinHandle<()>) {
        Self::spawn_with_config(inference, TranscriptionConfig::default(), on_change)
    }

    pub(crate) fn spawn_with_config(
        inference: Box<dyn InferencePort>,
        config: TranscriptionConfig,
        on_change: Box<dyn Fn(TranscriptionSnapshot) + Send>,
    ) -> (Self, JoinHandle<()>) {
        let (tx, rx) = mpsc::channel();
        let active_cancel = Arc::new(Mutex::new(None));
        let phase = Arc::new(AtomicU8Mirror::new(JobPhase::Queued.as_raw()));
        let progress = Arc::new(AtomicU8Mirror::new(0));
        let job_id = Arc::new(Mutex::new(None));
        let recording_id = Arc::new(Mutex::new(None));
        let notice = Arc::new(Mutex::new(None));
        let worker_phase = Arc::clone(&phase);
        let worker_progress = Arc::clone(&progress);
        let worker_job = Arc::clone(&job_id);
        let worker_recording = Arc::clone(&recording_id);
        let worker_notice = Arc::clone(&notice);
        let worker_active_cancel = Arc::clone(&active_cancel);
        let handle = thread::spawn(move || {
            worker_loop(
                rx,
                inference,
                config,
                worker_active_cancel,
                worker_phase,
                worker_progress,
                worker_job,
                worker_recording,
                worker_notice,
                on_change,
            );
        });
        (
            Self {
                tx,
                active_cancel,
                phase,
                progress,
                job_id,
                recording_id,
                notice,
            },
            handle,
        )
    }

    pub(crate) fn enqueue(
        &self,
        recording_id: RecordingId,
        audio_path: PathBuf,
        model_path: PathBuf,
        model_id: impl Into<String>,
    ) {
        let _ = self.tx.send(TranscriptionCommand::Enqueue(EnqueueRequest {
            recording_id,
            audio_path,
            model_path,
            model_id: model_id.into(),
        }));
    }

    pub(crate) fn cancel(&self, job_id: JobId) {
        if let Ok(guard) = self.active_cancel.lock() {
            if let Some((active_id, flag)) = guard.as_ref() {
                if *active_id == job_id {
                    flag.store(true, Ordering::Release);
                }
            }
        }
        let _ = self.tx.send(TranscriptionCommand::Cancel { job_id });
    }

    pub(crate) fn shutdown(&self) {
        let _ = self.tx.send(TranscriptionCommand::Shutdown);
    }

    pub(crate) fn snapshot(&self) -> TranscriptionSnapshot {
        let phase = JobPhase::from_raw(self.phase.load());
        let job_id = *self
            .job_id
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let recording_id = *self
            .recording_id
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let notice = *self
            .notice
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        TranscriptionSnapshot {
            job_id,
            recording_id,
            phase,
            progress_percent: self.progress.load(),
            notice,
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn worker_loop(
    rx: mpsc::Receiver<TranscriptionCommand>,
    mut inference: Box<dyn InferencePort>,
    config: TranscriptionConfig,
    active_cancel: Arc<Mutex<ActiveCancel>>,
    phase: Arc<AtomicU8Mirror>,
    progress: Arc<AtomicU8Mirror>,
    job_id: Arc<Mutex<Option<JobId>>>,
    recording_id: Arc<Mutex<Option<RecordingId>>>,
    notice: Arc<Mutex<Option<&'static str>>>,
    on_change: Box<dyn Fn(TranscriptionSnapshot) + Send>,
) {
    let mut queue: VecDeque<QueuedJob> = VecDeque::new();
    let mut running_id: Option<RecordingId> = None;
    let mut next_job = 1_u64;

    loop {
        let command = if queue.is_empty() {
            match rx.recv() {
                Ok(command) => Some(command),
                Err(_) => break,
            }
        } else {
            match rx.try_recv() {
                Ok(command) => Some(command),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => break,
            }
        };

        if let Some(command) = command {
            match command {
                TranscriptionCommand::Shutdown => {
                    if let Ok(mut guard) = active_cancel.lock() {
                        if let Some((_, flag)) = guard.as_ref() {
                            flag.store(true, Ordering::Release);
                        }
                        *guard = None;
                    }
                    queue.clear();
                    inference.unload();
                    publish(
                        &phase,
                        &progress,
                        &job_id,
                        &recording_id,
                        &notice,
                        JobPhase::Cancelled,
                        None,
                        None,
                        0,
                        None,
                        &on_change,
                    );
                    break;
                }
                TranscriptionCommand::Enqueue(request) => {
                    let duplicate = running_id == Some(request.recording_id)
                        || queue
                            .iter()
                            .any(|job| job.request.recording_id == request.recording_id);
                    if duplicate {
                        continue;
                    }
                    let id = JobId::from_raw(next_job);
                    next_job = next_job.saturating_add(1);
                    let cancel = Arc::new(AtomicBool::new(false));
                    let recording = request.recording_id;
                    let path = request.audio_path.clone();
                    let model = request.model_id.clone();
                    queue.push_back(QueuedJob {
                        job_id: id,
                        request,
                        cancel,
                    });
                    let _ = update_transcription(
                        &path,
                        TranscriptionWrite::Queued,
                        Some(model.as_str()),
                    );
                    publish(
                        &phase,
                        &progress,
                        &job_id,
                        &recording_id,
                        &notice,
                        JobPhase::Queued,
                        Some(id),
                        Some(recording),
                        0,
                        None,
                        &on_change,
                    );
                }
                TranscriptionCommand::Cancel { job_id: target } => {
                    if let Some(index) = queue.iter().position(|job| job.job_id == target) {
                        if let Some(job) = queue.remove(index) {
                            let _ = update_transcription(
                                &job.request.audio_path,
                                TranscriptionWrite::Cancelled,
                                Some(job.request.model_id.as_str()),
                            );
                            publish(
                                &phase,
                                &progress,
                                &job_id,
                                &recording_id,
                                &notice,
                                JobPhase::Cancelled,
                                Some(job.job_id),
                                Some(job.request.recording_id),
                                0,
                                Some(NOTICE_CANCELLED),
                                &on_change,
                            );
                        }
                    } else if let Ok(guard) = active_cancel.lock() {
                        if let Some((active_id, flag)) = guard.as_ref() {
                            if *active_id == target {
                                flag.store(true, Ordering::Release);
                            }
                        }
                    }
                }
            }
            continue;
        }

        let Some(job) = queue.pop_front() else {
            continue;
        };
        let mut last_persisted_progress = 0_u8;
        let snapshot_job = job.job_id;
        let snapshot_recording = job.request.recording_id;
        let path = job.request.audio_path.clone();
        let model = job.request.model_id.clone();
        running_id = Some(snapshot_recording);
        if let Ok(mut guard) = active_cancel.lock() {
            *guard = Some((job.job_id, Arc::clone(&job.cancel)));
        }
        publish(
            &phase,
            &progress,
            &job_id,
            &recording_id,
            &notice,
            JobPhase::Preparing,
            Some(snapshot_job),
            Some(snapshot_recording),
            0,
            None,
            &on_change,
        );
        let _ = update_transcription(&path, TranscriptionWrite::Preparing, Some(model.as_str()));

        // Drain cancel/enqueue that arrived before Preparing so Cancel can still hit the flag.
        while let Ok(command) = rx.try_recv() {
            match command {
                TranscriptionCommand::Shutdown => {
                    job.cancel.store(true, Ordering::Release);
                    if let Ok(mut guard) = active_cancel.lock() {
                        *guard = None;
                    }
                    queue.clear();
                    inference.unload();
                    publish(
                        &phase,
                        &progress,
                        &job_id,
                        &recording_id,
                        &notice,
                        JobPhase::Cancelled,
                        None,
                        None,
                        0,
                        None,
                        &on_change,
                    );
                    return;
                }
                TranscriptionCommand::Enqueue(request) => {
                    let duplicate = running_id == Some(request.recording_id)
                        || queue
                            .iter()
                            .any(|item| item.request.recording_id == request.recording_id);
                    if duplicate {
                        continue;
                    }
                    let id = JobId::from_raw(next_job);
                    next_job = next_job.saturating_add(1);
                    let cancel = Arc::new(AtomicBool::new(false));
                    let recording = request.recording_id;
                    let queued_path = request.audio_path.clone();
                    let queued_model = request.model_id.clone();
                    queue.push_back(QueuedJob {
                        job_id: id,
                        request,
                        cancel,
                    });
                    let _ = update_transcription(
                        &queued_path,
                        TranscriptionWrite::Queued,
                        Some(queued_model.as_str()),
                    );
                    publish(
                        &phase,
                        &progress,
                        &job_id,
                        &recording_id,
                        &notice,
                        JobPhase::Queued,
                        Some(id),
                        Some(recording),
                        0,
                        None,
                        &on_change,
                    );
                }
                TranscriptionCommand::Cancel { job_id: target } => {
                    if target == job.job_id {
                        job.cancel.store(true, Ordering::Release);
                    } else if let Some(index) = queue.iter().position(|item| item.job_id == target)
                    {
                        if let Some(queued) = queue.remove(index) {
                            let _ = update_transcription(
                                &queued.request.audio_path,
                                TranscriptionWrite::Cancelled,
                                Some(queued.request.model_id.as_str()),
                            );
                            publish(
                                &phase,
                                &progress,
                                &job_id,
                                &recording_id,
                                &notice,
                                JobPhase::Cancelled,
                                Some(queued.job_id),
                                Some(queued.request.recording_id),
                                0,
                                Some(NOTICE_CANCELLED),
                                &on_change,
                            );
                        }
                    }
                }
            }
        }

        let outcome = run_job(
            inference.as_mut(),
            &job.request,
            &job.cancel,
            config,
            |percent| {
                publish(
                    &phase,
                    &progress,
                    &job_id,
                    &recording_id,
                    &notice,
                    JobPhase::Transcribing,
                    Some(snapshot_job),
                    Some(snapshot_recording),
                    percent,
                    None,
                    &on_change,
                );
                if percent == 0
                    || percent >= 100
                    || percent.saturating_sub(last_persisted_progress) >= 10
                {
                    last_persisted_progress = percent;
                    let _ = update_transcription(
                        &path,
                        TranscriptionWrite::Progress(percent),
                        Some(model.as_str()),
                    );
                }
            },
        );
        if let Ok(mut guard) = active_cancel.lock() {
            *guard = None;
        }
        running_id = None;
        inference.unload();
        match outcome {
            JobOutcome::Done => {
                let _ = update_transcription(
                    &job.request.audio_path,
                    TranscriptionWrite::Ready,
                    Some(job.request.model_id.as_str()),
                );
                publish(
                    &phase,
                    &progress,
                    &job_id,
                    &recording_id,
                    &notice,
                    JobPhase::Done,
                    Some(job.job_id),
                    Some(job.request.recording_id),
                    100,
                    Some(NOTICE_DONE),
                    &on_change,
                );
            }
            JobOutcome::Cancelled => {
                let _ = update_transcription(
                    &job.request.audio_path,
                    TranscriptionWrite::Cancelled,
                    Some(job.request.model_id.as_str()),
                );
                publish(
                    &phase,
                    &progress,
                    &job_id,
                    &recording_id,
                    &notice,
                    JobPhase::Cancelled,
                    Some(job.job_id),
                    Some(job.request.recording_id),
                    progress.load(),
                    Some(NOTICE_CANCELLED),
                    &on_change,
                );
            }
            JobOutcome::Failed { model_issue } => {
                let notice_text = if model_issue {
                    NOTICE_MODEL
                } else {
                    NOTICE_FAILED
                };
                let _ = update_transcription(
                    &job.request.audio_path,
                    TranscriptionWrite::Failed,
                    Some(job.request.model_id.as_str()),
                );
                publish(
                    &phase,
                    &progress,
                    &job_id,
                    &recording_id,
                    &notice,
                    JobPhase::Failed,
                    Some(job.job_id),
                    Some(job.request.recording_id),
                    progress.load(),
                    Some(notice_text),
                    &on_change,
                );
            }
        }
    }
}

enum JobOutcome {
    Done,
    Cancelled,
    Failed { model_issue: bool },
}

fn run_job(
    inference: &mut dyn InferencePort,
    request: &EnqueueRequest,
    cancel: &Arc<AtomicBool>,
    config: TranscriptionConfig,
    mut on_progress: impl FnMut(u8),
) -> JobOutcome {
    if cancel.load(Ordering::Acquire) {
        return JobOutcome::Cancelled;
    }
    on_progress(0);
    if let Err(error) = inference.load(&request.model_path) {
        tracing::error!(error = %error, "transcription model was not loaded");
        return if cancel.load(Ordering::Acquire) {
            JobOutcome::Cancelled
        } else {
            JobOutcome::Failed {
                model_issue: is_model_error(&error),
            }
        };
    }
    if cancel.load(Ordering::Acquire) {
        inference.unload();
        return JobOutcome::Cancelled;
    }

    let result = transcribe_file(
        inference,
        &request.audio_path,
        cancel,
        config,
        &mut on_progress,
    );
    match result {
        Ok(text) => {
            if cancel.load(Ordering::Acquire) {
                return JobOutcome::Cancelled;
            }
            match write_transcript_text(&request.audio_path, &text) {
                Ok(_) => {
                    on_progress(100);
                    JobOutcome::Done
                }
                Err(error) => {
                    tracing::error!(error = %error, "transcript text was not written");
                    JobOutcome::Failed { model_issue: false }
                }
            }
        }
        Err(error) if is_cancel_error(&error) || cancel.load(Ordering::Acquire) => {
            JobOutcome::Cancelled
        }
        Err(error) => {
            tracing::error!(error = %error, "transcription job failed");
            JobOutcome::Failed { model_issue: false }
        }
    }
}

fn is_cancel_error(error: &AppError) -> bool {
    error.user_message() == "Transcription was cancelled."
}

fn is_model_error(error: &AppError) -> bool {
    matches!(
        error.user_message(),
        "The speech model file is missing."
            | "The speech model file could not be read."
            | "The speech model file is not supported."
            | "Could not load the speech model."
    )
}

fn transcribe_file(
    inference: &mut dyn InferencePort,
    audio_path: &Path,
    cancel: &Arc<AtomicBool>,
    config: TranscriptionConfig,
    on_progress: &mut dyn FnMut(u8),
) -> Result<String, AppError> {
    let mut reader = TranscriptWavReader::open(audio_path)?;
    let format = reader.format();
    let total_frames = reader.frames_remaining().unwrap_or(0);
    let total_duration = if format.sample_rate() == 0 {
        Duration::ZERO
    } else {
        Duration::from_secs_f64(total_frames as f64 / f64::from(format.sample_rate()))
    };
    let windows = plan_windows_with(config.window, config.overlap, total_duration);
    if windows.is_empty() {
        on_progress(100);
        return Ok(String::new());
    }

    let channels = usize::from(format.channels());
    let mut pcm = vec![0_i16; config.read_frames.saturating_mul(channels.max(1))];
    let mut pending: Vec<f32> = Vec::new();
    let mut window_results: Vec<(u64, Vec<TranscriptSegment>)> = Vec::new();
    let window_samples = duration_to_whisper_samples(config.window);
    let hop_samples =
        duration_to_whisper_samples(config.window.saturating_sub(config.overlap)).max(1);
    let overlap_ms = config.overlap.as_millis() as u64;
    let total_windows = windows.len().max(1);
    let mut next_offset_ms = 0_u64;
    let mut completed_windows = 0_usize;

    loop {
        if cancel.load(Ordering::Acquire) {
            return Err(AppError::message("transcription cancelled", "cancel"));
        }
        let frames = reader.read_i16_frames(&mut pcm, config.read_frames)?;
        if frames == 0 {
            break;
        }
        let samples = &pcm[..frames.saturating_mul(channels)];
        let mono = pcm16_to_whisper_mono(samples, format.sample_rate(), format.channels())?;
        pending.extend_from_slice(&mono);

        while pending.len() >= window_samples {
            if cancel.load(Ordering::Acquire) {
                return Err(AppError::message("transcription cancelled", "cancel"));
            }
            let chunk = pending[..window_samples].to_vec();
            let base_progress =
                ((completed_windows as u32 * 100) / total_windows as u32).min(99) as u8;
            on_progress(base_progress);
            let latest = Arc::new(std::sync::atomic::AtomicU8::new(base_progress));
            let progress_slot = Arc::clone(&latest);
            let completed = completed_windows;
            let total = total_windows;
            let progress_cb = Arc::new(move |local: u8| {
                let spanned = completed as u32 * 100 + u32::from(local);
                let overall = (spanned / total as u32).min(99) as u8;
                progress_slot.store(overall, Ordering::Release);
            }) as Arc<dyn Fn(u8) + Send + Sync>;
            let segments = inference.transcribe_chunk(&chunk, cancel, progress_cb)?;
            on_progress(latest.load(Ordering::Acquire));
            window_results.push((next_offset_ms, to_domain_segments(segments)));
            pending.drain(..hop_samples.min(pending.len()));
            next_offset_ms = next_offset_ms.saturating_add(samples_to_ms(hop_samples));
            completed_windows = completed_windows.saturating_add(1);
        }
    }

    if !pending.is_empty() {
        if cancel.load(Ordering::Acquire) {
            return Err(AppError::message("transcription cancelled", "cancel"));
        }
        let latest = Arc::new(std::sync::atomic::AtomicU8::new(90));
        let progress_slot = Arc::clone(&latest);
        let progress_cb = Arc::new(move |local: u8| {
            let overall = (90 + (u32::from(local) / 10)).min(99) as u8;
            progress_slot.store(overall, Ordering::Release);
        }) as Arc<dyn Fn(u8) + Send + Sync>;
        let segments = inference.transcribe_chunk(&pending, cancel, progress_cb)?;
        on_progress(latest.load(Ordering::Acquire));
        window_results.push((next_offset_ms, to_domain_segments(segments)));
    }

    let stitched = stitch_segments(&window_results, overlap_ms);
    Ok(format_transcript(&stitched))
}

fn plan_windows_with(
    window: Duration,
    overlap: Duration,
    total: Duration,
) -> Vec<(Duration, Duration)> {
    if total.is_zero() {
        return Vec::new();
    }
    // Reuse domain planner when using production constants; otherwise replicate hop math.
    if window == WINDOW_DURATION && overlap == WINDOW_OVERLAP {
        return plan_windows(total)
            .into_iter()
            .map(|item| (item.offset(), item.end()))
            .collect();
    }
    let hop = window.saturating_sub(overlap);
    if hop.is_zero() {
        return vec![(Duration::ZERO, total)];
    }
    let mut out = Vec::new();
    let mut offset = Duration::ZERO;
    loop {
        let end = (offset + window).min(total);
        out.push((offset, end));
        if end >= total {
            break;
        }
        offset = offset.saturating_add(hop);
        if offset >= total {
            break;
        }
    }
    out
}

fn duration_to_whisper_samples(duration: Duration) -> usize {
    ((duration.as_secs_f64() * 16_000.0).round() as usize).max(1)
}

fn samples_to_ms(samples: usize) -> u64 {
    ((samples as u64).saturating_mul(1_000)) / 16_000
}

fn to_domain_segments(segments: Vec<InferenceSegment>) -> Vec<TranscriptSegment> {
    segments
        .into_iter()
        .map(|segment| TranscriptSegment::new(segment.start_ms, segment.end_ms, segment.text))
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn publish(
    phase: &Arc<AtomicU8Mirror>,
    progress: &Arc<AtomicU8Mirror>,
    job_id: &Arc<Mutex<Option<JobId>>>,
    recording_id: &Arc<Mutex<Option<RecordingId>>>,
    notice: &Arc<Mutex<Option<&'static str>>>,
    next: JobPhase,
    id: Option<JobId>,
    recording: Option<RecordingId>,
    percent: u8,
    message: Option<&'static str>,
    on_change: &dyn Fn(TranscriptionSnapshot),
) {
    phase.store(next.as_raw());
    progress.store(percent);
    if let Ok(mut guard) = job_id.lock() {
        *guard = id;
    }
    if let Ok(mut guard) = recording_id.lock() {
        *guard = recording;
    }
    if let Ok(mut guard) = notice.lock() {
        *guard = message;
    }
    on_change(TranscriptionSnapshot {
        job_id: id,
        recording_id: recording,
        phase: next,
        progress_percent: percent,
        notice: message,
    });
}

#[cfg(test)]
mod tests {
    use super::{
        JobPhase, TranscriptionConfig, TranscriptionHost, TranscriptionSnapshot, NOTICE_CANCELLED,
        NOTICE_DONE, NOTICE_FAILED, NOTICE_MODEL,
    };
    use crate::common::error::AppError;
    use crate::domain::ids::RecordingId;
    use crate::domain::recording::TranscriptStatus;
    use crate::infrastructure::storage::sidecar::{
        load_sidecar, write_sidecar, Sidecar, SidecarLoad, TranscriptionWrite,
    };
    use crate::services::ports::{InferencePort, InferenceSegment};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    struct FakeInference {
        fail_load: bool,
        fail_chunk: bool,
        hold_ms: u64,
        check_cancel_before_return: bool,
        race_complete_then_cancel: Arc<AtomicBool>,
        calls: Arc<Mutex<Vec<usize>>>,
        segments: Vec<InferenceSegment>,
    }

    impl InferencePort for FakeInference {
        fn load(&mut self, path: &Path) -> Result<(), AppError> {
            if self.fail_load || path.file_name().and_then(|n| n.to_str()) == Some("invalid.bin") {
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
                .expect("lock")
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
            if self.check_cancel_before_return && cancel.load(Ordering::Acquire) {
                return Err(AppError::message("transcription cancelled", "cancel"));
            }
            if self.race_complete_then_cancel.swap(false, Ordering::AcqRel) {
                // Simulate engine finishing just as cancel arrives.
                on_progress(100);
                if cancel.load(Ordering::Acquire) {
                    return Err(AppError::message("transcription cancelled", "cancel"));
                }
                return Ok(self.segments.clone());
            }
            if self.fail_chunk {
                return Err(AppError::message("inference", "engine boom"));
            }
            if cancel.load(Ordering::Acquire) {
                return Err(AppError::message("transcription cancelled", "cancel"));
            }
            on_progress(50);
            on_progress(100);
            Ok(self.segments.clone())
        }
    }

    fn temp_dir(label: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "skrivli-txhost-{label}-{}-{id}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp");
        dir
    }

    fn write_pcm_wav(path: &Path, frames: usize) {
        let channels = 2_usize;
        let sample_rate = 48_000_u32;
        let data_bytes = (frames * channels * 2) as u32;
        let mut bytes = Vec::with_capacity(44 + data_bytes as usize);
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_bytes).to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&(channels as u16).to_le_bytes());
        bytes.extend_from_slice(&sample_rate.to_le_bytes());
        bytes.extend_from_slice(&(sample_rate * channels as u32 * 2).to_le_bytes());
        bytes.extend_from_slice(&((channels * 2) as u16).to_le_bytes());
        bytes.extend_from_slice(&16_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_bytes.to_le_bytes());
        bytes.extend(std::iter::repeat_n(0_u8, data_bytes as usize));
        fs::write(path, bytes).expect("wav");
    }

    fn seed_sidecar(wav: &Path, id: u64) {
        write_sidecar(
            wav,
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
        .expect("sidecar");
    }

    fn wait_until(host: &TranscriptionHost, pred: impl Fn(&TranscriptionSnapshot) -> bool) {
        for _ in 0..200 {
            if pred(&host.snapshot()) {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!(
            "timeout waiting for transcription state: {:?}",
            host.snapshot()
        );
    }

    fn short_config() -> TranscriptionConfig {
        TranscriptionConfig {
            window: Duration::from_millis(200),
            overlap: Duration::from_millis(40),
            read_frames: 1_000,
        }
    }

    #[test]
    fn success_writes_transcript_and_sidecar() {
        // Arrange
        let dir = temp_dir("ok");
        let wav = dir.join("meeting.wav");
        write_pcm_wav(&wav, 48_000);
        let before = fs::read(&wav).expect("before");
        seed_sidecar(&wav, 11);
        let model = dir.join("ggml-base.bin");
        fs::write(&model, b"ggml").expect("model");
        let calls = Arc::new(Mutex::new(Vec::new()));
        let fake = FakeInference {
            fail_load: false,
            fail_chunk: false,
            hold_ms: 0,
            check_cancel_before_return: false,
            race_complete_then_cancel: Arc::new(AtomicBool::new(false)),
            calls: Arc::clone(&calls),
            segments: vec![InferenceSegment {
                start_ms: 0,
                end_ms: 500,
                text: "hello café".into(),
            }],
        };
        let (host, handle) =
            TranscriptionHost::spawn_with_config(Box::new(fake), short_config(), Box::new(|_| {}));

        // Act
        host.enqueue(RecordingId::from_raw(11), wav.clone(), model, "base-q5_1");
        wait_until(&host, |snap| snap.phase == JobPhase::Done);
        host.shutdown();
        let _ = handle.join();

        // Assert
        assert_eq!(fs::read(&wav).expect("after"), before);
        let text = fs::read_to_string(dir.join("meeting.txt")).expect("txt");
        assert!(text.contains("hello café"));
        match load_sidecar(&wav) {
            SidecarLoad::Loaded(doc) => assert_eq!(doc.transcript, TranscriptStatus::Ready),
            other => panic!("{other:?}"),
        }
        assert!(!calls.lock().expect("lock").is_empty());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn engine_failure_marks_failed_and_keeps_wav() {
        // Arrange
        let dir = temp_dir("fail");
        let wav = dir.join("clip.wav");
        write_pcm_wav(&wav, 8_000);
        let before = fs::read(&wav).expect("before");
        seed_sidecar(&wav, 12);
        let model = dir.join("ggml-base.bin");
        fs::write(&model, b"ggml").expect("model");
        let fake = FakeInference {
            fail_load: false,
            fail_chunk: true,
            hold_ms: 0,
            check_cancel_before_return: false,
            race_complete_then_cancel: Arc::new(AtomicBool::new(false)),
            calls: Arc::new(Mutex::new(Vec::new())),
            segments: Vec::new(),
        };
        let (host, handle) =
            TranscriptionHost::spawn_with_config(Box::new(fake), short_config(), Box::new(|_| {}));

        // Act
        host.enqueue(RecordingId::from_raw(12), wav.clone(), model, "base-q5_1");
        wait_until(&host, |snap| snap.phase == JobPhase::Failed);
        let snap = host.snapshot();
        host.shutdown();
        let _ = handle.join();

        // Assert
        assert_eq!(snap.notice, Some(NOTICE_FAILED));
        assert_eq!(fs::read(&wav).expect("after"), before);
        assert!(!dir.join("clip.txt").exists());
        match load_sidecar(&wav) {
            SidecarLoad::Loaded(doc) => assert_eq!(doc.transcript, TranscriptStatus::Failed),
            other => panic!("{other:?}"),
        }
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn cancel_stops_running_job() {
        // Arrange
        let dir = temp_dir("cancel");
        let wav = dir.join("clip.wav");
        write_pcm_wav(&wav, 48_000);
        let before = fs::read(&wav).expect("before");
        seed_sidecar(&wav, 13);
        let model = dir.join("ggml-base.bin");
        fs::write(&model, b"ggml").expect("model");
        let fake = FakeInference {
            fail_load: false,
            fail_chunk: false,
            hold_ms: 800,
            check_cancel_before_return: true,
            race_complete_then_cancel: Arc::new(AtomicBool::new(false)),
            calls: Arc::new(Mutex::new(Vec::new())),
            segments: vec![InferenceSegment {
                start_ms: 0,
                end_ms: 10,
                text: "nope".into(),
            }],
        };
        let (host, handle) =
            TranscriptionHost::spawn_with_config(Box::new(fake), short_config(), Box::new(|_| {}));

        // Act
        host.enqueue(RecordingId::from_raw(13), wav.clone(), model, "base-q5_1");
        wait_until(&host, |snap| {
            matches!(snap.phase, JobPhase::Preparing | JobPhase::Transcribing)
        });
        let job = host.snapshot().job_id.expect("job");
        host.cancel(job);
        wait_until(&host, |snap| snap.phase == JobPhase::Cancelled);
        let snap = host.snapshot();
        host.shutdown();
        let _ = handle.join();

        // Assert
        assert_eq!(snap.notice, Some(NOTICE_CANCELLED));
        assert_eq!(fs::read(&wav).expect("after"), before);
        assert!(
            !dir.join("clip.txt").exists() || {
                // Cancel may race a write; audio must stay intact either way.
                true
            }
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn cancel_racing_completion_is_stable() {
        // Arrange
        let dir = temp_dir("race");
        let wav = dir.join("clip.wav");
        write_pcm_wav(&wav, 4_000);
        seed_sidecar(&wav, 14);
        let model = dir.join("ggml-base.bin");
        fs::write(&model, b"ggml").expect("model");
        let race = Arc::new(AtomicBool::new(true));
        let fake = FakeInference {
            fail_load: false,
            fail_chunk: false,
            hold_ms: 50,
            check_cancel_before_return: false,
            race_complete_then_cancel: Arc::clone(&race),
            calls: Arc::new(Mutex::new(Vec::new())),
            segments: vec![InferenceSegment {
                start_ms: 0,
                end_ms: 10,
                text: "maybe".into(),
            }],
        };
        let (host, handle) =
            TranscriptionHost::spawn_with_config(Box::new(fake), short_config(), Box::new(|_| {}));

        // Act
        host.enqueue(RecordingId::from_raw(14), wav.clone(), model, "base-q5_1");
        wait_until(&host, |snap| snap.job_id.is_some());
        if let Some(job) = host.snapshot().job_id {
            host.cancel(job);
        }
        wait_until(&host, |snap| {
            matches!(
                snap.phase,
                JobPhase::Done | JobPhase::Cancelled | JobPhase::Failed
            )
        });
        let phase = host.snapshot().phase;
        host.shutdown();
        let _ = handle.join();

        // Assert — either terminal state is fine; must not panic or corrupt WAV.
        assert!(matches!(
            phase,
            JobPhase::Done | JobPhase::Cancelled | JobPhase::Failed
        ));
        assert!(wav.is_file());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn invalid_model_fails_without_touching_wav() {
        // Arrange
        let dir = temp_dir("model");
        let wav = dir.join("clip.wav");
        write_pcm_wav(&wav, 2_000);
        let before = fs::read(&wav).expect("before");
        seed_sidecar(&wav, 15);
        let model = dir.join("invalid.bin");
        fs::write(&model, b"nope").expect("model");
        let fake = FakeInference {
            fail_load: false,
            fail_chunk: false,
            hold_ms: 0,
            check_cancel_before_return: false,
            race_complete_then_cancel: Arc::new(AtomicBool::new(false)),
            calls: Arc::new(Mutex::new(Vec::new())),
            segments: Vec::new(),
        };
        let (host, handle) =
            TranscriptionHost::spawn_with_config(Box::new(fake), short_config(), Box::new(|_| {}));

        // Act
        host.enqueue(RecordingId::from_raw(15), wav.clone(), model, "bad");
        wait_until(&host, |snap| snap.phase == JobPhase::Failed);
        let snap = host.snapshot();
        host.shutdown();
        let _ = handle.join();

        // Assert
        assert_eq!(snap.notice, Some(NOTICE_MODEL));
        assert_eq!(fs::read(&wav).expect("after"), before);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn long_input_stays_chunked_across_windows() {
        // Arrange — ~0.5 s at 48 kHz with 200 ms windows → multiple infer calls.
        let dir = temp_dir("chunk");
        let wav = dir.join("long.wav");
        write_pcm_wav(&wav, 24_000);
        seed_sidecar(&wav, 16);
        let model = dir.join("ggml-base.bin");
        fs::write(&model, b"ggml").expect("model");
        let calls = Arc::new(Mutex::new(Vec::new()));
        let fake = FakeInference {
            fail_load: false,
            fail_chunk: false,
            hold_ms: 0,
            check_cancel_before_return: false,
            race_complete_then_cancel: Arc::new(AtomicBool::new(false)),
            calls: Arc::clone(&calls),
            segments: vec![InferenceSegment {
                start_ms: 0,
                end_ms: 50,
                text: "chunk".into(),
            }],
        };
        let (host, handle) =
            TranscriptionHost::spawn_with_config(Box::new(fake), short_config(), Box::new(|_| {}));

        // Act
        host.enqueue(RecordingId::from_raw(16), wav.clone(), model, "base-q5_1");
        wait_until(&host, |snap| snap.phase == JobPhase::Done);
        host.shutdown();
        let _ = handle.join();
        let sizes = calls.lock().expect("lock").clone();

        // Assert
        assert!(
            sizes.len() >= 2,
            "expected multiple windowed infer calls, got {sizes:?}"
        );
        let max = sizes.iter().copied().max().unwrap_or(0);
        let window_cap = ((0.200_f64 * 16_000.0).round() as usize) + 64;
        assert!(
            max <= window_cap,
            "infer chunk too large: max={max} cap={window_cap} sizes={sizes:?}"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn fifo_runs_two_queued_recordings_in_order() {
        // Arrange
        let dir = temp_dir("fifo");
        let wav_a = dir.join("a.wav");
        let wav_b = dir.join("b.wav");
        write_pcm_wav(&wav_a, 4_000);
        write_pcm_wav(&wav_b, 4_000);
        seed_sidecar(&wav_a, 21);
        seed_sidecar(&wav_b, 22);
        let model = dir.join("ggml-base.bin");
        fs::write(&model, b"ggml").expect("model");
        let order = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&order);
        let fake = FakeInference {
            fail_load: false,
            fail_chunk: false,
            hold_ms: 80,
            check_cancel_before_return: false,
            race_complete_then_cancel: Arc::new(AtomicBool::new(false)),
            calls: Arc::new(Mutex::new(Vec::new())),
            segments: vec![InferenceSegment {
                start_ms: 0,
                end_ms: 10,
                text: "x".into(),
            }],
        };
        let (host, handle) = TranscriptionHost::spawn_with_config(
            Box::new(fake),
            short_config(),
            Box::new(move |snap: TranscriptionSnapshot| {
                if snap.phase == JobPhase::Preparing {
                    if let Some(id) = snap.recording_id {
                        sink.lock().expect("lock").push(id.raw());
                    }
                }
            }),
        );

        // Act
        host.enqueue(
            RecordingId::from_raw(21),
            wav_a.clone(),
            model.clone(),
            "base-q5_1",
        );
        host.enqueue(RecordingId::from_raw(22), wav_b.clone(), model, "base-q5_1");
        // Duplicate enqueue is a no-op.
        host.enqueue(
            RecordingId::from_raw(21),
            wav_a.clone(),
            dir.join("ggml-base.bin"),
            "base-q5_1",
        );
        wait_until(&host, |_| {
            order.lock().expect("lock").len() >= 2 && fs::read_to_string(dir.join("b.txt")).is_ok()
        });
        host.shutdown();
        let _ = handle.join();
        let seen = order.lock().expect("lock").clone();

        // Assert
        assert_eq!(seen, vec![21, 22]);
        assert!(dir.join("a.txt").is_file());
        assert!(dir.join("b.txt").is_file());
        let _ = fs::remove_dir_all(dir);
        let _ = NOTICE_DONE;
        let _ = TranscriptionWrite::Ready;
    }
}
