//! Presentation snapshots owned by the GPUI shell.

use crate::domain::ids::{JobId, RecordingId};
use crate::domain::recording::{settled_transcript_status, TranscriptStatus};
use crate::services::models::{ModelFile, ModelPhase, ModelSnapshot};
use crate::services::transcription::{
    JobPhase, TranscriptionSnapshot, NOTICE_CANCELLED, NOTICE_DONE, NOTICE_FAILED, NOTICE_MODEL,
};

pub(crate) use crate::domain::recording::RecordPhase;

/// Shown when a re-run fails and the earlier transcript is still on disk.
pub(crate) const NOTICE_FAILED_KEPT: &str =
    "Transcription failed. Your previous transcript is unchanged.";

/// True for notices that report a transcription result. They belong in the status bar and on
/// the Recordings tab, never in the Record tab banner.
pub(crate) fn is_transcription_notice(notice: &str) -> bool {
    [
        NOTICE_DONE,
        NOTICE_FAILED,
        NOTICE_CANCELLED,
        NOTICE_MODEL,
        NOTICE_FAILED_KEPT,
    ]
    .contains(&notice)
}

/// The status-bar notice for a job update. A plain failure becomes the "previous transcript
/// is unchanged" notice when a transcript file already exists.
fn settled_notice(
    phase: JobPhase,
    notice: &'static str,
    has_transcript_file: bool,
) -> &'static str {
    if phase == JobPhase::Failed && has_transcript_file && notice == NOTICE_FAILED {
        NOTICE_FAILED_KEPT
    } else {
        notice
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AppTab {
    Record,
    Recordings,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Presentation {
    tab: AppTab,
    shell_pulses: u64,
    visible: bool,
    close_to_tray: bool,
    tray_explanation_seen: bool,
    notice: Option<&'static str>,
    pending_show: bool,
    record_phase: RecordPhase,
    selected_recording: Option<RecordingId>,
    known_recordings: Vec<RecordingId>,
    loaded_transcript: String,
    detail_text: String,
    transcript_status: Option<TranscriptStatus>,
    transcript_recording: Option<RecordingId>,
    active_job: Option<JobId>,
    model_phase: ModelPhase,
    model_progress: Option<u8>,
    model_failure: Option<&'static str>,
    model_files: Vec<ModelFile>,
    model_files_known: bool,
    download_target: Option<String>,
}

impl Presentation {
    pub(crate) fn new(close_to_tray: bool, tray_explanation_seen: bool, visible: bool) -> Self {
        Self {
            tab: AppTab::Record,
            shell_pulses: 0,
            visible,
            close_to_tray,
            tray_explanation_seen,
            notice: None,
            pending_show: false,
            record_phase: RecordPhase::Idle,
            selected_recording: None,
            known_recordings: Vec::new(),
            loaded_transcript: String::new(),
            detail_text: String::new(),
            transcript_status: None,
            transcript_recording: None,
            active_job: None,
            model_phase: ModelPhase::Idle,
            model_progress: None,
            model_failure: None,
            model_files: Vec::new(),
            model_files_known: false,
            download_target: None,
        }
    }

    pub(crate) fn set_selected_recording(&mut self, id: Option<RecordingId>) {
        self.selected_recording = id;
    }

    pub(crate) fn set_known_recordings(&mut self, ids: Vec<RecordingId>) {
        self.known_recordings = ids;
    }

    pub(crate) fn knows_recording(&self, id: RecordingId) -> bool {
        self.known_recordings.contains(&id)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn selected_recording(&self) -> Option<RecordingId> {
        self.selected_recording
    }

    pub(crate) fn set_loaded_transcript(&mut self, text: impl Into<String>) {
        self.loaded_transcript = text.into();
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn loaded_transcript(&self) -> &str {
        &self.loaded_transcript
    }

    pub(crate) fn detail_text(&self) -> &str {
        &self.detail_text
    }

    pub(crate) fn transcript_status(&self) -> Option<TranscriptStatus> {
        self.transcript_status
    }

    pub(crate) fn transcript_recording(&self) -> Option<RecordingId> {
        self.transcript_recording
    }

    pub(crate) fn status_for(&self, id: RecordingId) -> Option<TranscriptStatus> {
        if self.transcript_recording == Some(id) {
            self.transcript_status
        } else {
            None
        }
    }

    pub(crate) fn active_job(&self) -> Option<(RecordingId, JobId)> {
        match (self.transcript_recording, self.active_job) {
            (Some(recording), Some(job)) => Some((recording, job)),
            _ => None,
        }
    }

    pub(crate) fn model_progress(&self) -> Option<u8> {
        self.model_progress
    }

    pub(crate) fn model_failure(&self) -> Option<&'static str> {
        self.model_failure
    }

    pub(crate) fn model_phase(&self) -> ModelPhase {
        self.model_phase
    }

    pub(crate) fn model_files(&self) -> &[ModelFile] {
        &self.model_files
    }

    /// False until the worker has reported the installed list once.
    #[cfg_attr(not(test), allow(dead_code))] // `model_status` reads the field directly.
    pub(crate) fn model_files_known(&self) -> bool {
        self.model_files_known
    }

    pub(crate) fn download_target(&self) -> Option<&str> {
        self.download_target.as_deref()
    }

    /// Marks the download busy right away, so a double click cannot queue a second one.
    pub(crate) fn begin_download(&mut self, id: impl Into<String>) {
        self.download_target = Some(id.into());
        self.model_phase = ModelPhase::Downloading;
        self.model_failure = None;
        self.model_progress = Some(0);
    }

    pub(crate) fn clear_download_target(&mut self) {
        self.download_target = None;
    }

    /// The finished model, once. A later Idle snapshot from a refresh finds nothing here.
    pub(crate) fn take_installed_target(&mut self) -> Option<String> {
        if self.model_phase != ModelPhase::Ready {
            return None;
        }
        let target = self.download_target.take()?;
        self.model_files
            .iter()
            .any(|file| file.variant_id.as_deref() == Some(target.as_str()))
            .then_some(target)
    }

    /// Reduce a transcription snapshot into the status strip and the selected row.
    /// A progress tick does not replace the loaded transcript, the selection, or the tab.
    ///
    /// `has_transcript_file` says whether a transcript already exists beside the recording,
    /// so a failed or cancelled re-run settles back to Ready instead of Failed.
    pub(crate) fn apply_transcription(
        &mut self,
        snapshot: TranscriptionSnapshot,
        has_transcript_file: bool,
    ) {
        let Some(recording_id) = snapshot.recording_id else {
            return;
        };
        let status = settled_transcript_status(
            snapshot.transcript_status(),
            snapshot.phase == JobPhase::Cancelled,
            has_transcript_file,
        );
        self.transcript_status = Some(status);
        self.transcript_recording = Some(recording_id);
        self.active_job = match snapshot.phase {
            JobPhase::Queued | JobPhase::Preparing | JobPhase::Transcribing => snapshot.job_id,
            JobPhase::Done | JobPhase::Failed | JobPhase::Cancelled => None,
        };
        self.detail_text = match status {
            TranscriptStatus::Failed => snapshot
                .detail
                .unwrap_or("Transcription failed.")
                .to_string(),
            TranscriptStatus::Ready => TranscriptStatus::Ready.display(),
            other => other.display(),
        };
        if let Some(notice) = snapshot.notice {
            self.notice = Some(settled_notice(snapshot.phase, notice, has_transcript_file));
        }
    }

    pub(crate) fn apply_model(&mut self, snapshot: ModelSnapshot) {
        self.model_files_known = true;
        self.model_phase = snapshot.phase;
        self.model_failure = snapshot.failure;
        if snapshot.phase == ModelPhase::Downloading || snapshot.phase == ModelPhase::Ready {
            self.model_progress = Some(snapshot.progress_percent);
        }
        if !snapshot.models.is_empty()
            || matches!(snapshot.phase, ModelPhase::Ready | ModelPhase::Idle)
        {
            self.model_files = snapshot.models;
        }
    }

    pub(crate) fn transcription_active(&self) -> bool {
        matches!(
            self.transcript_status,
            Some(TranscriptStatus::Queued | TranscriptStatus::Progress(_))
        )
    }

    pub(crate) fn tab(&self) -> AppTab {
        self.tab
    }

    pub(crate) fn set_tab(&mut self, tab: AppTab) {
        self.tab = tab;
    }

    pub(crate) fn visible(&self) -> bool {
        self.visible
    }

    pub(crate) fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
    }

    pub(crate) fn close_to_tray(&self) -> bool {
        self.close_to_tray
    }

    pub(crate) fn set_close_to_tray(&mut self, close_to_tray: bool) {
        self.close_to_tray = close_to_tray;
    }

    pub(crate) fn tray_explanation_seen(&self) -> bool {
        self.tray_explanation_seen
    }

    pub(crate) fn mark_tray_explanation_seen(&mut self) {
        self.tray_explanation_seen = true;
    }

    #[cfg_attr(not(test), allow(dead_code))] // No longer shown; the bridge test still counts it.
    pub(crate) fn shell_pulses(&self) -> u64 {
        self.shell_pulses
    }

    pub(crate) fn add_shell_pulses(&mut self, count: u64) {
        self.shell_pulses = self.shell_pulses.saturating_add(count);
    }

    pub(crate) fn set_notice(&mut self, notice: Option<&'static str>) {
        self.notice = notice;
    }

    pub(crate) fn notice(&self) -> Option<&'static str> {
        self.notice
    }

    pub(crate) fn record_phase(&self) -> RecordPhase {
        self.record_phase
    }

    pub(crate) fn set_record_phase(&mut self, phase: RecordPhase) {
        self.record_phase = phase;
    }

    pub(crate) fn inputs_locked(&self) -> bool {
        matches!(
            self.record_phase,
            RecordPhase::Starting | RecordPhase::Recording | RecordPhase::Stopping
        )
    }

    pub(crate) fn pending_show(&self) -> bool {
        self.pending_show
    }

    pub(crate) fn set_pending_show(&mut self, pending: bool) {
        self.pending_show = pending;
    }

    /// `elapsed` is passed in because the window owns the recording start time.
    /// Same text on every tab: activity, notice, then transcription progress.
    pub(crate) fn status_line(&self, elapsed: &str) -> String {
        let activity = match self.record_phase {
            RecordPhase::Starting => Some("Starting".to_string()),
            RecordPhase::Recording => Some(format!("Recording {elapsed}")),
            RecordPhase::Stopping => Some("Saving".to_string()),
            RecordPhase::Idle | RecordPhase::Error => None,
        };
        let transcription = self
            .transcript_status
            .filter(|_| self.transcription_active())
            .map(|status| match status {
                TranscriptStatus::Progress(percent) => format!("Transcribing {percent}%"),
                other => other.display().to_string(),
            });
        let parts: Vec<String> = activity
            .into_iter()
            .chain(self.notice.map(str::to_string))
            .chain(transcription)
            .collect();
        if parts.is_empty() {
            "Ready".to_string()
        } else {
            parts.join(" · ")
        }
    }

    pub(crate) fn model_status(&self, selected_id: &str) -> ModelStatus {
        if self.model_phase == ModelPhase::Downloading {
            if let Some(id) = &self.download_target {
                return ModelStatus::Downloading {
                    id: id.clone(),
                    percent: self.model_progress.unwrap_or(0),
                };
            }
        }
        if !self.model_files_known {
            return ModelStatus::Checking;
        }
        match model_install_state(selected_id, selected_id, &self.model_files) {
            ModelInstallState::InUse => ModelStatus::InUse {
                id: selected_id.to_string(),
            },
            ModelInstallState::Installed | ModelInstallState::NotInstalled => ModelStatus::Missing,
        }
    }
}

/// What the status bar and Settings report about the model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ModelStatus {
    Checking,
    InUse { id: String },
    Missing,
    Downloading { id: String, percent: u8 },
}

