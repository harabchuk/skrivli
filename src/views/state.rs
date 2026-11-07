//! Presentation snapshots owned by the GPUI shell.

use crate::domain::ids::{JobId, RecordingId};
use crate::domain::recording::TranscriptStatus;
use crate::services::models::{ModelFile, ModelPhase, ModelSnapshot};
use crate::services::transcription::{JobPhase, TranscriptionSnapshot};

pub(crate) use crate::domain::recording::RecordPhase;

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
    #[cfg_attr(not(test), allow(dead_code))] // Read by the status bar in a later phase.
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
    pub(crate) fn apply_transcription(&mut self, snapshot: TranscriptionSnapshot) {
        let Some(recording_id) = snapshot.recording_id else {
            return;
        };
        let status = snapshot.transcript_status();
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
            self.notice = Some(notice);
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

    pub(crate) fn status_line(&self) -> String {
        let shell = if self.shell_pulses() == 0 {
            "Idle".to_string()
        } else {
            format!("Shell ready ({})", self.shell_pulses)
        };
        let body = match self.notice {
            Some(notice) => format!("{notice} {shell}"),
            None => shell,
        };
        let base = match self.record_phase {
            RecordPhase::Starting => format!("Starting {body}"),
            RecordPhase::Recording => format!("Recording {body}"),
            RecordPhase::Stopping => format!("Saving {body}"),
            RecordPhase::Idle | RecordPhase::Error => body,
        };
        if self.transcription_active() {
            if let Some(status) = self.transcript_status {
                return format!("{base} · {}", status.display());
            }
        }
        base
    }
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
    use super::{model_install_state, ModelInstallState, Presentation};
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
        state.apply_transcription(snapshot(JobPhase::Transcribing, 42, None));

        // Assert
        assert_eq!(state.loaded_transcript(), loaded);
        assert_eq!(state.selected_recording(), Some(id));
        assert_eq!(state.tab(), AppTab::Recordings);
        assert_eq!(
            state.transcript_status(),
            Some(TranscriptStatus::Progress(42))
        );
        assert!(state.status_line().starts_with("Recording "));
        assert!(state.status_line().contains("42%"));
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
        state.apply_transcription(TranscriptionSnapshot {
            job_id: Some(JobId::from_raw(9)),
            recording_id: Some(id),
            phase: JobPhase::Done,
            progress_percent: 100,
            notice: Some("Transcript ready."),
            detail: Some("Ready"),
        });

        // Assert
        assert_eq!(state.tab(), AppTab::Record);
        assert_eq!(state.selected_recording(), Some(id));
        assert_eq!(state.loaded_transcript(), "stay");
        assert_eq!(state.transcript_status(), Some(TranscriptStatus::Ready));
        assert_eq!(state.detail_text(), "Ready");
    }

    #[test]
    fn transcription_failure_detail_is_user_message() {
        // Arrange
        let mut state = presentation();
        state.set_record_phase(RecordPhase::Idle);
        state.set_selected_recording(Some(RecordingId::from_raw(4)));

        // Act
        state.apply_transcription(snapshot(
            JobPhase::Failed,
            10,
            Some("Could not run speech recognition."),
        ));

        // Assert
        assert_eq!(state.transcript_status(), Some(TranscriptStatus::Failed));
        assert_eq!(state.detail_text(), "Could not run speech recognition.");
        assert!(!state.detail_text().contains("engine"));
        assert!(!state.status_line().starts_with("Recording "));
    }

    #[test]
    fn model_progress_keeps_the_latest_percent() {
        // Arrange
        let mut state = presentation();
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
        assert!(state.status_line().starts_with("Recording "));
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
                    filename: format!("ggml-{id}.bin"),
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
            filename: "ggml-x.bin".to_string(),
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
}
