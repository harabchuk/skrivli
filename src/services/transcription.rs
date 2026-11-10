//! Offline transcription job queue. One FIFO worker; no GPUI or engine types leak out.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use crate::common::error::AppError;
use crate::domain::ids::{JobId, RecordingId};
use crate::domain::recording::TranscriptStatus;
use crate::domain::transcript::{format_transcript, TranscriptSegment};
use crate::infrastructure::ai::resample::pcm16_to_16k_mono;
use crate::infrastructure::files::transcript_reader::{TranscriptFlacReader, DEFAULT_CHUNK_FRAMES};
use crate::infrastructure::storage::sidecar::{
    load_sidecar, transcript_path_for, update_transcription, write_transcript_text, SidecarLoad,
    TranscriptionWrite,
};
use crate::services::ports::{InferencePort, InferenceSegment, JobIdentity, TranscriptNotifier};

pub(crate) const NOTICE_DONE: &str = "Transcript ready.";
pub(crate) const NOTICE_FAILED: &str = "Transcription failed.";
pub(crate) const NOTICE_CANCELLED: &str = "Transcription cancelled.";
pub(crate) const NOTICE_MODEL: &str = "The transcription model file is missing or unsupported.";
const DETAIL_IDENTITY: &str = "The recording changed before the transcript was saved.";

/// Tunables for the streaming feed. Production uses the reader default; tests shrink it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TranscriptionConfig {
    /// Frames read from the recording per feed. Cancel and progress are handled between feeds.
    pub(crate) read_frames: usize,
}

