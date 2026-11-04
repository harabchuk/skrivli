//! One command path for the toolbar, menus, and the tray.

use std::sync::mpsc::Receiver;

use crate::domain::ids::{JobId, RecordingId};
use crate::infrastructure::audio::devices::DeviceCatalog;
use crate::infrastructure::files::player::PlayerSnapshot;
use crate::infrastructure::storage::settings::StoredEndpoint;
use crate::infrastructure::windows::tray::TrayCommand;
use crate::services::library::LibrarySnapshot;
use crate::services::recording::RecordSnapshot;

use super::state::{AppTab, Presentation};

pub(crate) const NOT_AVAILABLE: &str = "Not available yet.";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AppCommand {
    StartRecording,
    StopRecording,
    SelectMicrophone(StoredEndpoint),
    SelectSystem(StoredEndpoint),
    BrowseRecordings,
    // Queued until a recording service exists. The command path already accepts them.
    #[allow(dead_code)]
    Transcribe {
        recording_id: RecordingId,
    },
    #[allow(dead_code)]
    CancelTranscription {
        job_id: JobId,
    },
    RefreshLibrary,
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
    Unavailable,
    StartRecording,
    StopRecording,
    BrowseRecordings,
    RefreshLibrary,
}

pub(crate) fn command_from_tray(command: TrayCommand) -> AppCommand {
    match command {
        TrayCommand::Show => AppCommand::Show,
        TrayCommand::ToggleVisibility => AppCommand::ToggleVisibility,
        TrayCommand::StartRecording => AppCommand::StartRecording,
        TrayCommand::StopRecording => AppCommand::StopRecording,
        TrayCommand::OpenRecordingsFolder => AppCommand::OpenRecordingsFolder,
        TrayCommand::Settings => AppCommand::OpenSettings,
        TrayCommand::Exit => AppCommand::Exit,
        TrayCommand::SessionEnding => AppCommand::SessionEnding,
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
        AppCommand::Transcribe { .. } | AppCommand::CancelTranscription { .. } => {
            state.set_notice(Some(NOT_AVAILABLE));
            Effect::Unavailable
        }
        AppCommand::RefreshLibrary => Effect::RefreshLibrary,
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
}

pub(crate) struct BridgeBatch {
    pub(crate) effects: Vec<Effect>,
    pub(crate) recording: Option<RecordSnapshot>,
    pub(crate) devices: Option<DeviceCatalog>,
    pub(crate) library: Option<LibrarySnapshot>,
    pub(crate) player: Option<PlayerSnapshot>,
}

/// Coalesce pulses and keep only the latest recording, device, library, and player snapshots.
pub(crate) fn drain_bridge(rx: &Receiver<BridgeMessage>, state: &mut Presentation) -> BridgeBatch {
    let mut pulses = 0_u64;
    let mut effects = Vec::new();
    let mut recording = None;
    let mut devices = None;
    let mut library = None;
    let mut player = None;
    while let Ok(message) = rx.try_recv() {
        match message {
            BridgeMessage::ShellPulse => pulses = pulses.saturating_add(1),
            BridgeMessage::Command(command) => effects.push(apply_command(state, command)),
            BridgeMessage::Recording(snapshot) => recording = Some(snapshot),
            BridgeMessage::Devices(catalog) => devices = Some(catalog),
            BridgeMessage::Library(snapshot) => library = Some(snapshot),
            BridgeMessage::Player(snapshot) => player = Some(snapshot),
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
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::thread;

    use super::{
        apply_command, command_from_tray, drain_bridge, AppCommand, BridgeMessage, Effect,
        NOT_AVAILABLE,
    };
    use crate::app::state::{AppTab, Presentation};
    use crate::domain::ids::{JobId, RecordingId};
    use crate::infrastructure::windows::tray::TrayCommand;

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
        state.set_record_phase(crate::app::state::RecordPhase::Recording);
        let ignored = apply_command(&mut state, AppCommand::StartRecording);
        let browse = apply_command(&mut state, AppCommand::BrowseRecordings);
        let transcribe = apply_command(
            &mut state,
            AppCommand::Transcribe {
                recording_id: RecordingId::from_raw(7),
            },
        );

        // Assert
        assert_eq!(start, Effect::StartRecording);
        assert_eq!(stop, Effect::StopRecording);
        assert_eq!(ignored, Effect::None);
        assert_eq!(browse, Effect::None);
        assert_eq!(transcribe, Effect::Unavailable);
        let refresh = apply_command(&mut state, AppCommand::RefreshLibrary);
        assert_eq!(refresh, Effect::RefreshLibrary);
        assert_eq!(
            state.status_line(),
            format!("Recording {NOT_AVAILABLE} Idle")
        );
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
        state.set_record_phase(crate::app::state::RecordPhase::Recording);
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
    fn tray_commands_use_the_same_path() {
        // Arrange
        let mapped = [
            (TrayCommand::Show, AppCommand::Show),
            (TrayCommand::ToggleVisibility, AppCommand::ToggleVisibility),
            (TrayCommand::StartRecording, AppCommand::StartRecording),
            (TrayCommand::StopRecording, AppCommand::StopRecording),
            (
                TrayCommand::OpenRecordingsFolder,
                AppCommand::OpenRecordingsFolder,
            ),
            (TrayCommand::Settings, AppCommand::OpenSettings),
            (TrayCommand::Exit, AppCommand::Exit),
            (TrayCommand::SessionEnding, AppCommand::SessionEnding),
        ];

        // Act
        let actual: Vec<_> = mapped
            .iter()
            .map(|(tray, _)| command_from_tray(*tray))
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
}
