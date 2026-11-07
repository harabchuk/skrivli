//! One command path for the toolbar, menus, and the tray.

use std::sync::mpsc::Receiver;

use crate::domain::ids::{JobId, RecordingId};
use crate::infrastructure::audio::devices::DeviceCatalog;
use crate::infrastructure::storage::settings::StoredEndpoint;
use crate::services::library::LibrarySnapshot;
use crate::services::playback::PlayerSnapshot;
use crate::services::ports::HostCommand;
use crate::services::recording::RecordSnapshot;
use crate::services::transcription::TranscriptionSnapshot;

use super::state::{AppTab, Presentation};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AppCommand {
    StartRecording,
    StopRecording,
    SelectMicrophone(StoredEndpoint),
    SelectSystem(StoredEndpoint),
    /// Dispatched from Settings (folder Browse) when that dialog is wired.
    #[allow(dead_code)]
    BrowseRecordings,
    Transcribe {
        recording_id: RecordingId,
    },
    CancelTranscription {
        job_id: JobId,
    },
    RefreshLibrary,
    /// Download the transcription model selected in the prompt. User-initiated only.
    DownloadModel,
    CancelModelDownload,
    RetryModelDownload,
    /// Make the model highlighted in the dialog the one in use.
    UseModel,
    Show,
    Hide,
    ToggleVisibility,
    OpenSettings,
    OpenRecordingsFolder,
    About,
    Exit,
    /// Windows logoff/shutdown: stop and finalize without a confirm dialog.
    SessionEnding,
    SelectTab(AppTab),
    /// Show the window and select this recording when the library still has it.
    ShowRecording(RecordingId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Effect {
    None,
    Show,
    Hide {
        explain: bool,
    },
    Quit,
    /// Ask to stop/save before quitting while a recording is active.
    ConfirmExit,
    OpenFolder,
    OpenSettings,
    OpenAbout,
    #[allow(dead_code)] // Reserved for future unavailable-feature notices.
    Unavailable,
    StartRecording,
    StopRecording,
    BrowseRecordings,
    RefreshLibrary,
    Transcribe {
        recording_id: RecordingId,
    },
    CancelTranscription {
        job_id: JobId,
    },
    DownloadModel,
    CancelModelDownload,
    RetryModelDownload,
    UseModel,
    /// Show the window. `recording_id` is set only when that row is still in the library.
    ShowSelect {
        recording_id: Option<RecordingId>,
    },
}

impl From<HostCommand> for AppCommand {
    fn from(command: HostCommand) -> Self {
        match command {
            HostCommand::Show => AppCommand::Show,
            HostCommand::ToggleVisibility => AppCommand::ToggleVisibility,
            HostCommand::StartRecording => AppCommand::StartRecording,
            HostCommand::StopRecording => AppCommand::StopRecording,
            HostCommand::OpenRecordingsFolder => AppCommand::OpenRecordingsFolder,
            HostCommand::OpenSettings => AppCommand::OpenSettings,
            HostCommand::Exit => AppCommand::Exit,
            HostCommand::SessionEnding => AppCommand::SessionEnding,
            HostCommand::ShowRecording(id) => AppCommand::ShowRecording(id),
        }
    }
}

/// Shell commands update presentation. Recording commands become effects the
/// window performs off the render path.
pub(crate) fn apply_command(state: &mut Presentation, command: AppCommand) -> Effect {
    match command {
        AppCommand::StartRecording => {
            if state.inputs_locked() {
                Effect::None
            } else {
                Effect::StartRecording
            }
        }
        AppCommand::StopRecording => Effect::StopRecording,
        AppCommand::BrowseRecordings => {
            if state.inputs_locked() {
                Effect::None
            } else {
                Effect::BrowseRecordings
            }
        }
        AppCommand::SelectMicrophone(_) | AppCommand::SelectSystem(_) => Effect::None,
        AppCommand::Transcribe { recording_id } => Effect::Transcribe { recording_id },
        AppCommand::CancelTranscription { job_id } => Effect::CancelTranscription { job_id },
        AppCommand::RefreshLibrary => Effect::RefreshLibrary,
        AppCommand::DownloadModel => Effect::DownloadModel,
        AppCommand::CancelModelDownload => Effect::CancelModelDownload,
        AppCommand::RetryModelDownload => Effect::RetryModelDownload,
        AppCommand::UseModel => Effect::UseModel,
        AppCommand::Show => {
            state.set_visible(true);
            state.set_pending_show(false);
            Effect::Show
        }
        AppCommand::Hide => hide(state),
        AppCommand::ToggleVisibility => {
            if state.visible() {
                hide(state)
            } else {
                state.set_visible(true);
                Effect::Show
            }
        }
        AppCommand::OpenSettings => Effect::OpenSettings,
        AppCommand::OpenRecordingsFolder => Effect::OpenFolder,
        AppCommand::About => Effect::OpenAbout,
        AppCommand::Exit => {
            if state.inputs_locked() {
                Effect::ConfirmExit
            } else {
                Effect::Quit
            }
        }
        AppCommand::SessionEnding => Effect::StopRecording,
        AppCommand::SelectTab(tab) => {
            state.set_tab(tab);
            Effect::None
        }
        AppCommand::ShowRecording(id) => {
            state.set_visible(true);
            state.set_pending_show(false);
            let recording_id = if state.knows_recording(id) {
                state.set_selected_recording(Some(id));
                state.set_tab(AppTab::Recordings);
                Some(id)
            } else {
                None
            };
            Effect::ShowSelect { recording_id }
        }
    }
}

fn hide(state: &mut Presentation) -> Effect {
    state.set_visible(false);
    let explain = !state.tray_explanation_seen();
    if explain {
        state.mark_tray_explanation_seen();
        state.set_notice(Some("Skrivli is still running in the notification area."));
    }
    Effect::Hide { explain }
}

pub(crate) enum BridgeMessage {
    Command(AppCommand),
    ShellPulse,
    Recording(RecordSnapshot),
    Devices(DeviceCatalog),
    Library(LibrarySnapshot),
    Player(PlayerSnapshot),
    Transcription(TranscriptionSnapshot),
    Model(crate::services::models::ModelSnapshot),
}

pub(crate) struct BridgeBatch {
    pub(crate) effects: Vec<Effect>,
    pub(crate) recording: Option<RecordSnapshot>,
    pub(crate) devices: Option<DeviceCatalog>,
    pub(crate) library: Option<LibrarySnapshot>,
    pub(crate) player: Option<PlayerSnapshot>,
    pub(crate) transcription: Option<TranscriptionSnapshot>,
    pub(crate) model: Option<crate::services::models::ModelSnapshot>,
}

/// Coalesce pulses and keep only the latest recording, device, library, player,
/// and transcription snapshots.
pub(crate) fn drain_bridge(rx: &Receiver<BridgeMessage>, state: &mut Presentation) -> BridgeBatch {
    let mut pulses = 0_u64;
    let mut effects = Vec::new();
    let mut recording = None;
    let mut devices = None;
    let mut library = None;
    let mut player = None;
    let mut transcription = None;
    let mut model = None;
    while let Ok(message) = rx.try_recv() {
        match message {
            BridgeMessage::ShellPulse => pulses = pulses.saturating_add(1),
            BridgeMessage::Command(command) => effects.push(apply_command(state, command)),
            BridgeMessage::Recording(snapshot) => recording = Some(snapshot),
            BridgeMessage::Devices(catalog) => devices = Some(catalog),
            BridgeMessage::Library(snapshot) => {
                let ids = snapshot
                    .catalog
                    .entries
                    .iter()
                    .map(|entry| entry.info.id())
                    .collect();
                state.set_known_recordings(ids);
                library = Some(snapshot);
            }
            BridgeMessage::Player(snapshot) => player = Some(snapshot),
            BridgeMessage::Transcription(snapshot) => transcription = Some(snapshot),
            BridgeMessage::Model(snapshot) => model = Some(snapshot),
        }
    }
    if pulses > 0 {
        state.add_shell_pulses(pulses);
    }
    BridgeBatch {
        effects,
        recording,
        devices,
        library,
        player,
        transcription,
        model,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::thread;

    use super::{apply_command, drain_bridge, AppCommand, BridgeMessage, Effect};
    use crate::domain::ids::{JobId, RecordingId};
    use crate::domain::recording::RecordPhase;
    use crate::services::ports::HostCommand;
    use crate::views::state::{AppTab, Presentation};

    fn presentation() -> Presentation {
        Presentation::new(true, false, true)
    }

    #[test]
    fn recording_commands_report_availability() {
        // Arrange
        let mut state = presentation();

        // Act
        let start = apply_command(&mut state, AppCommand::StartRecording);
        let stop = apply_command(&mut state, AppCommand::StopRecording);
        state.set_record_phase(RecordPhase::Recording);
        let ignored = apply_command(&mut state, AppCommand::StartRecording);
        let browse = apply_command(&mut state, AppCommand::BrowseRecordings);
        let transcribe = apply_command(
            &mut state,
            AppCommand::Transcribe {
                recording_id: RecordingId::from_raw(7),
            },
        );
        let cancel = apply_command(
            &mut state,
            AppCommand::CancelTranscription {
                job_id: JobId::from_raw(3),
            },
        );

        // Assert
        assert_eq!(start, Effect::StartRecording);
        assert_eq!(stop, Effect::StopRecording);
        assert_eq!(ignored, Effect::None);
        assert_eq!(browse, Effect::None);
        assert_eq!(
            transcribe,
            Effect::Transcribe {
                recording_id: RecordingId::from_raw(7)
            }
        );
        assert_eq!(
            cancel,
            Effect::CancelTranscription {
                job_id: JobId::from_raw(3)
            }
        );
        let refresh = apply_command(&mut state, AppCommand::RefreshLibrary);
        assert_eq!(refresh, Effect::RefreshLibrary);
        assert_eq!(RecordingId::from_raw(7).raw(), 7);
        assert_eq!(JobId::from_raw(3).raw(), 3);
    }

    #[test]
    fn shell_commands_select_tabs_and_visibility() {
        // Arrange
        let mut state = presentation();

        // Act
        let tab = apply_command(&mut state, AppCommand::SelectTab(AppTab::Recordings));
        let hide = apply_command(&mut state, AppCommand::Hide);
        let show = apply_command(&mut state, AppCommand::Show);
        let settings = apply_command(&mut state, AppCommand::OpenSettings);
        let folder = apply_command(&mut state, AppCommand::OpenRecordingsFolder);
        let about = apply_command(&mut state, AppCommand::About);
        let exit = apply_command(&mut state, AppCommand::Exit);
        state.set_record_phase(RecordPhase::Recording);
        let exit_while_recording = apply_command(&mut state, AppCommand::Exit);
        let session_ending = apply_command(&mut state, AppCommand::SessionEnding);

        // Assert
        assert_eq!(tab, Effect::None);
        assert_eq!(state.tab(), AppTab::Recordings);
        assert_eq!(hide, Effect::Hide { explain: true });
        assert!(state.tray_explanation_seen());
        assert_eq!(show, Effect::Show);
        assert!(state.visible());
        assert_eq!(settings, Effect::OpenSettings);
        assert_eq!(folder, Effect::OpenFolder);
        assert_eq!(about, Effect::OpenAbout);
        assert_eq!(exit, Effect::Quit);
        assert_eq!(exit_while_recording, Effect::ConfirmExit);
        assert_eq!(session_ending, Effect::StopRecording);
    }

    #[test]
    fn second_hide_does_not_explain_again() {
        // Arrange
        let mut state = presentation();
        apply_command(&mut state, AppCommand::Hide);
        state.set_visible(true);

        // Act
        let again = apply_command(&mut state, AppCommand::Hide);

        // Assert
        assert_eq!(again, Effect::Hide { explain: false });
    }

    #[test]
    fn host_commands_use_the_same_path() {
        // Arrange
        let mapped = [
            (HostCommand::Show, AppCommand::Show),
            (HostCommand::ToggleVisibility, AppCommand::ToggleVisibility),
            (HostCommand::StartRecording, AppCommand::StartRecording),
            (HostCommand::StopRecording, AppCommand::StopRecording),
            (
                HostCommand::OpenRecordingsFolder,
                AppCommand::OpenRecordingsFolder,
            ),
            (HostCommand::OpenSettings, AppCommand::OpenSettings),
            (HostCommand::Exit, AppCommand::Exit),
            (HostCommand::SessionEnding, AppCommand::SessionEnding),
            (
                HostCommand::ShowRecording(RecordingId::from_raw(4)),
                AppCommand::ShowRecording(RecordingId::from_raw(4)),
            ),
        ];

        // Act
        let actual: Vec<_> = mapped
            .iter()
            .map(|(host, _)| AppCommand::from(*host))
            .collect();

        // Assert
        let expected: Vec<_> = mapped.into_iter().map(|(_, command)| command).collect();
        assert_eq!(actual, expected);
    }

    #[test]
    fn cross_thread_pulses_coalesce_into_one_status_update() {
        // Arrange
        let (tx, rx) = mpsc::channel();
        let producer = thread::spawn(move || {
            for _ in 0..5 {
                tx.send(BridgeMessage::ShellPulse).expect("send");
            }
        });
        producer.join().expect("producer");
        let mut state = presentation();

        // Act
        let effects = drain_bridge(&rx, &mut state);
        let again = drain_bridge(&rx, &mut state);

        // Assert
        assert!(effects.effects.is_empty());
        assert!(again.effects.is_empty());
        assert_eq!(state.shell_pulses(), 5);
        assert_eq!(state.status_line(), "Shell ready (5)");
    }

    #[test]
    fn model_commands_return_host_effects() {
        // Arrange
        let mut state = presentation();

        // Act
        let download = apply_command(&mut state, AppCommand::DownloadModel);
        let cancel = apply_command(&mut state, AppCommand::CancelModelDownload);
        let retry = apply_command(&mut state, AppCommand::RetryModelDownload);

        // Assert
        assert_eq!(download, Effect::DownloadModel);
        assert_eq!(cancel, Effect::CancelModelDownload);
        assert_eq!(retry, Effect::RetryModelDownload);
    }

    #[test]
    fn use_model_command_returns_its_effect() {
        // Arrange
        let mut state = presentation();

        // Act
        let effect = apply_command(&mut state, AppCommand::UseModel);

        // Assert
        assert_eq!(effect, Effect::UseModel);
    }

    #[test]
    fn model_snapshots_coalesce_to_the_latest() {
        use crate::services::models::{ModelPhase, ModelSnapshot};

        // Arrange
        let (tx, rx) = mpsc::channel();
        tx.send(BridgeMessage::Model(ModelSnapshot {
            phase: ModelPhase::Downloading,
            progress_percent: 10,
            failure: None,
            notice: None,
            total_bytes: 100,
            models: Vec::new(),
        }))
        .expect("send");
        tx.send(BridgeMessage::Model(ModelSnapshot {
            phase: ModelPhase::Downloading,
            progress_percent: 90,
            failure: None,
            notice: None,
            total_bytes: 100,
            models: Vec::new(),
        }))
        .expect("send");
        let mut state = presentation();

        // Act
        let batch = drain_bridge(&rx, &mut state);

        // Assert
        assert_eq!(batch.model.map(|snap| snap.progress_percent), Some(90));
    }

    #[test]
    fn show_recording_selects_id_when_library_snapshot_has_the_row() {
        use std::path::PathBuf;

        use crate::domain::recording::{
            RecordingContainer, RecordingInfo, SidecarState, TranscriptStatus,
        };
        use crate::infrastructure::storage::library::{LibraryCatalog, LibraryEntry};
        use crate::services::library::LibrarySnapshot;

        // Arrange
        let id = RecordingId::from_raw(9);
        let other = RecordingId::from_raw(3);
        let snapshot = LibrarySnapshot {
            catalog: LibraryCatalog {
                entries: vec![LibraryEntry {
                    info: RecordingInfo::new(
                        id,
                        "meeting",
                        RecordingContainer::Wav,
                        None,
                        None,
                        1,
                        None,
                        TranscriptStatus::Ready,
                        SidecarState::Present,
                        false,
                        Vec::new(),
                    ),
                    audio_path: PathBuf::from("meeting.wav"),
                    sidecar_path: None,
                    transcript_path: None,
                }],
                error: None,
            },
            selected: Some(other),
            notice: None,
        };
        let (tx, rx) = mpsc::channel();
        tx.send(BridgeMessage::Library(snapshot)).expect("library");
        tx.send(BridgeMessage::Command(AppCommand::ShowRecording(id)))
            .expect("command");
        let mut state = presentation();
        state.set_selected_recording(Some(other));
        state.set_tab(AppTab::Record);

        // Act
        let batch = drain_bridge(&rx, &mut state);

        // Assert
        assert_eq!(state.selected_recording(), Some(id));
        assert_eq!(state.tab(), AppTab::Recordings);
        assert!(state.visible());
        assert_eq!(
            batch.effects,
            vec![Effect::ShowSelect {
                recording_id: Some(id)
            }]
        );
    }

    #[test]
    fn show_recording_leaves_selection_when_the_row_is_gone() {
        use crate::infrastructure::storage::library::LibraryCatalog;
        use crate::services::library::LibrarySnapshot;

        // Arrange
        let missing = RecordingId::from_raw(9);
        let kept = RecordingId::from_raw(3);
        let (tx, rx) = mpsc::channel();
        tx.send(BridgeMessage::Library(LibrarySnapshot {
            catalog: LibraryCatalog::empty(),
            selected: Some(kept),
            notice: None,
        }))
        .expect("library");
        tx.send(BridgeMessage::Command(AppCommand::ShowRecording(missing)))
            .expect("command");
        let mut state = presentation();
        state.set_selected_recording(Some(kept));
        state.set_tab(AppTab::Record);

        // Act
        let batch = drain_bridge(&rx, &mut state);

        // Assert
        assert_eq!(state.selected_recording(), Some(kept));
        assert_eq!(state.tab(), AppTab::Record);
        assert!(state.visible());
        assert_eq!(
            batch.effects,
            vec![Effect::ShowSelect { recording_id: None }]
        );
    }
}