/// Whether one model is usable. Selected but not installed counts as `NotInstalled`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModelInstallState {
    InUse,
    Installed,
    NotInstalled,
}

pub(crate) fn model_install_state(
    id: &str,
    selected_id: &str,
    files: &[ModelFile],
) -> ModelInstallState {
    let installed = files
        .iter()
        .any(|file| file.variant_id.as_deref() == Some(id));
    match (installed, id == selected_id) {
        (true, true) => ModelInstallState::InUse,
        (true, false) => ModelInstallState::Installed,
        (false, _) => ModelInstallState::NotInstalled,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        model_install_state, ModelInstallState, ModelStatus, Presentation, NOTICE_FAILED,
        NOTICE_FAILED_KEPT,
    };
    use crate::domain::ids::{JobId, RecordingId};
    use crate::domain::recording::{RecordPhase, TranscriptStatus};
    use crate::services::models::{ModelFile, ModelPhase, ModelSnapshot};
    use crate::services::transcription::{JobPhase, TranscriptionSnapshot};
    use crate::views::state::AppTab;

    fn presentation() -> Presentation {
        Presentation::new(true, false, true)
    }

    fn snapshot(
        phase: JobPhase,
        percent: u8,
        detail: Option<&'static str>,
    ) -> TranscriptionSnapshot {
        TranscriptionSnapshot {
            job_id: Some(JobId::from_raw(9)),
            recording_id: Some(RecordingId::from_raw(4)),
            phase,
            progress_percent: percent,
            notice: None,
            detail,
        }
    }

    #[test]
    fn transcription_progress_keeps_loaded_text_and_selection() {
        // Arrange
        let mut state = presentation();
        let id = RecordingId::from_raw(4);
        let loaded = "こんにちは — café".to_string();
        state.set_tab(AppTab::Recordings);
        state.set_record_phase(RecordPhase::Recording);
        state.set_selected_recording(Some(id));
        state.set_loaded_transcript(loaded.clone());

        // Act
        state.apply_transcription(snapshot(JobPhase::Transcribing, 42, None), false);

        // Assert
        assert_eq!(state.loaded_transcript(), loaded);
        assert_eq!(state.selected_recording(), Some(id));
        assert_eq!(state.tab(), AppTab::Recordings);
        assert_eq!(
            state.transcript_status(),
            Some(TranscriptStatus::Progress(42))
        );
        assert!(state.status_line("00:00:05").starts_with("Recording "));
        assert!(state.status_line("00:00:05").contains("42%"));
    }

    #[test]
    fn transcription_completion_sets_ready_without_changing_tab() {
        // Arrange
        let mut state = presentation();
        let id = RecordingId::from_raw(4);
        state.set_tab(AppTab::Record);
        state.set_selected_recording(Some(id));
        state.set_loaded_transcript("stay");

        // Act
        state.apply_transcription(
            TranscriptionSnapshot {
                job_id: Some(JobId::from_raw(9)),
                recording_id: Some(id),
                phase: JobPhase::Done,
                progress_percent: 100,
                notice: Some("Transcript ready."),
                detail: Some("Ready"),
            },
            true,
        );

        // Assert
        assert_eq!(state.tab(), AppTab::Record);
        assert_eq!(state.selected_recording(), Some(id));
        assert_eq!(state.loaded_transcript(), "stay");
        assert_eq!(state.transcript_status(), Some(TranscriptStatus::Ready));
        assert_eq!(state.detail_text(), "Ready");
    }

    #[test]
    fn failed_rerun_settles_to_ready_with_the_unchanged_notice() {
        // Arrange
        let mut state = presentation();
        state.set_tab(AppTab::Recordings);
        let mut failed = snapshot(
            JobPhase::Failed,
            10,
            Some("Could not run speech recognition."),
        );
        failed.notice = Some(NOTICE_FAILED);

        // Act
        state.apply_transcription(failed, true);

        // Assert
        assert_eq!(state.transcript_status(), Some(TranscriptStatus::Ready));
        assert_eq!(state.active_job(), None);
        assert_eq!(state.status_line("00:00:05"), NOTICE_FAILED_KEPT);
    }

    #[test]
    fn cancelled_run_settles_by_whether_a_transcript_exists() {
        // Arrange
        let mut first_run = presentation();
        let mut rerun = presentation();

        // Act
        first_run.apply_transcription(snapshot(JobPhase::Cancelled, 10, None), false);
        rerun.apply_transcription(snapshot(JobPhase::Cancelled, 10, None), true);

        // Assert
        assert_eq!(first_run.transcript_status(), Some(TranscriptStatus::None));
        assert_eq!(rerun.transcript_status(), Some(TranscriptStatus::Ready));
    }

    #[test]
    fn transcription_failure_detail_is_user_message() {
        // Arrange
        let mut state = presentation();
        state.set_record_phase(RecordPhase::Idle);
        state.set_selected_recording(Some(RecordingId::from_raw(4)));

        // Act
        state.apply_transcription(
            snapshot(
                JobPhase::Failed,
                10,
                Some("Could not run speech recognition."),
            ),
            false,
        );

        // Assert
        assert_eq!(state.transcript_status(), Some(TranscriptStatus::Failed));
        assert_eq!(state.detail_text(), "Could not run speech recognition.");
        assert!(!state.detail_text().contains("engine"));
        assert!(!state.status_line("00:00:05").starts_with("Recording "));
    }

    #[test]
    fn model_progress_keeps_the_latest_percent() {
        // Arrange
        let mut state = presentation();
        state.set_tab(AppTab::Recordings);
        state.set_record_phase(RecordPhase::Recording);

        // Act
        state.apply_model(ModelSnapshot {
            phase: ModelPhase::Downloading,
            progress_percent: 40,
            failure: None,
            notice: None,
            total_bytes: 100,
            models: Vec::new(),
        });
        state.apply_model(ModelSnapshot {
            phase: ModelPhase::Downloading,
            progress_percent: 80,
            failure: None,
            notice: None,
            total_bytes: 100,
            models: Vec::new(),
        });

        // Assert
        assert_eq!(state.model_progress(), Some(80));
        assert!(state.status_line("00:00:05").starts_with("Recording "));
    }

    fn model_snapshot(phase: ModelPhase, installed: &[&str]) -> ModelSnapshot {
        ModelSnapshot {
            phase,
            progress_percent: 0,
            failure: None,
            notice: None,
            total_bytes: 0,
            models: installed
                .iter()
                .map(|id| ModelFile {
                    filename: format!("{id}.gguf"),
                    variant_id: Some((*id).to_string()),
                })
                .collect(),
        }
    }

    #[test]
    fn install_state_needs_the_file_and_the_selection() {
        // Arrange
        let files = model_snapshot(ModelPhase::Idle, &["a"]).models;
        let unmapped = [ModelFile {
            filename: "x.gguf".to_string(),
            variant_id: None,
        }];

        // Act
        let in_use = model_install_state("a", "a", &files);
        let installed = model_install_state("a", "b", &files);
        let missing = model_install_state("b", "b", &files);
        let no_variant = model_install_state("x", "x", &unmapped);

        // Assert
        assert_eq!(in_use, ModelInstallState::InUse);
        assert_eq!(installed, ModelInstallState::Installed);
        assert_eq!(missing, ModelInstallState::NotInstalled);
        assert_eq!(no_variant, ModelInstallState::NotInstalled);
    }

    #[test]
    fn model_list_is_unknown_until_the_first_snapshot() {
        // Arrange
        let mut state = presentation();

        // Act
        let before = state.model_files_known();
        state.apply_model(model_snapshot(ModelPhase::Idle, &[]));
        let after = state.model_files_known();

        // Assert
        assert!(!before);
        assert!(after);
    }

    #[test]
    fn begin_download_is_busy_before_the_first_snapshot() {
        // Arrange
        let mut state = presentation();

        // Act
        state.begin_download("a");

        // Assert
        assert_eq!(state.model_phase(), ModelPhase::Downloading);
        assert_eq!(state.download_target(), Some("a"));
    }

    #[test]
    fn ready_hands_over_the_target_once() {
        // Arrange
        let mut state = presentation();
        state.begin_download("a");
        state.apply_model(model_snapshot(ModelPhase::Ready, &["a"]));

        // Act
        let first = state.take_installed_target();
        let second = state.take_installed_target();

        // Assert
        assert_eq!(first.as_deref(), Some("a"));
        assert_eq!(second, None);
    }

    #[test]
    fn failure_keeps_the_target_and_hands_nothing_over() {
        // Arrange
        let mut state = presentation();
        state.begin_download("a");
        state.apply_model(model_snapshot(ModelPhase::Failed, &[]));

        // Act
        let taken = state.take_installed_target();

        // Assert
        assert_eq!(taken, None);
        assert_eq!(state.download_target(), Some("a"));
    }

    #[test]
    fn clearing_the_target_forgets_the_download() {
        // Arrange
        let mut state = presentation();
        state.begin_download("a");

        // Act
        state.clear_download_target();

        // Assert
        assert_eq!(state.download_target(), None);
    }

    #[test]
    fn status_line_is_the_same_on_both_tabs() {
        // Arrange
        let mut state = presentation();
        state.set_notice(Some("Saved."));
        state.set_record_phase(RecordPhase::Recording);
        state.set_tab(AppTab::Record);
        let on_record = state.status_line("00:00:07");

        // Act
        state.set_tab(AppTab::Recordings);
        let on_recordings = state.status_line("00:00:07");

        // Assert
        assert_eq!(on_record, "Recording 00:00:07 · Saved.");
        assert_eq!(on_record, on_recordings);
    }

    #[test]
    fn status_line_reads_ready_when_idle_on_the_record_tab() {
        // Arrange
        let mut state = presentation();
        state.set_tab(AppTab::Record);

        // Act
        let line = state.status_line("00:00:00");

        // Assert
        assert_eq!(line, "Ready");
    }

    #[test]
    fn status_line_on_record_tab_keeps_transcription_progress() {
        // Arrange
        let mut state = presentation();
        state.set_record_phase(RecordPhase::Recording);
        state.apply_transcription(snapshot(JobPhase::Transcribing, 42, None), false);

        // Act
        let line = state.status_line("00:00:07");

        // Assert
        assert_eq!(line, "Recording 00:00:07 · Transcribing 42%");
    }

    #[test]
    fn status_line_reads_naturally_for_every_phase() {
        // Arrange
        let mut state = presentation();
        state.set_tab(AppTab::Recordings);
        let idle = state.status_line("00:00:05");

        // Act
        state.set_record_phase(RecordPhase::Starting);
        let starting = state.status_line("00:00:05");
        state.set_record_phase(RecordPhase::Recording);
        let recording = state.status_line("00:12:41");
        state.set_record_phase(RecordPhase::Stopping);
        let saving = state.status_line("00:12:41");
        state.set_record_phase(RecordPhase::Error);
        let errored = state.status_line("00:12:41");

        // Assert
        assert_eq!(idle, "Ready");
        assert_eq!(starting, "Starting");
        assert_eq!(recording, "Recording 00:12:41");
        assert_eq!(saving, "Saving");
        assert_eq!(errored, "Ready");
    }

    #[test]
    fn status_line_shows_transcription_progress() {
        // Arrange
        let mut state = presentation();
        state.apply_transcription(snapshot(JobPhase::Transcribing, 42, None), false);

        // Act
        let line = state.status_line("00:00:05");

        // Assert
        assert_eq!(line, "Transcribing 42%");
    }

    #[test]
    fn status_line_puts_a_notice_with_the_activity() {
        // Arrange
        let mut state = presentation();
        state.set_tab(AppTab::Recordings);
        state.set_notice(Some("Saved."));
        let alone = state.status_line("00:00:05");
        state.set_record_phase(RecordPhase::Recording);

        // Act
        let with_activity = state.status_line("00:00:07");

        // Assert
        assert_eq!(alone, "Saved.");
        assert_eq!(with_activity, "Recording 00:00:07 · Saved.");
    }

    #[test]
    fn status_line_never_shows_internal_text() {
        // Arrange
        let mut state = presentation();
        state.add_shell_pulses(5);

        // Act
        let line = state.status_line("00:00:05");

        // Assert
        assert!(!line.contains("Shell"));
        assert!(!line.contains("Idle"));
    }

    #[test]
    fn model_status_is_checking_until_the_list_is_known() {
        // Arrange
        let mut state = presentation();

        // Act
        let before = state.model_status("a");
        state.apply_model(model_snapshot(ModelPhase::Idle, &[]));
        let after = state.model_status("a");

        // Assert
        assert_eq!(before, ModelStatus::Checking);
        assert_eq!(after, ModelStatus::Missing);
    }

    #[test]
    fn model_status_is_in_use_when_the_selected_model_is_installed() {
        // Arrange
        let mut state = presentation();
        state.apply_model(model_snapshot(ModelPhase::Idle, &["a", "b"]));

        // Act
        let status = state.model_status("a");

        // Assert
        assert_eq!(
            status,
            ModelStatus::InUse {
                id: "a".to_string()
            }
        );
    }

    #[test]
    fn model_status_is_missing_when_only_another_model_is_installed() {
        // Arrange
        let mut state = presentation();
        state.apply_model(model_snapshot(ModelPhase::Idle, &["b"]));

        // Act
        let status = state.model_status("a");

        // Assert
        assert_eq!(status, ModelStatus::Missing);
    }

    #[test]
    fn old_whisper_selection_shows_no_model_installed() {
        // Arrange: settings from before the engine change still name a Whisper model,
        // and no `.gguf` file is on disk (old `ggml-*.bin` files are not listed).
        let mut state = presentation();
        state.apply_model(model_snapshot(ModelPhase::Idle, &[]));

        // Act
        let status = state.model_status("base-q5_1");

        // Assert
        assert_eq!(status, ModelStatus::Missing);
    }

    #[test]
    fn a_download_in_progress_wins_over_everything() {
        // Arrange
        let mut state = presentation();

        // Act
        state.begin_download("a");
        let before_list = state.model_status("b");
        state.apply_model(ModelSnapshot {
            progress_percent: 40,
            ..model_snapshot(ModelPhase::Downloading, &["b"])
        });
        let during = state.model_status("b");

        // Assert
        let expected = |percent| ModelStatus::Downloading {
            id: "a".to_string(),
            percent,
        };
        assert_eq!(before_list, expected(0));
        assert_eq!(during, expected(40));
    }
}