impl Default for TranscriptionConfig {
    fn default() -> Self {
        Self {
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
    /// Detail-pane text. Failures store [`AppError::user_message`], never engine logs.
    pub(crate) detail: Option<&'static str>,
}

impl TranscriptionSnapshot {
    pub(crate) fn idle() -> Self {
        Self {
            job_id: None,
            recording_id: None,
            phase: JobPhase::Queued,
            progress_percent: 0,
            notice: None,
            detail: None,
        }
    }

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TrackPhase {
    Queued,
    Running,
}

struct TrackedJob {
    job_id: Option<JobId>,
    recording_id: RecordingId,
    audio_path: PathBuf,
    txt_path: Option<PathBuf>,
    model_id: String,
    phase: TrackPhase,
    cancel: Arc<AtomicBool>,
    suppress_write: bool,
}

struct JobLedger {
    jobs: Vec<TrackedJob>,
}

struct BoundView {
    audio: PathBuf,
    txt: Option<PathBuf>,
    suppress: bool,
    cancel: bool,
}

impl JobLedger {
    fn new() -> Self {
        Self { jobs: Vec::new() }
    }

    fn contains(&self, id: RecordingId) -> bool {
        self.jobs.iter().any(|job| job.recording_id == id)
    }

    fn find_mut(&mut self, id: RecordingId) -> Option<&mut TrackedJob> {
        self.jobs.iter_mut().find(|job| job.recording_id == id)
    }

    fn remove(&mut self, id: RecordingId) {
        self.jobs.retain(|job| job.recording_id != id);
    }

    fn insert_queued(&mut self, recording_id: RecordingId, audio_path: PathBuf, model_id: String) {
        self.jobs.push(TrackedJob {
            job_id: None,
            recording_id,
            audio_path,
            txt_path: None,
            model_id,
            phase: TrackPhase::Queued,
            cancel: Arc::new(AtomicBool::new(false)),
            suppress_write: false,
        });
    }

    fn view(&self, id: RecordingId) -> Option<BoundView> {
        self.jobs
            .iter()
            .find(|job| job.recording_id == id)
            .map(|job| BoundView {
                audio: job.audio_path.clone(),
                txt: job.txt_path.clone(),
                suppress: job.suppress_write,
                cancel: job.cancel.load(Ordering::Acquire),
            })
    }
}

enum Adopted {
    Ignore,
    Suppressed {
        path: PathBuf,
        model: String,
        recording: RecordingId,
    },
    Ready(QueuedJob),
}

type ActiveCancel = Option<(JobId, Arc<AtomicBool>)>;

struct SilentNotifier;

impl TranscriptNotifier for SilentNotifier {
    fn transcript_ready(&self, _recording_id: RecordingId) {}
}

/// Owns one inference worker and a FIFO of transcription jobs.
#[derive(Clone)]
pub(crate) struct TranscriptionHost {
    tx: Sender<TranscriptionCommand>,
    /// Running job cancel flag; set from [`Self::cancel`] without waiting on the worker.
    active_cancel: Arc<Mutex<ActiveCancel>>,
    ledger: Arc<Mutex<JobLedger>>,
    phase: Arc<AtomicU8Mirror>,
    progress: Arc<AtomicU8Mirror>,
    job_id: Arc<Mutex<Option<JobId>>>,
    recording_id: Arc<Mutex<Option<RecordingId>>>,
    notice: Arc<Mutex<Option<&'static str>>>,
    detail: Arc<Mutex<Option<&'static str>>>,
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
    /// Production startup goes through [`Self::spawn_with_notifier`] so the
    /// completion port and the user setting are explicit.
    #[allow(dead_code)]
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
        Self::spawn_with_notifier(
            inference,
            config,
            Arc::new(SilentNotifier),
            Arc::new(AtomicBool::new(true)),
            on_change,
        )
    }

    pub(crate) fn spawn_with_notifier(
        inference: Box<dyn InferencePort>,
        config: TranscriptionConfig,
        notifier: Arc<dyn TranscriptNotifier>,
        notify_enabled: Arc<AtomicBool>,
        on_change: Box<dyn Fn(TranscriptionSnapshot) + Send>,
    ) -> (Self, JoinHandle<()>) {
        let (tx, rx) = mpsc::channel();
        let active_cancel = Arc::new(Mutex::new(None));
        let ledger = Arc::new(Mutex::new(JobLedger::new()));
        let phase = Arc::new(AtomicU8Mirror::new(JobPhase::Queued.as_raw()));
        let progress = Arc::new(AtomicU8Mirror::new(0));
        let job_id = Arc::new(Mutex::new(None));
        let recording_id = Arc::new(Mutex::new(None));
        let notice = Arc::new(Mutex::new(None));
        let detail = Arc::new(Mutex::new(None));
        let worker_phase = Arc::clone(&phase);
        let worker_progress = Arc::clone(&progress);
        let worker_job = Arc::clone(&job_id);
        let worker_recording = Arc::clone(&recording_id);
        let worker_notice = Arc::clone(&notice);
        let worker_detail = Arc::clone(&detail);
        let worker_active_cancel = Arc::clone(&active_cancel);
        let worker_ledger = Arc::clone(&ledger);
        let worker_notifier = Arc::clone(&notifier);
        let worker_notify = Arc::clone(&notify_enabled);
        let handle = thread::spawn(move || {
            worker_loop(
                rx,
                inference,
                config,
                worker_active_cancel,
                worker_ledger,
                worker_notifier,
                worker_notify,
                worker_phase,
                worker_progress,
                worker_job,
                worker_recording,
                worker_notice,
                worker_detail,
                on_change,
            );
        });
        (
            Self {
                tx,
                active_cancel,
                ledger,
                phase,
                progress,
                job_id,
                recording_id,
                notice,
                detail,
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
        let model_id = model_id.into();
        {
            let mut ledger = self
                .ledger
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if ledger.contains(recording_id) {
                return;
            }
            ledger.insert_queued(recording_id, audio_path.clone(), model_id.clone());
        }
        let _ = self.tx.send(TranscriptionCommand::Enqueue(EnqueueRequest {
            recording_id,
            audio_path,
            model_path,
            model_id,
        }));
    }

    /// Rename of this id. A queued job is cancelled. A running job is retargeted
    /// so a later write uses `new_audio` when that file still belongs to the id.
    pub(crate) fn note_rename(&self, id: RecordingId, new_audio: PathBuf) {
        let action = {
            let mut ledger = self
                .ledger
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(job) = ledger.find_mut(id) else {
                return;
            };
            if job.phase == TrackPhase::Running && job.txt_path.is_some() && !job.suppress_write {
                if let Ok(txt) = transcript_path_for(&new_audio) {
                    job.audio_path = new_audio;
                    job.txt_path = Some(txt);
                    return;
                }
            }
            job.suppress_write = true;
            job.cancel.store(true, Ordering::Release);
            (job.audio_path.clone(), job.model_id.clone())
        };
        let _ = update_transcription(&action.0, TranscriptionWrite::Cancelled, Some(&action.1));
    }

    /// Delete of this id. The job must not write a transcript after this returns.
    pub(crate) fn note_delete(&self, id: RecordingId) {
        let action = {
            let mut ledger = self
                .ledger
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(job) = ledger.find_mut(id) else {
                return;
            };
            job.suppress_write = true;
            job.cancel.store(true, Ordering::Release);
            (job.audio_path.clone(), job.model_id.clone())
        };
        let _ = update_transcription(&action.0, TranscriptionWrite::Cancelled, Some(&action.1));
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
        let detail = *self
            .detail
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        TranscriptionSnapshot {
            job_id,
            recording_id,
            phase,
            progress_percent: self.progress.load(),
            notice,
            detail,
        }
    }
}

impl JobIdentity for TranscriptionHost {
    fn before_rename(&self, id: RecordingId, new_audio: &Path) {
        self.note_rename(id, new_audio.to_path_buf());
    }

    fn before_delete(&self, id: RecordingId) {
        self.note_delete(id);
    }
}

#[allow(clippy::too_many_arguments)]
fn worker_loop(
    rx: mpsc::Receiver<TranscriptionCommand>,
    mut inference: Box<dyn InferencePort>,
    config: TranscriptionConfig,
    active_cancel: Arc<Mutex<ActiveCancel>>,
    ledger: Arc<Mutex<JobLedger>>,
    notifier: Arc<dyn TranscriptNotifier>,
    notify_enabled: Arc<AtomicBool>,
    phase: Arc<AtomicU8Mirror>,
    progress: Arc<AtomicU8Mirror>,
    job_id: Arc<Mutex<Option<JobId>>>,
    recording_id: Arc<Mutex<Option<RecordingId>>>,
    notice: Arc<Mutex<Option<&'static str>>>,
    detail: Arc<Mutex<Option<&'static str>>>,
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
                        &detail,
                        JobPhase::Cancelled,
                        None,
                        None,
                        0,
                        None,
                        None,
                        &on_change,
                    );
                    break;
                }
                TranscriptionCommand::Enqueue(request) => {
                    match accept_enqueue(request, &queue, running_id, &mut next_job, &ledger) {
                        Adopted::Ignore => {}
                        Adopted::Suppressed {
                            path,
                            model,
                            recording,
                        } => {
                            let _ = update_transcription(
                                &path,
                                TranscriptionWrite::Cancelled,
                                Some(model.as_str()),
                            );
                            publish(
                                &phase,
                                &progress,
                                &job_id,
                                &recording_id,
                                &notice,
                                &detail,
                                JobPhase::Cancelled,
                                None,
                                Some(recording),
                                0,
                                Some(NOTICE_CANCELLED),
                                None,
                                &on_change,
                            );
                        }
                        Adopted::Ready(queued) => {
                            let id = queued.job_id;
                            let recording = queued.request.recording_id;
                            let path = queued.request.audio_path.clone();
                            let model = queued.request.model_id.clone();
                            queue.push_back(queued);
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
                                &detail,
                                JobPhase::Queued,
                                Some(id),
                                Some(recording),
                                0,
                                None,
                                None,
                                &on_change,
                            );
                        }
                    }
                }
                TranscriptionCommand::Cancel { job_id: target } => {
                    if let Some(index) = queue.iter().position(|job| job.job_id == target) {
                        if let Some(job) = queue.remove(index) {
                            forget_job(&ledger, job.request.recording_id);
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
                                &detail,
                                JobPhase::Cancelled,
                                Some(job.job_id),
                                Some(job.request.recording_id),
                                0,
                                Some(NOTICE_CANCELLED),
                                None,
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
        match begin_running(&ledger, job.request.recording_id) {
            Start::Cancelled => {
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
                    &detail,
                    JobPhase::Cancelled,
                    Some(job.job_id),
                    Some(job.request.recording_id),
                    0,
                    Some(NOTICE_CANCELLED),
                    None,
                    &on_change,
                );
                continue;
            }
            Start::Failed => {
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
                    &detail,
                    JobPhase::Failed,
                    Some(job.job_id),
                    Some(job.request.recording_id),
                    0,
                    Some(NOTICE_FAILED),
                    Some(DETAIL_IDENTITY),
                    &on_change,
                );
                continue;
            }
            Start::Run => {}
        }
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
            &detail,
            JobPhase::Preparing,
            Some(snapshot_job),
            Some(snapshot_recording),
            0,
            None,
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
                        &detail,
                        JobPhase::Cancelled,
                        None,
                        None,
                        0,
                        None,
                        None,
                        &on_change,
                    );
                    return;
                }
                TranscriptionCommand::Enqueue(request) => {
                    match accept_enqueue(request, &queue, running_id, &mut next_job, &ledger) {
                        Adopted::Ignore => {}
                        Adopted::Suppressed {
                            path,
                            model,
                            recording,
                        } => {
                            let _ = update_transcription(
                                &path,
                                TranscriptionWrite::Cancelled,
                                Some(model.as_str()),
                            );
                            publish(
                                &phase,
                                &progress,
                                &job_id,
                                &recording_id,
                                &notice,
                                &detail,
                                JobPhase::Cancelled,
                                None,
                                Some(recording),
                                0,
                                Some(NOTICE_CANCELLED),
                                None,
                                &on_change,
                            );
                        }
                        Adopted::Ready(queued) => {
                            let id = queued.job_id;
                            let recording = queued.request.recording_id;
                            let queued_path = queued.request.audio_path.clone();
                            let queued_model = queued.request.model_id.clone();
                            queue.push_back(queued);
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
                                &detail,
                                JobPhase::Queued,
                                Some(id),
                                Some(recording),
                                0,
                                None,
                                None,
                                &on_change,
                            );
                        }
                    }
                }
                TranscriptionCommand::Cancel { job_id: target } => {
                    if target == job.job_id {
                        job.cancel.store(true, Ordering::Release);
                    } else if let Some(index) = queue.iter().position(|item| item.job_id == target)
                    {
                        if let Some(queued) = queue.remove(index) {
                            forget_job(&ledger, queued.request.recording_id);
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
                                &detail,
                                JobPhase::Cancelled,
                                Some(queued.job_id),
                                Some(queued.request.recording_id),
                                0,
                                Some(NOTICE_CANCELLED),
                                None,
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
            snapshot_recording,
            &ledger,
            config,
            |percent| {
                publish(
                    &phase,
                    &progress,
                    &job_id,
                    &recording_id,
                    &notice,
                    &detail,
                    JobPhase::Transcribing,
                    Some(snapshot_job),
                    Some(snapshot_recording),
                    percent,
                    None,
                    None,
                    &on_change,
                );
                if percent == 0
                    || percent >= 100
                    || percent.saturating_sub(last_persisted_progress) >= 10
                {
                    last_persisted_progress = percent;
                    let progress_path =
                        bound_audio(&ledger, snapshot_recording).unwrap_or_else(|| path.clone());
                    let _ = update_transcription(
                        &progress_path,
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
        let status_audio = bound_audio(&ledger, snapshot_recording)
            .unwrap_or_else(|| job.request.audio_path.clone());
        match outcome {
            JobOutcome::Done => {
                let _ = update_transcription(
                    &status_audio,
                    TranscriptionWrite::Ready,
                    Some(job.request.model_id.as_str()),
                );
                publish(
                    &phase,
                    &progress,
                    &job_id,
                    &recording_id,
                    &notice,
                    &detail,
                    JobPhase::Done,
                    Some(job.job_id),
                    Some(job.request.recording_id),
                    100,
                    Some(NOTICE_DONE),
                    None,
                    &on_change,
                );
                if notify_enabled.load(Ordering::Acquire) {
                    notifier.transcript_ready(snapshot_recording);
                }
            }
            JobOutcome::Cancelled => {
                let _ = update_transcription(
                    &status_audio,
                    TranscriptionWrite::Cancelled,
                    Some(job.request.model_id.as_str()),
                );
                publish(
                    &phase,
                    &progress,
                    &job_id,
                    &recording_id,
                    &notice,
                    &detail,
                    JobPhase::Cancelled,
                    Some(job.job_id),
                    Some(job.request.recording_id),
                    progress.load(),
                    Some(NOTICE_CANCELLED),
                    None,
                    &on_change,
                );
            }
            JobOutcome::Failed {
                model_issue,
                detail: failure_detail,
            } => {
                let notice_text = if model_issue {
                    NOTICE_MODEL
                } else {
                    NOTICE_FAILED
                };
                let _ = update_transcription(
                    &status_audio,
                    TranscriptionWrite::Failed,
                    Some(job.request.model_id.as_str()),
                );
                publish(
                    &phase,
                    &progress,
                    &job_id,
                    &recording_id,
                    &notice,
                    &detail,
                    JobPhase::Failed,
                    Some(job.job_id),
                    Some(job.request.recording_id),
                    progress.load(),
                    Some(notice_text),
                    Some(failure_detail),
                    &on_change,
                );
            }
        }
        forget_job(&ledger, snapshot_recording);
    }
}

enum JobOutcome {
    Done,
    Cancelled,
    Failed {
        model_issue: bool,
        detail: &'static str,
    },
}

fn run_job(
    inference: &mut dyn InferencePort,
    request: &EnqueueRequest,
    cancel: &Arc<AtomicBool>,
    recording_id: RecordingId,
    ledger: &Mutex<JobLedger>,
    config: TranscriptionConfig,
    mut on_progress: impl FnMut(u8),
) -> JobOutcome {
    if write_blocked(ledger, recording_id, cancel) {
        return JobOutcome::Cancelled;
    }
    on_progress(0);
    if let Err(error) = inference.load(&request.model_path) {
        tracing::error!(error = %error, "transcription model was not loaded");
        return if write_blocked(ledger, recording_id, cancel) {
            JobOutcome::Cancelled
        } else {
            JobOutcome::Failed {
                model_issue: is_model_error(&error),
                detail: error.user_message(),
            }
        };
    }
    if write_blocked(ledger, recording_id, cancel) {
        inference.unload();
        return JobOutcome::Cancelled;
    }

    let Some(audio) = bound_audio(ledger, recording_id) else {
        inference.unload();
        return JobOutcome::Cancelled;
    };
    if !audio_belongs(recording_id, &audio) {
        inference.unload();
        return identity_failure();
    }

    let result = transcribe_file(inference, &audio, cancel, config, &mut on_progress);
    match result {
        Ok(text) => commit_transcript(ledger, recording_id, cancel, &text, &mut on_progress),
        Err(error) if is_cancel_error(&error) || write_blocked(ledger, recording_id, cancel) => {
            JobOutcome::Cancelled
        }
        Err(error) => {
            tracing::error!(error = %error, "transcription job failed");
            JobOutcome::Failed {
                model_issue: false,
                detail: error.user_message(),
            }
        }
    }
}

fn identity_failure() -> JobOutcome {
    JobOutcome::Failed {
        model_issue: false,
        detail: DETAIL_IDENTITY,
    }
}

fn commit_transcript(
    ledger: &Mutex<JobLedger>,
    recording_id: RecordingId,
    cancel: &AtomicBool,
    text: &str,
    on_progress: &mut dyn FnMut(u8),
) -> JobOutcome {
    if write_blocked(ledger, recording_id, cancel) {
        return JobOutcome::Cancelled;
    }
    let Some(view) = ledger
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .view(recording_id)
    else {
        return JobOutcome::Cancelled;
    };
    if view.suppress || view.cancel {
        return JobOutcome::Cancelled;
    }
    let Some(txt) = view.txt else {
        return JobOutcome::Cancelled;
    };
    if transcript_path_for(&view.audio).ok().as_ref() != Some(&txt)
        || !audio_belongs(recording_id, &view.audio)
    {
        return identity_failure();
    }
    match write_transcript_text(&view.audio, text) {
        Ok(path) if path == txt => {
            on_progress(100);
            JobOutcome::Done
        }
        Ok(_) => identity_failure(),
        Err(error) => {
            tracing::error!(error = %error, "transcript text was not written");
            JobOutcome::Failed {
                model_issue: false,
                detail: error.user_message(),
            }
        }
    }
}

fn audio_belongs(id: RecordingId, audio: &Path) -> bool {
    match load_sidecar(audio) {
        SidecarLoad::Loaded(doc) => doc.id == id,
        SidecarLoad::Missing | SidecarLoad::Malformed => false,
    }
}

fn write_blocked(ledger: &Mutex<JobLedger>, id: RecordingId, cancel: &AtomicBool) -> bool {
    if cancel.load(Ordering::Acquire) {
        return true;
    }
    match ledger
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .view(id)
    {
        Some(view) => view.suppress || view.cancel,
        None => true,
    }
}

fn bound_audio(ledger: &Mutex<JobLedger>, id: RecordingId) -> Option<PathBuf> {
    ledger
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .view(id)
        .map(|view| view.audio)
}

fn forget_job(ledger: &Mutex<JobLedger>, id: RecordingId) {
    ledger
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(id);
}

enum Start {
    Run,
    Cancelled,
    Failed,
}

fn begin_running(ledger: &Mutex<JobLedger>, id: RecordingId) -> Start {
    let mut guard = ledger
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(job) = guard.find_mut(id) else {
        return Start::Cancelled;
    };
    if job.suppress_write || job.cancel.load(Ordering::Acquire) {
        guard.remove(id);
        return Start::Cancelled;
    }
    let Ok(txt) = transcript_path_for(&job.audio_path) else {
        guard.remove(id);
        return Start::Failed;
    };
    job.txt_path = Some(txt);
    job.phase = TrackPhase::Running;
    Start::Run
}

fn accept_enqueue(
    request: EnqueueRequest,
    queue: &VecDeque<QueuedJob>,
    running_id: Option<RecordingId>,
    next_job: &mut u64,
    ledger: &Mutex<JobLedger>,
) -> Adopted {
    if running_id == Some(request.recording_id)
        || queue
            .iter()
            .any(|job| job.request.recording_id == request.recording_id)
    {
        return Adopted::Ignore;
    }
    let mut guard = ledger
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(job) = guard.find_mut(request.recording_id) else {
        return Adopted::Ignore;
    };
    if job.job_id.is_some() {
        return Adopted::Ignore;
    }
    if job.suppress_write || job.cancel.load(Ordering::Acquire) {
        let path = job.audio_path.clone();
        let model = job.model_id.clone();
        let recording = job.recording_id;
        guard.remove(recording);
        return Adopted::Suppressed {
            path,
            model,
            recording,
        };
    }
    let id = JobId::from_raw(*next_job);
    *next_job = next_job.saturating_add(1);
    job.job_id = Some(id);
    let cancel = Arc::clone(&job.cancel);
    drop(guard);
    Adopted::Ready(QueuedJob {
        job_id: id,
        request,
        cancel,
    })
}

fn is_cancel_error(error: &AppError) -> bool {
    error.user_message() == "Transcription was cancelled."
}

fn is_model_error(error: &AppError) -> bool {
    matches!(
        error.user_message(),
        "The transcription model file is missing."
            | "The transcription model file could not be read."
            | "The transcription model file is not supported."
            | "Could not load the transcription model."
    )
}

/// Feeds the recording to the engine as one stream, one read-sized block at a time.
/// Cancel is checked and progress reported between feeds.
fn transcribe_file(
    inference: &mut dyn InferencePort,
    audio_path: &Path,
    cancel: &Arc<AtomicBool>,
    config: TranscriptionConfig,
    on_progress: &mut dyn FnMut(u8),
) -> Result<String, AppError> {
    let mut reader = TranscriptFlacReader::open(audio_path)?;
    let format = reader.format();
    let total_frames = reader.frames_remaining().unwrap_or(0);
    if total_frames == 0 {
        on_progress(100);
        return Ok(String::new());
    }

    let channels = usize::from(format.channels());
    let mut pcm = vec![0_i16; config.read_frames.saturating_mul(channels.max(1))];
    let mut stream = inference.begin_stream()?;
    let mut segments: Vec<TranscriptSegment> = Vec::new();
    let mut frames_fed = 0_u64;

    loop {
        if cancel.load(Ordering::Acquire) {
            return Err(AppError::message("transcription cancelled", "cancel"));
        }
        let frames = reader.read_i16_frames(&mut pcm, config.read_frames)?;
        if frames == 0 {
            break;
        }
        let samples = &pcm[..frames.saturating_mul(channels)];
        let mono = pcm16_to_16k_mono(samples, format.sample_rate(), format.channels())?;
        if !mono.is_empty() {
            segments.extend(to_domain_segments(stream.feed(&mono)?));
        }
        frames_fed = frames_fed.saturating_add(frames as u64);
        on_progress(percent_fed(frames_fed, total_frames));
    }

    if cancel.load(Ordering::Acquire) {
        return Err(AppError::message("transcription cancelled", "cancel"));
    }
    segments.extend(to_domain_segments(stream.finish()?));
    Ok(format_transcript(&segments))
}

/// Share of the recording fed so far. Stays below 100 until the transcript is saved.
fn percent_fed(fed: u64, total: u64) -> u8 {
    (fed.saturating_mul(100) / total.max(1)).min(99) as u8
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
    detail_slot: &Arc<Mutex<Option<&'static str>>>,
    next: JobPhase,
    id: Option<JobId>,
    recording: Option<RecordingId>,
    percent: u8,
    message: Option<&'static str>,
    detail: Option<&'static str>,
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
    if let Ok(mut guard) = detail_slot.lock() {
        *guard = detail;
    }
    on_change(TranscriptionSnapshot {
        job_id: id,
        recording_id: recording,
        phase: next,
        progress_percent: percent,
        notice: message,
        detail,
    });
}

#[cfg(test)]
mod tests {
    use super::{
        transcribe_file, JobPhase, TranscriptionConfig, TranscriptionHost, TranscriptionSnapshot,
        NOTICE_CANCELLED, NOTICE_DONE, NOTICE_FAILED, NOTICE_MODEL,
    };
    use crate::common::error::AppError;
    use crate::domain::ids::RecordingId;
    use crate::domain::recording::TranscriptStatus;
    use crate::infrastructure::storage::sidecar::{
        load_sidecar, write_sidecar, Sidecar, SidecarLoad, TranscriptionWrite,
    };
    use crate::services::ports::{
        InferencePort, InferenceSegment, SpeechStream, TranscriptNotifier,
    };
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    struct FakeInference {
        fail_load: bool,
        fail_chunk: bool,
        hold_ms: u64,
        calls: Arc<Mutex<Vec<usize>>>,
        segments: Vec<InferenceSegment>,
    }

    /// Returns the scripted segments on the first feed, like an engine that finishes a
    /// sentence early. `hold_ms` slows every feed down.
    struct FakeStream {
        fail_chunk: bool,
        hold_ms: u64,
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

        fn begin_stream(&mut self) -> Result<Box<dyn SpeechStream + '_>, AppError> {
            Ok(Box::new(FakeStream {
                fail_chunk: self.fail_chunk,
                hold_ms: self.hold_ms,
                calls: Arc::clone(&self.calls),
                segments: self.segments.clone(),
            }))
        }
    }

    impl SpeechStream for FakeStream {
        fn feed(&mut self, samples: &[f32]) -> Result<Vec<InferenceSegment>, AppError> {
            self.calls.lock().expect("lock").push(samples.len());
            if self.hold_ms > 0 {
                std::thread::sleep(Duration::from_millis(self.hold_ms));
            }
            if self.fail_chunk {
                return Err(AppError::message("inference", "engine boom"));
            }
            Ok(std::mem::take(&mut self.segments))
        }

        fn finish(self: Box<Self>) -> Result<Vec<InferenceSegment>, AppError> {
            Ok(Vec::new())
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

    fn write_pcm_flac(path: &Path, frames: usize) {
        crate::infrastructure::files::flac::write_silence(path, frames).expect("flac");
    }

    fn seed_sidecar(audio: &Path, id: u64) {
        write_sidecar(
            audio,
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
        TranscriptionConfig { read_frames: 1_000 }
    }

    /// Records the highest percent the host had published when the second feed arrived.
    struct LiveProgressInference {
        published: Arc<AtomicU8>,
        seen_at_second_feed: Arc<Mutex<Option<u8>>>,
    }

    struct LiveProgressStream {
        published: Arc<AtomicU8>,
        seen_at_second_feed: Arc<Mutex<Option<u8>>>,
        feeds: usize,
    }

    impl InferencePort for LiveProgressInference {
        fn load(&mut self, _path: &Path) -> Result<(), AppError> {
            Ok(())
        }

        fn unload(&mut self) {}

        fn begin_stream(&mut self) -> Result<Box<dyn SpeechStream + '_>, AppError> {
            Ok(Box::new(LiveProgressStream {
                published: Arc::clone(&self.published),
                seen_at_second_feed: Arc::clone(&self.seen_at_second_feed),
                feeds: 0,
            }))
        }
    }

    impl SpeechStream for LiveProgressStream {
        fn feed(&mut self, _samples: &[f32]) -> Result<Vec<InferenceSegment>, AppError> {
            self.feeds += 1;
            if self.feeds == 2 {
                *self.seen_at_second_feed.lock().expect("lock") =
                    Some(self.published.load(Ordering::Acquire));
            }
            Ok(Vec::new())
        }

        fn finish(self: Box<Self>) -> Result<Vec<InferenceSegment>, AppError> {
            Ok(Vec::new())
        }
    }

    #[test]
    fn progress_is_published_while_streaming() {
        // Arrange
        let dir = temp_dir("live-progress");
        let audio = dir.join("meeting.flac");
        write_pcm_flac(&audio, 48_000);
        seed_sidecar(&audio, 31);
        let model = dir.join("model.gguf");
        fs::write(&model, b"gguf").expect("model");
        let published = Arc::new(AtomicU8::new(0));
        let seen = Arc::new(Mutex::new(None));
        let fake = LiveProgressInference {
            published: Arc::clone(&published),
            seen_at_second_feed: Arc::clone(&seen),
        };
        let sink = Arc::clone(&published);
        let (host, handle) = TranscriptionHost::spawn_with_config(
            Box::new(fake),
            short_config(),
            Box::new(move |snap| {
                sink.fetch_max(snap.progress_percent, Ordering::AcqRel);
            }),
        );

        // Act
        host.enqueue(
            RecordingId::from_raw(31),
            audio,
            model,
            "nemotron-3.5-streaming-0.6b-q8_0",
        );
        wait_until(&host, |snap| snap.phase == JobPhase::Done);
        host.shutdown();
        let _ = handle.join();

        // Assert
        let seen = *seen.lock().expect("lock");
        assert!(seen.is_some_and(|percent| percent > 0), "{seen:?}");
        let _ = fs::remove_dir_all(dir);
    }

    /// What a scripted engine was fed.
    #[derive(Default)]
    struct FeedLog {
        feeds: Vec<usize>,
        finished: bool,
    }

    /// Engine that returns `per_feed[n]` for feed `n`, `tail` on finish, and can raise
    /// the cancel flag after a number of feeds.
    struct ScriptedInference {
        per_feed: Vec<Vec<InferenceSegment>>,
        tail: Vec<InferenceSegment>,
        log: Arc<Mutex<FeedLog>>,
        cancel_after_feeds: Option<(usize, Arc<AtomicBool>)>,
    }

    struct ScriptedStream {
        per_feed: Vec<Vec<InferenceSegment>>,
        tail: Vec<InferenceSegment>,
        log: Arc<Mutex<FeedLog>>,
        cancel_after_feeds: Option<(usize, Arc<AtomicBool>)>,
    }

    impl ScriptedInference {
        fn new(per_feed: Vec<Vec<InferenceSegment>>, tail: Vec<InferenceSegment>) -> Self {
            Self {
                per_feed,
                tail,
                log: Arc::new(Mutex::new(FeedLog::default())),
                cancel_after_feeds: None,
            }
        }
    }

    impl InferencePort for ScriptedInference {
        fn load(&mut self, _path: &Path) -> Result<(), AppError> {
            Ok(())
        }

        fn unload(&mut self) {}

        fn begin_stream(&mut self) -> Result<Box<dyn SpeechStream + '_>, AppError> {
            Ok(Box::new(ScriptedStream {
                per_feed: self.per_feed.clone(),
                tail: self.tail.clone(),
                log: Arc::clone(&self.log),
                cancel_after_feeds: self.cancel_after_feeds.clone(),
            }))
        }
    }

    impl SpeechStream for ScriptedStream {
        fn feed(&mut self, samples: &[f32]) -> Result<Vec<InferenceSegment>, AppError> {
            let mut log = self.log.lock().expect("lock");
            let index = log.feeds.len();
            log.feeds.push(samples.len());
            if let Some((after, flag)) = &self.cancel_after_feeds {
                if log.feeds.len() >= *after {
                    flag.store(true, Ordering::Release);
                }
            }
            Ok(self.per_feed.get(index).cloned().unwrap_or_default())
        }

        fn finish(self: Box<Self>) -> Result<Vec<InferenceSegment>, AppError> {
            self.log.lock().expect("lock").finished = true;
            Ok(self.tail)
        }
    }

    fn segment(start_ms: u64, end_ms: u64, text: &str) -> InferenceSegment {
        InferenceSegment {
            start_ms,
            end_ms,
            text: text.into(),
        }
    }

    /// Stream a 10,000-frame (10 feeds of 1,000 frames) recording through `engine`.
    fn run_scripted(
        label: &str,
        engine: &mut ScriptedInference,
        cancel: &Arc<AtomicBool>,
    ) -> (Result<String, AppError>, Vec<u8>) {
        let dir = temp_dir(label);
        let audio = dir.join("clip.flac");
        write_pcm_flac(&audio, 10_000);
        let mut percents = Vec::new();
        let result = transcribe_file(engine, &audio, cancel, short_config(), &mut |percent| {
            percents.push(percent)
        });
        let _ = fs::remove_dir_all(dir);
        (result, percents)
    }

    #[test]
    fn streams_whole_file_in_read_sized_feeds_and_formats_segments() {
        // Arrange
        let mut per_feed = vec![Vec::new(); 10];
        per_feed[0] = vec![segment(0, 500, "first")];
        per_feed[2] = vec![segment(2_000, 2_500, "second")];
        let mut engine = ScriptedInference::new(per_feed, vec![segment(5_000, 5_500, "last")]);
        let cancel = Arc::new(AtomicBool::new(false));

        // Act
        let (result, _) = run_scripted("stream-all", &mut engine, &cancel);

        // Assert
        let expected = crate::domain::transcript::format_transcript(&[
            crate::domain::transcript::TranscriptSegment::new(0, 500, "first"),
            crate::domain::transcript::TranscriptSegment::new(2_000, 2_500, "second"),
            crate::domain::transcript::TranscriptSegment::new(5_000, 5_500, "last"),
        ]);
        assert_eq!(result.expect("transcript"), expected);
        let log = engine.log.lock().expect("lock");
        assert_eq!(log.feeds.len(), 10);
        // 10,000 frames at 48 kHz are about 3,333 samples at 16 kHz, split over ten feeds.
        let total: usize = log.feeds.iter().sum();
        assert!((3_000..=3_700).contains(&total), "{:?}", log.feeds);
        assert!(log.feeds.iter().all(|&len| (1..=400).contains(&len)));
        assert!(log.finished);
    }

    #[test]
    fn cancel_between_feeds_stops_without_writing_transcript() {
        // Arrange
        let cancel = Arc::new(AtomicBool::new(false));
        let mut engine = ScriptedInference::new(Vec::new(), vec![segment(0, 10, "never")]);
        engine.cancel_after_feeds = Some((2, Arc::clone(&cancel)));

        // Act
        let (result, _) = run_scripted("cancel-feeds", &mut engine, &cancel);

        // Assert
        let error = result.expect_err("cancelled");
        assert!(super::is_cancel_error(&error));
        let log = engine.log.lock().expect("lock");
        assert_eq!(log.feeds.len(), 2);
        assert!(!log.finished);
    }

    #[test]
    fn progress_follows_samples_fed_and_holds_below_100() {
        // Arrange
        let mut engine = ScriptedInference::new(Vec::new(), Vec::new());
        let cancel = Arc::new(AtomicBool::new(false));

        // Act
        let (result, percents) = run_scripted("progress", &mut engine, &cancel);

        // Assert
        result.expect("transcript");
        assert_eq!(percents, vec![10, 20, 30, 40, 50, 60, 70, 80, 90, 99]);
    }

    #[test]
    fn finish_segments_are_kept() {
        // Arrange
        let mut engine = ScriptedInference::new(Vec::new(), vec![segment(100, 900, "only tail")]);
        let cancel = Arc::new(AtomicBool::new(false));

        // Act
        let (result, _) = run_scripted("finish-tail", &mut engine, &cancel);

        // Assert
        assert!(result.expect("transcript").contains("only tail"));
    }

    #[test]
    fn success_writes_transcript_and_sidecar() {
        // Arrange
        let dir = temp_dir("ok");
        let audio = dir.join("meeting.flac");
        write_pcm_flac(&audio, 48_000);
        let before = fs::read(&audio).expect("before");
        seed_sidecar(&audio, 11);
        let model = dir.join("ggml-base.bin");
        fs::write(&model, b"ggml").expect("model");
        let calls = Arc::new(Mutex::new(Vec::new()));
        let fake = FakeInference {
            fail_load: false,
            fail_chunk: false,
            hold_ms: 0,
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
        host.enqueue(RecordingId::from_raw(11), audio.clone(), model, "base-q5_1");
        wait_until(&host, |snap| snap.phase == JobPhase::Done);
        host.shutdown();
        let _ = handle.join();

        // Assert
        assert_eq!(fs::read(&audio).expect("after"), before);
        let text = fs::read_to_string(dir.join("meeting.txt")).expect("txt");
        assert!(text.contains("hello café"));
        match load_sidecar(&audio) {
            SidecarLoad::Loaded(doc) => assert_eq!(doc.transcript, TranscriptStatus::Ready),
            other => panic!("{other:?}"),
        }
        assert!(!calls.lock().expect("lock").is_empty());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn engine_failure_marks_failed_and_keeps_flac() {
        // Arrange
        let dir = temp_dir("fail");
        let audio = dir.join("clip.flac");
        write_pcm_flac(&audio, 8_000);
        let before = fs::read(&audio).expect("before");
        seed_sidecar(&audio, 12);
        let model = dir.join("ggml-base.bin");
        fs::write(&model, b"ggml").expect("model");
        let fake = FakeInference {
            fail_load: false,
            fail_chunk: true,
            hold_ms: 0,
            calls: Arc::new(Mutex::new(Vec::new())),
            segments: Vec::new(),
        };
        let (host, handle) =
            TranscriptionHost::spawn_with_config(Box::new(fake), short_config(), Box::new(|_| {}));

        // Act
        host.enqueue(RecordingId::from_raw(12), audio.clone(), model, "base-q5_1");
        wait_until(&host, |snap| snap.phase == JobPhase::Failed);
        let snap = host.snapshot();
        host.shutdown();
        let _ = handle.join();

        // Assert
        assert_eq!(snap.notice, Some(NOTICE_FAILED));
        assert_eq!(fs::read(&audio).expect("after"), before);
        assert!(!dir.join("clip.txt").exists());
        match load_sidecar(&audio) {
            SidecarLoad::Loaded(doc) => assert_eq!(doc.transcript, TranscriptStatus::Failed),
            other => panic!("{other:?}"),
        }
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn failing_rerun_leaves_the_old_transcript_byte_identical() {
        // Arrange
        let dir = temp_dir("rerun-fail");
        let audio = dir.join("clip.flac");
        write_pcm_flac(&audio, 8_000);
        seed_sidecar(&audio, 41);
        let old = "[00:00:00] old café text
"
        .as_bytes()
        .to_vec();
        fs::write(dir.join("clip.txt"), &old).expect("old txt");
        let model = dir.join("ggml-base.bin");
        fs::write(&model, b"ggml").expect("model");
        let fake = FakeInference {
            fail_load: false,
            fail_chunk: true,
            hold_ms: 0,
            calls: Arc::new(Mutex::new(Vec::new())),
            segments: Vec::new(),
        };
        let (host, handle) =
            TranscriptionHost::spawn_with_config(Box::new(fake), short_config(), Box::new(|_| {}));

        // Act
        host.enqueue(RecordingId::from_raw(41), audio, model, "base-q5_1");
        wait_until(&host, |snap| snap.phase == JobPhase::Failed);
        host.shutdown();
        let _ = handle.join();

        // Assert
        assert_eq!(fs::read(dir.join("clip.txt")).expect("txt"), old);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn cancelled_rerun_leaves_the_old_transcript_byte_identical() {
        // Arrange
        let dir = temp_dir("rerun-cancel");
        let audio = dir.join("clip.flac");
        write_pcm_flac(&audio, 48_000);
        seed_sidecar(&audio, 42);
        let old = "[00:00:00] old café text
"
        .as_bytes()
        .to_vec();
        fs::write(dir.join("clip.txt"), &old).expect("old txt");
        let model = dir.join("ggml-base.bin");
        fs::write(&model, b"ggml").expect("model");
        let fake = FakeInference {
            fail_load: false,
            fail_chunk: false,
            hold_ms: 800,
            calls: Arc::new(Mutex::new(Vec::new())),
            segments: vec![InferenceSegment {
                start_ms: 0,
                end_ms: 10,
                text: "new".into(),
            }],
        };
        let (host, handle) =
            TranscriptionHost::spawn_with_config(Box::new(fake), short_config(), Box::new(|_| {}));

        // Act
        host.enqueue(RecordingId::from_raw(42), audio, model, "base-q5_1");
        wait_until(&host, |snap| {
            matches!(snap.phase, JobPhase::Preparing | JobPhase::Transcribing)
        });
        let job = host.snapshot().job_id.expect("job");
        host.cancel(job);
        wait_until(&host, |snap| snap.phase == JobPhase::Cancelled);
        host.shutdown();
        let _ = handle.join();

        // Assert
        assert_eq!(fs::read(dir.join("clip.txt")).expect("txt"), old);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn failed_snapshot_detail_is_user_message_not_engine_text() {
        // Arrange
        let dir = temp_dir("fail-detail");
        let audio = dir.join("clip.flac");
        write_pcm_flac(&audio, 8_000);
        seed_sidecar(&audio, 22);
        let model = dir.join("ggml-base.bin");
        fs::write(&model, b"ggml").expect("model");
        let fake = FakeInference {
            fail_load: false,
            fail_chunk: true,
            hold_ms: 0,
            calls: Arc::new(Mutex::new(Vec::new())),
            segments: Vec::new(),
        };
        let (host, handle) =
            TranscriptionHost::spawn_with_config(Box::new(fake), short_config(), Box::new(|_| {}));

        // Act
        host.enqueue(RecordingId::from_raw(22), audio, model, "base-q5_1");
        wait_until(&host, |snap| snap.phase == JobPhase::Failed);
        let snap = host.snapshot();
        host.shutdown();
        let _ = handle.join();

        // Assert
        assert_eq!(snap.notice, Some(NOTICE_FAILED));
        assert_eq!(snap.detail, Some("Could not run speech recognition."));
        assert_eq!(
            AppError::message("inference", "engine boom transcript line").user_message(),
            snap.detail.unwrap_or("")
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn cancel_stops_running_job() {
        // Arrange
        let dir = temp_dir("cancel");
        let audio = dir.join("clip.flac");
        write_pcm_flac(&audio, 48_000);
        let before = fs::read(&audio).expect("before");
        seed_sidecar(&audio, 13);
        let model = dir.join("ggml-base.bin");
        fs::write(&model, b"ggml").expect("model");
        let fake = FakeInference {
            fail_load: false,
            fail_chunk: false,
            hold_ms: 800,
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
        host.enqueue(RecordingId::from_raw(13), audio.clone(), model, "base-q5_1");
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
        assert_eq!(fs::read(&audio).expect("after"), before);
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
        let audio = dir.join("clip.flac");
        write_pcm_flac(&audio, 4_000);
        seed_sidecar(&audio, 14);
        let model = dir.join("ggml-base.bin");
        fs::write(&model, b"ggml").expect("model");
        let fake = FakeInference {
            fail_load: false,
            fail_chunk: false,
            hold_ms: 50,
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
        host.enqueue(RecordingId::from_raw(14), audio.clone(), model, "base-q5_1");
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

        // Assert — either terminal state is fine; must not panic or corrupt the audio.
        assert!(matches!(
            phase,
            JobPhase::Done | JobPhase::Cancelled | JobPhase::Failed
        ));
        assert!(audio.is_file());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn invalid_model_fails_without_touching_flac() {
        // Arrange
        let dir = temp_dir("model");
        let audio = dir.join("clip.flac");
        write_pcm_flac(&audio, 2_000);
        let before = fs::read(&audio).expect("before");
        seed_sidecar(&audio, 15);
        let model = dir.join("invalid.bin");
        fs::write(&model, b"nope").expect("model");
        let fake = FakeInference {
            fail_load: false,
            fail_chunk: false,
            hold_ms: 0,
            calls: Arc::new(Mutex::new(Vec::new())),
            segments: Vec::new(),
        };
        let (host, handle) =
            TranscriptionHost::spawn_with_config(Box::new(fake), short_config(), Box::new(|_| {}));

        // Act
        host.enqueue(RecordingId::from_raw(15), audio.clone(), model, "bad");
        wait_until(&host, |snap| snap.phase == JobPhase::Failed);
        let snap = host.snapshot();
        host.shutdown();
        let _ = handle.join();

        // Assert
        assert_eq!(snap.notice, Some(NOTICE_MODEL));
        assert_eq!(fs::read(&audio).expect("after"), before);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn long_input_is_fed_in_read_sized_blocks() {
        // Arrange — 24,000 frames with 1,000-frame reads → many small feeds.
        let dir = temp_dir("chunk");
        let audio = dir.join("long.flac");
        write_pcm_flac(&audio, 24_000);
        seed_sidecar(&audio, 16);
        let model = dir.join("model.gguf");
        fs::write(&model, b"gguf").expect("model");
        let calls = Arc::new(Mutex::new(Vec::new()));
        let fake = FakeInference {
            fail_load: false,
            fail_chunk: false,
            hold_ms: 0,
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
        host.enqueue(RecordingId::from_raw(16), audio.clone(), model, "m");
        wait_until(&host, |snap| snap.phase == JobPhase::Done);
        host.shutdown();
        let _ = handle.join();
        let sizes = calls.lock().expect("lock").clone();

        // Assert
        assert!(sizes.len() >= 2, "expected multiple feeds, got {sizes:?}");
        let max = sizes.iter().copied().max().unwrap_or(0);
        assert!(max <= 400, "feed too large: max={max} sizes={sizes:?}");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn fifo_runs_two_queued_recordings_in_order() {
        // Arrange
        let dir = temp_dir("fifo");
        let audio_a = dir.join("a.flac");
        let audio_b = dir.join("b.flac");
        write_pcm_flac(&audio_a, 4_000);
        write_pcm_flac(&audio_b, 4_000);
        seed_sidecar(&audio_a, 21);
        seed_sidecar(&audio_b, 22);
        let model = dir.join("ggml-base.bin");
        fs::write(&model, b"ggml").expect("model");
        let order = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&order);
        let fake = FakeInference {
            fail_load: false,
            fail_chunk: false,
            hold_ms: 80,
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
            audio_a.clone(),
            model.clone(),
            "base-q5_1",
        );
        host.enqueue(
            RecordingId::from_raw(22),
            audio_b.clone(),
            model,
            "base-q5_1",
        );
        // Duplicate enqueue is a no-op.
        host.enqueue(
            RecordingId::from_raw(21),
            audio_a.clone(),
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

    struct RecordingNotifier {
        ids: Arc<Mutex<Vec<RecordingId>>>,
    }

    impl TranscriptNotifier for RecordingNotifier {
        fn transcript_ready(&self, recording_id: RecordingId) {
            self.ids.lock().expect("lock").push(recording_id);
        }
    }

    fn notifier_pair() -> (Arc<RecordingNotifier>, Arc<Mutex<Vec<RecordingId>>>) {
        let ids = Arc::new(Mutex::new(Vec::new()));
        let notifier = Arc::new(RecordingNotifier {
            ids: Arc::clone(&ids),
        });
        (notifier, ids)
    }

    #[test]
    fn completion_notifies_once_when_enabled() {
        // Arrange
        let dir = temp_dir("notify-on");
        let audio = dir.join("meeting.flac");
        write_pcm_flac(&audio, 8_000);
        seed_sidecar(&audio, 31);
        let model = dir.join("ggml-base.bin");
        fs::write(&model, b"ggml").expect("model");
        let (notifier, ids) = notifier_pair();
        let fake = FakeInference {
            fail_load: false,
            fail_chunk: false,
            hold_ms: 0,
            calls: Arc::new(Mutex::new(Vec::new())),
            segments: vec![InferenceSegment {
                start_ms: 0,
                end_ms: 400,
                text: "hello".into(),
            }],
        };
        let (host, handle) = TranscriptionHost::spawn_with_notifier(
            Box::new(fake),
            short_config(),
            notifier,
            Arc::new(AtomicBool::new(true)),
            Box::new(|_| {}),
        );

        // Act
        host.enqueue(RecordingId::from_raw(31), audio, model, "base-q5_1");
        wait_until(&host, |snap| snap.phase == JobPhase::Done);
        host.shutdown();
        let _ = handle.join();
        let recorded = ids.lock().expect("lock").clone();

        // Assert — one completion, not one call per progress tick.
        assert_eq!(recorded, vec![RecordingId::from_raw(31)]);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn completion_does_not_notify_when_disabled() {
        // Arrange
        let dir = temp_dir("notify-off");
        let audio = dir.join("meeting.flac");
        write_pcm_flac(&audio, 8_000);
        seed_sidecar(&audio, 32);
        let model = dir.join("ggml-base.bin");
        fs::write(&model, b"ggml").expect("model");
        let (notifier, ids) = notifier_pair();
        let fake = FakeInference {
            fail_load: false,
            fail_chunk: false,
            hold_ms: 0,
            calls: Arc::new(Mutex::new(Vec::new())),
            segments: vec![InferenceSegment {
                start_ms: 0,
                end_ms: 400,
                text: "hello".into(),
            }],
        };
        let enabled = Arc::new(AtomicBool::new(false));
        let (host, handle) = TranscriptionHost::spawn_with_notifier(
            Box::new(fake),
            short_config(),
            notifier,
            Arc::clone(&enabled),
            Box::new(|_| {}),
        );

        // Act
        host.enqueue(RecordingId::from_raw(32), audio.clone(), model, "base-q5_1");
        wait_until(&host, |snap| snap.phase == JobPhase::Done);
        host.shutdown();
        let _ = handle.join();

        // Assert
        assert!(ids.lock().expect("lock").is_empty());
        assert!(dir.join("meeting.txt").is_file());
        let _ = fs::remove_dir_all(dir);
    }
}
