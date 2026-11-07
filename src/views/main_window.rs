//! Resizable shell: Record and Recordings tabs, settings gear, and the status strip.

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Editor, EditorState};
use gpui_kit::component::select::{SearchableVec, SelectEvent, SelectState};
use gpui_kit::component::slider::{SliderEvent, SliderState};
use gpui_kit::component::status_bar::StatusBar;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::table::{TableEvent, TableSelection, TableState};
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, IconName, WindowExt as _};
use gpui_kit::prelude::*;
use gpui_kit::{
    div, px, App, ClickEvent, Context, Entity, FocusHandle, IntoElement, ParentElement, Render,
    SharedString, Styled, Subscription, Task, Window,
};

use crate::domain::audio::format_elapsed;
use crate::domain::gain::{db_fs, format_gain};
use crate::domain::ids::{JobId, RecordingId};
use crate::domain::recording::{RecordPhase, TranscriptStatus};
use crate::domain::theme::ThemePreference;
use crate::domain::window_frame::LogicalRect;
use crate::infrastructure::ai::models::{path_for_variant, validate_model_file, ModelManifest};
use crate::infrastructure::audio::devices::DeviceCatalog;
use crate::infrastructure::audio::levels::{GainControl, LevelSnapshot};
use crate::infrastructure::storage::library::LibraryCatalog;
use crate::infrastructure::storage::settings::{Settings, StoredEndpoint, StoredWindowBounds};
use crate::infrastructure::storage::sidecar::transcript_path_for;
use crate::services::level_monitor::MonitorTargets;
use crate::services::library::{LibraryHost, LibrarySnapshot};
use crate::services::models::{ModelHost, ModelPhase, ModelSnapshot};
use crate::services::playback::{PlayerHost, PlayerPhase, PlayerSnapshot};
use crate::services::ports::DesktopShell;
use crate::services::recording::{
    build_start_request, RecordSnapshot, RecordingHost, StartRequestBuild, NOTICE_FALLBACK,
    NOTICE_SYSTEM_FALLBACK,
};
use crate::services::transcription::{JobPhase, TranscriptionHost, TranscriptionSnapshot};
use crate::ui::components::model_card::ModelCardData;
use crate::ui::theme::{apply_theme, theme_icon};
use crate::views::actions::{drain_bridge, AppCommand, BridgeMessage, Effect};
use crate::views::hooks::ShellHooks;
use crate::views::state::{model_install_state, AppTab, ModelStatus, Presentation};

use super::model_prompt::{
    after_model_ready, card_progress, error_display, model_copy, model_footer, model_status_item,
    open_model_alert, open_model_prompt, row_accessibility_label, row_meta, row_tag,
    AfterModelReady, ErrorDisplay, ModelPromptHandlers, ModelPromptView,
};
use super::recording::record_tab;
use super::recordings::{
    open_delete_dialog, open_rename_dialog, play_button_label, recordings_tab, rows_from_catalog,
    transcript_controls, RecordingsDelegate,
};
use super::settings::{
    open_about, open_exit_while_recording, open_settings, open_tray_explanation,
    settings_model_row, SettingsView,
};

const BOUNDS_DEBOUNCE: Duration = Duration::from_millis(400);

pub(crate) struct MainWindow {
    focus: FocusHandle,
    presentation: Presentation,
    settings: Settings,
    hooks: ShellHooks,
    commands: Receiver<BridgeMessage>,
    shell: Arc<dyn DesktopShell>,
    recording: RecordingHost,
    library: LibraryHost,
    player: PlayerHost,
    transcription: TranscriptionHost,
    model_host: ModelHost,
    models_dir: PathBuf,
    library_catalog: LibraryCatalog,
    library_selected: Option<RecordingId>,
    player_snapshot: PlayerSnapshot,
    transcription_snapshot: TranscriptionSnapshot,
    transcript_editor: Entity<EditorState>,
    transcript_generation: u64,
    transcript_error: Option<&'static str>,
    transcript_load: Option<Task<()>>,
    prompt_model_id: String,
    model_prompt_open: bool,
    /// Set only when Transcribe found no model. Starts once a model is ready.
    pending_transcription: Option<RecordingId>,
    recordings_table: Entity<TableState<RecordingsDelegate>>,
    _table_sub: Subscription,
    levels: Arc<LevelSnapshot>,
    gain_control: Arc<GainControl>,
    targets: Arc<MonitorTargets>,
    catalog: DeviceCatalog,
    microphone: Entity<SelectState<SearchableVec<SharedString>>>,
    mic_labels: Vec<SharedString>,
    mic_choices: Vec<StoredEndpoint>,
    _mic_sub: Subscription,
    system: Entity<SelectState<SearchableVec<SharedString>>>,
    system_labels: Vec<SharedString>,
    system_choices: Vec<StoredEndpoint>,
    _system_sub: Subscription,
    gain_slider: Entity<SliderState>,
    _gain_sub: Subscription,
    mic_meter: PeakHold,
    system_meter: PeakHold,
    _meters: Task<()>,
    started: Option<Instant>,
    exit_after_stop: bool,
    last_saved: Option<LogicalRect>,
    pending_bounds: Option<(LogicalRect, Instant)>,
    _bridge: Task<()>,
    _appearance: Subscription,
}

impl MainWindow {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        presentation: Presentation,
        settings: Settings,
        hooks: ShellHooks,
        commands: Receiver<BridgeMessage>,
        shell: Arc<dyn DesktopShell>,
        recording: RecordingHost,
        library: LibraryHost,
        player: PlayerHost,
        transcription: TranscriptionHost,
        model_host: ModelHost,
        models_dir: PathBuf,
        levels: Arc<LevelSnapshot>,
        gain_control: Arc<GainControl>,
        targets: Arc<MonitorTargets>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle().tab_stop(true);
        window.focus(&focus, cx);
        let recordings_table = cx.new(|cx| {
            TableState::new(RecordingsDelegate::new(), window, cx)
                .row_selectable(true)
                .sortable(true)
        });
        let table_sub = cx.subscribe_in(
            &recordings_table,
            window,
            |this, table, event: &TableEvent, window, cx| {
                if let TableEvent::SelectRow(row) = event {
                    let id = table.read(cx).delegate().row_id(*row);
                    this.select_library_row(id, window, cx);
                }
            },
        );
        let labels = vec![SharedString::from("Default communications")];
        let choices = vec![StoredEndpoint::Default];
        let microphone = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(labels.clone()),
                Some(gpui_kit::component::IndexPath::new(0)),
                window,
                cx,
            )
        });
        let mic_sub = cx.subscribe_in(
            &microphone,
            window,
            |this, _select, event: &SelectEvent<SearchableVec<SharedString>>, window, cx| {
                if let SelectEvent::Confirm(Some(label)) = event {
                    this.choose_microphone(label.clone(), window, cx);
                }
            },
        );
        let system_labels = vec![SharedString::from("Default output device")];
        let system_choices = vec![StoredEndpoint::Default];
        let system = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(system_labels.clone()),
                Some(gpui_kit::component::IndexPath::new(0)),
                window,
                cx,
            )
        });
        let system_sub = cx.subscribe_in(
            &system,
            window,
            |this, _select, event: &SelectEvent<SearchableVec<SharedString>>, window, cx| {
                if let SelectEvent::Confirm(Some(label)) = event {
                    this.choose_system(label.clone(), window, cx);
                }
            },
        );
        let gain_slider = cx.new(|_| {
            SliderState::new()
                .min(0.0)
                .max(4.0)
                .step(0.1)
                .default_value(settings.mic_gain())
        });
        let gain_sub = cx.subscribe_in(
            &gain_slider,
            window,
            |this, _slider, event: &SliderEvent, _window, cx| {
                let (SliderEvent::Change(value) | SliderEvent::Release(value)) = event;
                this.set_slider_gain(value.start(), cx);
            },
        );
        let executor = cx.background_executor().clone();
        let bridge = cx.spawn_in(window, async move |this, async_cx| loop {
            executor.timer(Duration::from_millis(150)).await;
            let step = this.update_in(async_cx, |view, window, cx| view.on_tick(window, cx));
            if !matches!(step, Ok(true)) {
                break;
            }
        });
        let meter_executor = cx.background_executor().clone();
        let meters = cx.spawn_in(window, async move |this, async_cx| loop {
            meter_executor.timer(Duration::from_millis(33)).await;
            let step = this.update_in(async_cx, |view, _window, cx| {
                if view.hooks.exit_requested() {
                    return false;
                }
                if view.sample_meters() {
                    cx.notify();
                }
                true
            });
            if !matches!(step, Ok(true)) {
                break;
            }
        });
        targets.set(
            endpoint_token(settings.microphone()),
            endpoint_token(settings.system_audio()),
        );
        let session = cx.entity();
        window.on_window_should_close(cx, move |window, cx| {
            session.update(cx, |view, cx| view.on_close_requested(window, cx))
        });
        let transcript_editor = cx.new(|cx| EditorState::new(window, cx).line_number(false));
        let prompt_model_id = settings.selected_model().to_string();
        // Status surfaces need the installed list before the dialog ever opens.
        model_host.refresh();
        let appearance_entity = cx.weak_entity();
        let appearance = window.observe_window_appearance(move |window, cx| {
            let Some(entity) = appearance_entity.upgrade() else {
                return;
            };
            entity.update(cx, |view, cx| {
                if view.settings.theme() == ThemePreference::System {
                    apply_theme(ThemePreference::System, Some(window), cx);
                    cx.notify();
                }
            });
        });
        Self {
            focus,
            presentation,
            settings,
            hooks,
            commands,
            shell,
            recording,
            library,
            player,
            transcription,
            model_host,
            models_dir,
            library_catalog: LibraryCatalog::empty(),
            library_selected: None,
            player_snapshot: PlayerSnapshot {
                phase: PlayerPhase::Idle,
                recording_id: None,
                notice: None,
            },
            transcription_snapshot: TranscriptionSnapshot::idle(),
            transcript_editor,
            transcript_generation: 0,
            transcript_error: None,
            transcript_load: None,
            prompt_model_id,
            model_prompt_open: false,
            pending_transcription: None,
            recordings_table,
            _table_sub: table_sub,
            levels,
            gain_control,
            targets,
            catalog: DeviceCatalog::empty(),
            microphone,
            mic_labels: labels,
            mic_choices: choices,
            _mic_sub: mic_sub,
            system,
            system_labels,
            system_choices,
            _system_sub: system_sub,
            gain_slider,
            _gain_sub: gain_sub,
            mic_meter: PeakHold::new(),
            system_meter: PeakHold::new(),
            _meters: meters,
            started: None,
            exit_after_stop: false,
            last_saved: None,
            pending_bounds: None,
            _bridge: bridge,
            _appearance: appearance,
        }
    }

    pub(crate) fn close_to_tray(&self) -> bool {
        self.settings.close_to_tray()
    }

    pub(crate) fn set_close_to_tray(&mut self, close_to_tray: bool, cx: &mut Context<Self>) {
        self.settings.set_close_to_tray(close_to_tray);
        self.presentation.set_close_to_tray(close_to_tray);
        self.hooks.save_settings(self.settings.clone());
        cx.notify();
    }

    pub(crate) fn cycle_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let system_is_dark = matches!(
            window.appearance(),
            gpui_kit::WindowAppearance::Dark | gpui_kit::WindowAppearance::VibrantDark
        );
        let next = self.settings.theme().next(system_is_dark);
        self.settings.set_theme(next);
        self.hooks.save_settings(self.settings.clone());
        apply_theme(next, Some(window), cx);
        cx.notify();
    }

    pub(crate) fn handle(
        &mut self,
        command: AppCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let AppCommand::SelectMicrophone(choice) = command {
            if !self.presentation.inputs_locked() {
                self.settings.set_microphone(choice);
                self.hooks.save_settings(self.settings.clone());
                self.publish_targets();
            }
            cx.notify();
            return;
        }
        if let AppCommand::SelectSystem(choice) = command {
            if !self.presentation.inputs_locked() {
                self.settings.set_system_audio(choice);
                self.hooks.save_settings(self.settings.clone());
                self.publish_targets();
            }
            cx.notify();
            return;
        }
        let effect = apply(&mut self.presentation, command);
        self.perform(effect, window, cx);
    }

    fn on_close_requested(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.flush_bounds(window);
        if self.presentation.close_to_tray() {
            let effect = apply(&mut self.presentation, AppCommand::Hide);
            self.perform(effect, window, cx);
            false
        } else {
            self.hooks.save_settings(self.settings.clone());
            self.hooks.request_exit();
            true
        }
    }

    fn on_tick(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.hooks.exit_requested() {
            return false;
        }
        let batch = drain_bridge(&self.commands, &mut self.presentation);
        let mut changed = !batch.effects.is_empty();
        if let Some(snapshot) = batch.recording {
            self.apply_snapshot(snapshot, window, cx);
            changed = true;
        }
        if let Some(catalog) = batch.devices {
            self.apply_devices(catalog, window, cx);
            changed = true;
        }
        if let Some(snapshot) = batch.library {
            self.apply_library(snapshot, window, cx);
            changed = true;
        }
        if let Some(snapshot) = batch.player {
            self.apply_player(snapshot);
            changed = true;
        }
        if let Some(snapshot) = batch.transcription {
            self.apply_transcription(snapshot, window, cx);
            changed = true;
        }
        if let Some(snapshot) = batch.model {
            self.apply_model(snapshot, window, cx);
            changed = true;
        }
        for effect in batch.effects {
            self.perform(effect, window, cx);
            changed = true;
        }
        if self.presentation.pending_show() {
            self.show_window(window);
            changed = true;
        }
        if self.presentation.close_to_tray() && self.shell.window_is_minimized() {
            let effect = apply(&mut self.presentation, AppCommand::Hide);
            self.perform(effect, window, cx);
            changed = true;
        }
        if self.note_bounds(window) {
            changed = true;
        }
        if self.presentation.record_phase() == RecordPhase::Recording {
            changed = true;
        }
        if changed {
            cx.notify();
        }
        !self.hooks.exit_requested()
    }

    fn perform(&mut self, effect: Effect, window: &mut Window, cx: &mut Context<Self>) {
        match effect {
            Effect::None | Effect::Unavailable => {}
            Effect::Show => self.show_window(window),
            Effect::ShowSelect { recording_id } => {
                self.show_window(window);
                if let Some(id) = recording_id {
                    if self.library_catalog.find(id).is_some() {
                        self.presentation.set_tab(AppTab::Recordings);
                        self.select_library_row(Some(id), window, cx);
                    }
                }
            }
            Effect::Hide { explain } => {
                self.flush_bounds(window);
                let _ = self.shell.set_window_visible(false);
                if explain {
                    self.settings.set_tray_explanation_seen(true);
                    self.hooks.save_settings(self.settings.clone());
                    open_tray_explanation(window, cx);
                }
            }
            Effect::OpenFolder => {
                if let Err(error) = self
                    .shell
                    .open_recordings_folder(self.settings.recordings_directory())
                {
                    tracing::error!(error = %error, "recordings folder was not opened");
                    self.presentation
                        .set_notice(Some("Could not open the recordings folder."));
                }
            }
            Effect::OpenSettings => {
                let session = cx.entity();
                let watched = session.clone();
                let model_entity = session.clone();
                let on_change = move |checked: &bool, _: &mut Window, cx: &mut App| {
                    session.update(cx, |view, cx| view.set_close_to_tray(*checked, cx));
                };
                let on_change_model = move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                    model_entity.update(cx, |view, cx| {
                        view.handle(AppCommand::OpenModelDialog, window, cx);
                    });
                };
                let read = move |cx: &App| watched.read_with(cx, |view, _| view.settings_view());
                open_settings(window, cx, read, on_change, on_change_model);
            }
            Effect::OpenModelDialog => self.open_model_prompt(window, cx),
            Effect::OpenAbout => open_about(window, cx),
            Effect::StartRecording => self.begin_recording(),
            Effect::StopRecording => self.recording.stop(),
            Effect::BrowseRecordings => self.browse_folder(),
            Effect::RefreshLibrary => self.library.refresh(),
            Effect::Transcribe { recording_id } => {
                self.begin_transcription(recording_id, window, cx)
            }
            Effect::CancelTranscription { job_id } => self.transcription.cancel(job_id),
            Effect::ConfirmExit => {
                let entity = cx.entity();
                open_exit_while_recording(window, cx, move |window, cx| {
                    entity.update(cx, |view, cx| {
                        view.exit_after_stop = true;
                        view.recording.stop();
                        cx.notify();
                        let _ = window;
                    });
                });
            }
            Effect::Quit => self.quit_now(window),
            Effect::DownloadModel => {
                self.presentation
                    .begin_download(self.prompt_model_id.clone());
                self.model_host.download(self.prompt_model_id.clone());
            }
            Effect::CancelModelDownload => self.model_host.cancel(),
            Effect::RetryModelDownload => {
                // The footer only offers Retry for the model the worker last tried.
                self.presentation
                    .begin_download(self.prompt_model_id.clone());
                self.model_host.retry();
            }
            Effect::UseModel => {
                self.settings
                    .set_selected_model(self.prompt_model_id.clone());
                self.hooks.save_settings(self.settings.clone());
                self.finish_model_choice(window, cx);
            }
        }
        cx.notify();
    }

    fn show_window(&mut self, window: &mut Window) {
        if self.shell.set_window_visible(true) {
            window.activate_window();
            self.presentation.set_visible(true);
            self.presentation.set_pending_show(false);
        } else {
            self.presentation.set_pending_show(true);
        }
    }

    fn note_bounds(&mut self, window: &Window) -> bool {
        if !self.presentation.visible() {
            return false;
        }
        let Some(rect) = logical_bounds(window) else {
            return false;
        };
        if self.last_saved == Some(rect) {
            self.pending_bounds = None;
            return false;
        }
        match self.pending_bounds {
            Some((pending, at)) if pending == rect && at.elapsed() >= BOUNDS_DEBOUNCE => {
                self.commit_bounds(rect);
                true
            }
            Some((pending, _)) if pending == rect => false,
            _ => {
                self.pending_bounds = Some((rect, Instant::now()));
                false
            }
        }
    }

    fn flush_bounds(&mut self, window: &Window) {
        if let Some(rect) = logical_bounds(window) {
            self.commit_bounds(rect);
        }
    }

    fn commit_bounds(&mut self, rect: LogicalRect) {
        let Some(stored) = StoredWindowBounds::new(rect.x(), rect.y(), rect.width(), rect.height())
        else {
            return;
        };
        self.settings.set_window_bounds(Some(stored));
        self.hooks.save_settings(self.settings.clone());
        self.last_saved = Some(rect);
        self.pending_bounds = None;
    }

    fn choose_microphone(
        &mut self,
        label: SharedString,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.presentation.inputs_locked() {
            return;
        }
        let Some(index) = self.mic_labels.iter().position(|item| item == &label) else {
            return;
        };
        let Some(choice) = self.mic_choices.get(index).cloned() else {
            return;
        };
        self.handle(AppCommand::SelectMicrophone(choice), _window, cx);
    }

    fn choose_system(&mut self, label: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        if self.presentation.inputs_locked() {
            return;
        }
        let Some(index) = self.system_labels.iter().position(|item| item == &label) else {
            return;
        };
        let Some(choice) = self.system_choices.get(index).cloned() else {
            return;
        };
        self.handle(AppCommand::SelectSystem(choice), window, cx);
    }

    fn set_slider_gain(&mut self, gain: f32, cx: &mut Context<Self>) {
        self.gain_control.set_slider(gain);
        self.settings.set_mic_gain(self.gain_control.slider());
        self.hooks.save_settings(self.settings.clone());
        cx.notify();
    }

    fn sample_meters(&mut self) -> bool {
        let mic = self.mic_meter.update(self.levels.take_mic());
        let system = self.system_meter.update(self.levels.take_system());
        let _mixed = self.levels.take_mixed();
        mic || system
    }

    fn publish_targets(&self) {
        self.targets.set(
            endpoint_token(self.settings.microphone()),
            endpoint_token(self.settings.system_audio()),
        );
    }

    fn apply_snapshot(
        &mut self,
        snapshot: RecordSnapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let phase = snapshot.phase;
        if phase == RecordPhase::Recording && self.started.is_none() {
            self.started = Some(Instant::now());
        }
        let became_idle = matches!(phase, RecordPhase::Idle | RecordPhase::Error)
            && self.presentation.record_phase() != phase;
        if matches!(phase, RecordPhase::Idle | RecordPhase::Error) {
            self.started = None;
        }
        self.presentation.set_record_phase(phase);
        self.presentation.set_notice(snapshot.notice);
        if became_idle && snapshot.id.is_some() {
            self.library.refresh();
            if phase == RecordPhase::Idle && self.settings.auto_transcribe() {
                if let (Some(id), Some(path)) = (snapshot.id, snapshot.finalized_path.clone()) {
                    self.enqueue_transcription(id, path, window, cx);
                }
            }
        }
        if self.exit_after_stop && matches!(phase, RecordPhase::Idle | RecordPhase::Error) {
            self.exit_after_stop = false;
            self.quit_now(window);
        }
    }

    fn apply_transcription(
        &mut self,
        snapshot: TranscriptionSnapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tab = self.presentation.tab();
        self.transcription_snapshot = snapshot.clone();
        self.presentation.apply_transcription(snapshot.clone());
        self.presentation.set_tab(tab);
        if let Some(id) = snapshot.recording_id {
            let display = snapshot.transcript_status().display();
            self.recordings_table.update(cx, |table, cx| {
                if table.delegate_mut().set_transcript_display(id, display) {
                    cx.notify();
                }
            });
        }
        if matches!(
            snapshot.phase,
            JobPhase::Done | JobPhase::Failed | JobPhase::Cancelled | JobPhase::Queued
        ) {
            self.library.refresh();
        }
        if snapshot.phase == JobPhase::Done && snapshot.recording_id == self.library_selected {
            self.load_selected_transcript(window, cx);
        }
    }

    fn apply_model(
        &mut self,
        snapshot: ModelSnapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let previous = self.presentation.model_failure();
        self.presentation.apply_model(snapshot);
        if let Some(id) = self.presentation.take_installed_target() {
            self.settings.set_selected_model(id.clone());
            self.hooks.save_settings(self.settings.clone());
            self.prompt_model_id = id;
            self.finish_model_choice(window, cx);
        }
        if let Some(message) = self.presentation.model_failure() {
            if previous != Some(message)
                && is_distinct_model_message(message)
                && error_display(self.model_prompt_open) == ErrorDisplay::Alert
            {
                open_model_alert(window, cx, message);
            }
        }
    }

    fn finish_model_choice(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match after_model_ready(self.pending_transcription.take()) {
            AfterModelReady::CloseAndTranscribe(recording_id) => {
                if self.model_prompt_open {
                    self.model_prompt_open = false;
                    if window.has_active_dialog(cx) {
                        window.close_dialog(cx);
                    }
                }
                self.begin_transcription(recording_id, window, cx);
            }
            AfterModelReady::StayOpen => {}
        }
    }

    /// Runs when the user dismisses the dialog. Abandons any Transcribe that was waiting.
    fn model_prompt_closed(&mut self) {
        self.model_prompt_open = false;
        self.pending_transcription = None;
        self.presentation.clear_download_target();
    }

    fn begin_transcription(
        &mut self,
        recording_id: RecordingId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = self.library_catalog.find(recording_id) else {
            self.presentation
                .set_notice(Some("Could not find that recording."));
            return;
        };
        self.enqueue_transcription(recording_id, entry.audio_path.clone(), window, cx);
    }

    fn enqueue_transcription(
        &mut self,
        recording_id: RecordingId,
        audio_path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match resolve_model_path(&self.models_dir, self.settings.selected_model()) {
            Ok((model_path, model_id)) => {
                self.transcription
                    .enqueue(recording_id, audio_path, model_path, model_id);
            }
            Err(error) => {
                tracing::error!(error = %error, "transcription model was not resolved");
                if error.user_message() == MODEL_MISSING_MESSAGE {
                    self.pending_transcription = Some(recording_id);
                }
                self.report_model_error(error.user_message(), window, cx);
            }
        }
    }

    fn report_model_error(
        &mut self,
        message: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if message == MODEL_MISSING_MESSAGE {
            self.open_model_prompt(window, cx);
            return;
        }
        if is_distinct_model_message(message) {
            open_model_alert(window, cx, message);
            return;
        }
        self.presentation.set_notice(Some(message));
    }

    fn row_transcript_status(&self, id: RecordingId) -> TranscriptStatus {
        if let Some(status) = self.presentation.status_for(id) {
            return status;
        }
        self.library_catalog
            .find(id)
            .map(|entry| entry.info.transcript())
            .unwrap_or(TranscriptStatus::None)
    }

    fn job_active_for(&self, id: Option<RecordingId>) -> bool {
        let Some(id) = id else {
            return false;
        };
        self.presentation
            .active_job()
            .is_some_and(|(recording, _)| recording == id)
    }

    fn load_selected_transcript(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.library_selected else {
            self.clear_transcript_editor(window, cx);
            return;
        };
        if self.row_transcript_status(id) != TranscriptStatus::Ready {
            self.transcript_error = None;
            self.clear_transcript_editor(window, cx);
            return;
        }
        let Some(entry) = self.library_catalog.find(id) else {
            self.clear_transcript_editor(window, cx);
            return;
        };
        let path = transcript_path_for(&entry.audio_path);
        self.transcript_generation = self.transcript_generation.saturating_add(1);
        let generation = self.transcript_generation;
        self.transcript_load = Some(cx.spawn_in(window, async move |this, async_cx| {
            let loaded = async_cx
                .background_spawn(async move { read_transcript_file(path) })
                .await;
            let _ = this.update_in(async_cx, |view, window, cx| {
                if view.transcript_generation != generation || view.library_selected != Some(id) {
                    return;
                }
                match loaded {
                    Ok(text) => {
                        view.transcript_error = None;
                        view.presentation.set_loaded_transcript(text.clone());
                        view.transcript_editor.update(cx, |editor, cx| {
                            editor.set_value(text, window, cx);
                        });
                    }
                    Err(message) => {
                        view.transcript_error = Some(message);
                        view.presentation.set_loaded_transcript(String::new());
                        view.transcript_editor.update(cx, |editor, cx| {
                            editor.set_value(String::new(), window, cx);
                        });
                    }
                }
                cx.notify();
            });
        }));
    }

    fn clear_transcript_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.transcript_generation = self.transcript_generation.saturating_add(1);
        self.presentation.set_loaded_transcript(String::new());
        self.transcript_editor.update(cx, |editor, cx| {
            editor.set_value(String::new(), window, cx);
        });
    }

    fn open_model_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Never stack a second model dialog (the tray can open Settings over this one).
        if self.model_prompt_open {
            return;
        }
        self.model_host.refresh();
        self.model_prompt_open = true;
        self.reset_prompt_selection();
        let read_entity = cx.entity();
        let select_entity = read_entity.clone();
        let download_entity = read_entity.clone();
        let use_entity = read_entity.clone();
        let cancel_entity = read_entity.clone();
        let retry_entity = read_entity.clone();
        let closed_entity = read_entity.clone();
        open_model_prompt(
            window,
            cx,
            move |cx| read_entity.read(cx).model_prompt_view(),
            ModelPromptHandlers {
                on_select: Rc::new(move |id, _window, cx| {
                    select_entity.update(cx, |view, cx| {
                        view.select_prompt_model(id);
                        cx.notify();
                    });
                }),
                on_download: Rc::new(move |_event, window, cx| {
                    download_entity.update(cx, |view, cx| {
                        view.handle(AppCommand::DownloadModel, window, cx);
                    });
                }),
                on_use: Rc::new(move |_event, window, cx| {
                    use_entity.update(cx, |view, cx| {
                        view.handle(AppCommand::UseModel, window, cx);
                    });
                }),
                on_cancel: Rc::new(move |_event, window, cx| {
                    cancel_entity.update(cx, |view, cx| {
                        view.handle(AppCommand::CancelModelDownload, window, cx);
                    });
                }),
                on_retry: Rc::new(move |_event, window, cx| {
                    retry_entity.update(cx, |view, cx| {
                        view.handle(AppCommand::RetryModelDownload, window, cx);
                    });
                }),
                on_closed: Rc::new(move |_event, _window, cx| {
                    closed_entity.update(cx, |view, cx| {
                        view.model_prompt_closed();
                        cx.notify();
                    });
                }),
            },
        );
    }

    /// Each opening starts on the saved model, never on the last highlight.
    fn reset_prompt_selection(&mut self) {
        let Ok(manifest) = ModelManifest::load_bundled() else {
            return;
        };
        let entry = manifest
            .by_id(self.settings.selected_model())
            .or_else(|| manifest.default_entry());
        if let Some(entry) = entry {
            self.prompt_model_id = entry.id().to_string();
        }
    }

    /// Highlights only. The saved model changes on Use or after a finished download.
    fn select_prompt_model(&mut self, id: &str) {
        self.prompt_model_id = id.to_string();
    }

    fn settings_view(&self) -> SettingsView {
        let status = self
            .presentation
            .model_status(self.settings.selected_model());
        let bytes = ModelManifest::load_bundled().ok().and_then(|manifest| {
            let id = match &status {
                ModelStatus::InUse { id } => id,
                _ => return None,
            };
            manifest.by_id(id).map(|entry| entry.bytes())
        });
        SettingsView {
            close_to_tray: self.close_to_tray(),
            model: settings_model_row(&status, bytes),
        }
    }

    fn model_prompt_view(&self) -> ModelPromptView {
        let phase = self.presentation.model_phase();
        let busy = phase == ModelPhase::Downloading;
        let Ok(manifest) = ModelManifest::load_bundled() else {
            return ModelPromptView {
                cards: Vec::new(),
                footer: model_footer(None, phase, None),
                selected_bytes: None,
                selected_name: SharedString::default(),
            };
        };
        let state_of = |id: &str| {
            model_install_state(
                id,
                self.settings.selected_model(),
                self.presentation.model_files(),
            )
        };
        let selected = manifest.by_id(&self.prompt_model_id);
        let target = self.presentation.download_target();
        // A failure belongs to the model that was being fetched, else the highlighted one.
        let failure_id = target.or(selected.map(|entry| entry.id()));
        ModelPromptView {
            cards: manifest
                .entries()
                .iter()
                .map(|entry| {
                    let id = entry.id();
                    let state = state_of(id);
                    let copy = model_copy(id);
                    let is_selected = selected.is_some_and(|chosen| chosen.id() == id);
                    let downloading_this = busy && target == Some(id);
                    ModelCardData {
                        id: SharedString::from(id),
                        name: SharedString::from(copy.name.to_string()),
                        description: SharedString::from(copy.description.to_string()),
                        meta: SharedString::from(row_meta(state, entry.bytes())),
                        tag: row_tag(state),
                        ratings: copy.ratings,
                        selected: is_selected,
                        disabled: busy,
                        progress: self
                            .presentation
                            .model_progress()
                            .filter(|_| downloading_this)
                            .map(|percent| card_progress(percent, entry.bytes())),
                        failure: self
                            .presentation
                            .model_failure()
                            .filter(|_| !busy && failure_id == Some(id))
                            .map(SharedString::from),
                        accessibility_label: SharedString::from(row_accessibility_label(
                            &copy, state,
                        )),
                    }
                })
                .collect(),
            footer: model_footer(
                selected.map(|entry| (entry.id(), state_of(entry.id()))),
                phase,
                target,
            ),
            selected_bytes: selected.map(|entry| entry.bytes()),
            selected_name: selected
                .map(|entry| SharedString::from(model_copy(entry.id()).name.to_string()))
                .unwrap_or_default(),
        }
    }

    fn apply_library(
        &mut self,
        snapshot: LibrarySnapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selection_changed = self.library_selected != snapshot.selected;
        if selection_changed {
            self.player.stop();
        }
        self.library_catalog = snapshot.catalog;
        self.library_selected = snapshot.selected;
        self.presentation.set_selected_recording(snapshot.selected);
        if let Some(notice) = snapshot.notice {
            self.presentation.set_notice(Some(notice));
        }
        let rows = rows_from_catalog(&self.library_catalog);
        let selected = self.library_selected;
        let overlay = self
            .presentation
            .transcript_recording()
            .zip(self.presentation.transcript_status());
        self.recordings_table.update(cx, |table, cx| {
            table.delegate_mut().set_rows(rows);
            if let Some((id, status)) = overlay {
                let _ = table
                    .delegate_mut()
                    .set_transcript_display(id, status.display());
            }
            // Rebuild column layout after row data changes (Kit TableState contract).
            table.refresh(cx);
            if let Some(id) = selected {
                if let Some(index) = table.delegate().index_of(id) {
                    table.set_selection(TableSelection::Row(index), cx);
                }
            }
            cx.notify();
        });
        if selection_changed {
            self.load_selected_transcript(window, cx);
        }
    }

    fn apply_player(&mut self, snapshot: PlayerSnapshot) {
        self.player_snapshot = snapshot;
        if let Some(notice) = self.player_snapshot.notice {
            self.presentation.set_notice(Some(notice));
        }
    }

    fn select_library_row(
        &mut self,
        id: Option<RecordingId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.library_selected != id {
            self.player.stop();
        }
        self.library_selected = id;
        self.presentation.set_selected_recording(id);
        self.library.select(id);
        self.load_selected_transcript(window, cx);
        cx.notify();
    }

    fn play_selected(&mut self) {
        let Some(id) = self.library_selected else {
            return;
        };
        let Some(entry) = self.library_catalog.find(id) else {
            return;
        };
        if entry.info.container() != crate::domain::recording::RecordingContainer::Wav {
            self.presentation
                .set_notice(Some("Only WAV playback is available."));
            return;
        }
        self.player.play(id, entry.audio_path.clone());
    }

    fn rename_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.library_selected else {
            return;
        };
        let Some(entry) = self.library_catalog.find(id) else {
            return;
        };
        let current = SharedString::from(entry.info.name().to_string());
        let entity = cx.entity();
        open_rename_dialog(window, cx, current, move |name, _window, cx| {
            entity.update(cx, |view, _cx| {
                view.library.rename(id, name);
            });
        });
    }

    fn delete_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.library_selected else {
            return;
        };
        let Some(entry) = self.library_catalog.find(id) else {
            return;
        };
        let name = SharedString::from(entry.info.name().to_string());
        let entity = cx.entity();
        open_delete_dialog(window, cx, name, move |_window, cx| {
            entity.update(cx, |view, _cx| {
                view.player.stop();
                view.library.delete(id);
            });
        });
    }

    fn apply_devices(
        &mut self,
        catalog: DeviceCatalog,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.presentation.inputs_locked() {
            self.recording.devices_changed();
        }
        self.catalog = catalog;
        let mut labels = vec![SharedString::from("Default communications")];
        let mut choices = vec![StoredEndpoint::Default];
        for device in &self.catalog.capture {
            if !device.is_active() {
                continue;
            }
            let mut label = device.name().to_string();
            if labels.iter().any(|existing| existing.as_ref() == label) {
                label.push_str(" (2)");
            }
            labels.push(SharedString::from(label));
            choices.push(StoredEndpoint::explicit(device.id().as_str()));
        }
        let selected = choices
            .iter()
            .position(|choice| choice == self.settings.microphone())
            .unwrap_or(0);
        if selected == 0 && !matches!(self.settings.microphone(), StoredEndpoint::Default) {
            self.presentation.set_notice(Some(NOTICE_FALLBACK));
            self.settings.set_microphone(StoredEndpoint::Default);
            self.hooks.save_settings(self.settings.clone());
        }
        self.install_mics(labels, choices, selected, window, cx);
        let mut system_labels = vec![SharedString::from("Default output device")];
        let mut system_choices = vec![StoredEndpoint::Default];
        for device in &self.catalog.render {
            if !device.is_active() {
                continue;
            }
            let mut label = device.name().to_string();
            if system_labels
                .iter()
                .any(|existing| existing.as_ref() == label)
            {
                label.push_str(" (2)");
            }
            system_labels.push(SharedString::from(label));
            system_choices.push(StoredEndpoint::explicit(device.id().as_str()));
        }
        let system_selected = system_choices
            .iter()
            .position(|choice| choice == self.settings.system_audio())
            .unwrap_or(0);
        if system_selected == 0 && !matches!(self.settings.system_audio(), StoredEndpoint::Default)
        {
            self.presentation.set_notice(Some(NOTICE_SYSTEM_FALLBACK));
            self.settings.set_system_audio(StoredEndpoint::Default);
            self.hooks.save_settings(self.settings.clone());
        }
        self.install_system(system_labels, system_choices, system_selected, window, cx);
        self.publish_targets();
    }

    fn install_mics(
        &mut self,
        labels: Vec<SharedString>,
        choices: Vec<StoredEndpoint>,
        selected: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let microphone = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(labels.clone()),
                Some(gpui_kit::component::IndexPath::new(selected)),
                window,
                cx,
            )
        });
        self._mic_sub = cx.subscribe_in(
            &microphone,
            window,
            |this, _select, event: &SelectEvent<SearchableVec<SharedString>>, window, cx| {
                if let SelectEvent::Confirm(Some(label)) = event {
                    this.choose_microphone(label.clone(), window, cx);
                }
            },
        );
        self.microphone = microphone;
        self.mic_labels = labels;
        self.mic_choices = choices;
    }

    fn install_system(
        &mut self,
        labels: Vec<SharedString>,
        choices: Vec<StoredEndpoint>,
        selected: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let system = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(labels.clone()),
                Some(gpui_kit::component::IndexPath::new(selected)),
                window,
                cx,
            )
        });
        self._system_sub = cx.subscribe_in(
            &system,
            window,
            |this, _select, event: &SelectEvent<SearchableVec<SharedString>>, window, cx| {
                if let SelectEvent::Confirm(Some(label)) = event {
                    this.choose_system(label.clone(), window, cx);
                }
            },
        );
        self.system = system;
        self.system_labels = labels;
        self.system_choices = choices;
    }

    fn begin_recording(&mut self) {
        match build_start_request(
            self.settings.recordings_directory().to_path_buf(),
            self.settings.microphone(),
            self.settings.system_audio(),
            &self.catalog,
        ) {
            StartRequestBuild::Ready(request) => self.recording.start(request),
            StartRequestBuild::Refused { notice } => {
                self.presentation.set_notice(Some(notice));
            }
        }
    }

    fn browse_folder(&mut self) {
        match self.shell.pick_recordings_folder() {
            Ok(Some(path)) => {
                if self.settings.set_recordings_directory(path.clone()).is_ok() {
                    self.hooks.save_settings(self.settings.clone());
                    self.library.set_directory(path);
                    self.player.stop();
                    self.presentation.set_notice(None);
                }
            }
            Ok(None) => {}
            Err(error) => {
                tracing::error!(error = %error, "folder was not chosen");
                self.presentation.set_notice(Some(error.user_message()));
            }
        }
    }

    fn quit_now(&mut self, window: &mut Window) {
        self.flush_bounds(window);
        self.hooks.save_settings(self.settings.clone());
        self.hooks.request_exit();
        window.remove_window();
    }
}

fn apply(state: &mut Presentation, command: AppCommand) -> Effect {
    crate::views::actions::apply_command(state, command)
}

fn read_transcript_file(
    path: Result<PathBuf, crate::common::error::AppError>,
) -> Result<String, &'static str> {
    use crate::common::error::AppError;
    let path = path.map_err(|error| error.user_message())?;
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Err(AppError::message("transcript missing", "not found").user_message())
        }
        Err(error) => Err(AppError::io("read transcript", error).user_message()),
    }
}

const MODEL_MISSING_MESSAGE: &str = "The transcription model file is missing.";

fn is_distinct_model_message(message: &str) -> bool {
    matches!(
        message,
        "The transcription model file is missing."
            | "The transcription model file is not supported."
            | "The downloaded transcription model could not be verified."
    )
}

fn resolve_model_path(
    models_dir: &std::path::Path,
    selected_id: &str,
) -> Result<(PathBuf, String), crate::common::error::AppError> {
    let manifest = ModelManifest::load_bundled()?;
    let entry = manifest
        .by_id(selected_id)
        .or_else(|| manifest.default_entry())
        .ok_or_else(|| {
            crate::common::error::AppError::message("model missing", "no model in manifest")
        })?;
    let path = path_for_variant(models_dir, entry);
    validate_model_file(&path)?;
    Ok((path, entry.id().to_string()))
}

fn endpoint_token(choice: &StoredEndpoint) -> String {
    match choice {
        StoredEndpoint::Default => String::new(),
        StoredEndpoint::Explicit(id) => id.clone(),
    }
}

struct PeakHold {
    level: f32,
    hold: f32,
    hold_at: Instant,
}

impl PeakHold {
    fn new() -> Self {
        Self {
            level: -60.0,
            hold: -60.0,
            hold_at: Instant::now(),
        }
    }

    fn update(&mut self, linear: f32) -> bool {
        let db = db_fs(linear);
        let mut changed = (db - self.level).abs() > 0.4;
        self.level = db;
        if db >= self.hold || self.hold_at.elapsed() >= Duration::from_secs(1) {
            if (self.hold - db).abs() > 0.4 {
                changed = true;
            }
            self.hold = db;
            self.hold_at = Instant::now();
        }
        changed
    }
}

fn logical_bounds(window: &Window) -> Option<LogicalRect> {
    let bounds = window.window_bounds().get_bounds();
    LogicalRect::new(
        bounds.origin.x.to_i32(),
        bounds.origin.y.to_i32(),
        bounds.size.width.to_i32(),
        bounds.size.height.to_i32(),
    )
}

impl Render for MainWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = match self.presentation.tab() {
            AppTab::Record => 0,
            AppTab::Recordings => 1,
        };
        let locked = self.presentation.inputs_locked();
        let phase = self.presentation.record_phase();
        let show_stop = matches!(
            phase,
            RecordPhase::Starting | RecordPhase::Recording | RecordPhase::Stopping
        );
        let elapsed = format_elapsed(
            self.started
                .map(|started| started.elapsed())
                .unwrap_or(Duration::ZERO),
        );
        let status_elapsed = elapsed.clone();
        let phase_label = match phase {
            RecordPhase::Idle => "Ready",
            RecordPhase::Starting => "Starting",
            RecordPhase::Recording => "Recording",
            RecordPhase::Stopping => "Saving",
            RecordPhase::Error => "Not recording",
        };
        let notice = SharedString::from(self.presentation.notice().unwrap_or(""));
        let entity = cx.entity();
        let toggle_entity = entity.clone();
        let body = match self.presentation.tab() {
            AppTab::Record => record_tab(
                &self.microphone,
                &self.system,
                &self.gain_slider,
                locked,
                show_stop,
                phase == RecordPhase::Stopping,
                SharedString::from(format_gain(self.settings.mic_gain())),
                SharedString::from(elapsed),
                SharedString::from(phase_label),
                notice,
                self.mic_meter.level,
                self.mic_meter.hold,
                self.system_meter.level,
                self.system_meter.hold,
                move |_event, window, cx| {
                    toggle_entity.update(cx, |view, cx| {
                        let command = if view.presentation.inputs_locked() {
                            AppCommand::StopRecording
                        } else {
                            AppCommand::StartRecording
                        };
                        view.handle(command, window, cx);
                    });
                },
                cx,
            )
            .into_any_element(),
            AppTab::Recordings => {
                let selected_row = self.library_selected.and_then(|id| {
                    self.recordings_table
                        .read(cx)
                        .delegate()
                        .rows()
                        .iter()
                        .find(|row| row.id == id)
                        .cloned()
                });
                let playing = self.player_snapshot.phase == PlayerPhase::Playing;
                let error = self.library_catalog.error.map(SharedString::from);
                let selected_id = self.library_selected;
                let status = selected_id
                    .map(|id| self.row_transcript_status(id))
                    .unwrap_or(TranscriptStatus::None);
                let controls = transcript_controls(status, self.job_active_for(selected_id));
                let failure = if status == TranscriptStatus::Failed {
                    let detail = self.presentation.detail_text();
                    if detail.is_empty() {
                        None
                    } else if self.presentation.transcript_recording() == selected_id {
                        Some(SharedString::from(detail.to_string()))
                    } else {
                        None
                    }
                } else {
                    self.transcript_error.map(SharedString::from)
                };
                let transcript_view =
                    if status == TranscriptStatus::Ready && self.transcript_error.is_none() {
                        Editor::new(&self.transcript_editor)
                            .readonly(true)
                            .h(px(220.))
                            .into_any_element()
                    } else {
                        div().into_any_element()
                    };
                let job_id = self
                    .presentation
                    .active_job()
                    .filter(|(recording, _)| Some(*recording) == selected_id)
                    .map(|(_, job)| job)
                    .unwrap_or_else(|| JobId::from_raw(0));
                let refresh_entity = entity.clone();
                let folder_entity = entity.clone();
                let play_entity = entity.clone();
                let stop_entity = entity.clone();
                let rename_entity = entity.clone();
                let delete_entity = entity.clone();
                let transcribe_entity = entity.clone();
                let cancel_entity = entity.clone();
                let retry_entity = entity.clone();
                let model_entity = entity.clone();
                recordings_tab(
                    &self.recordings_table,
                    &self.library_catalog,
                    selected_row.as_ref(),
                    playing,
                    play_button_label(self.player_snapshot.phase),
                    error,
                    move |_event, window, cx| {
                        refresh_entity.update(cx, |view, cx| {
                            view.handle(AppCommand::RefreshLibrary, window, cx);
                        });
                    },
                    move |_event, window, cx| {
                        folder_entity.update(cx, |view, cx| {
                            view.handle(AppCommand::OpenRecordingsFolder, window, cx);
                        });
                    },
                    move |_event, _window, cx| {
                        play_entity.update(cx, |view, cx| {
                            view.play_selected();
                            cx.notify();
                        });
                    },
                    move |_event, _window, cx| {
                        stop_entity.update(cx, |view, cx| {
                            view.player.stop();
                            cx.notify();
                        });
                    },
                    move |_event, window, cx| {
                        rename_entity.update(cx, |view, cx| view.rename_selected(window, cx));
                    },
                    move |_event, window, cx| {
                        delete_entity.update(cx, |view, cx| view.delete_selected(window, cx));
                    },
                    transcript_view,
                    controls,
                    failure,
                    move |_event, window, cx| {
                        let Some(id) = selected_id else {
                            return;
                        };
                        transcribe_entity.update(cx, |view, cx| {
                            view.handle(AppCommand::Transcribe { recording_id: id }, window, cx);
                        });
                    },
                    move |_event, window, cx| {
                        cancel_entity.update(cx, |view, cx| {
                            view.handle(AppCommand::CancelTranscription { job_id }, window, cx);
                        });
                    },
                    move |_event, window, cx| {
                        let Some(id) = selected_id else {
                            return;
                        };
                        retry_entity.update(cx, |view, cx| {
                            view.handle(AppCommand::Transcribe { recording_id: id }, window, cx);
                        });
                    },
                    move |_event, window, cx| {
                        model_entity.update(cx, |view, cx| view.open_model_prompt(window, cx));
                    },
                    cx,
                )
                .into_any_element()
            }
        };
        let status = SharedString::from(self.presentation.status_line(&status_elapsed));
        let model_status = self
            .presentation
            .model_status(self.settings.selected_model());
        let status_entity = entity.clone();
        let model_item = model_status_item(
            &model_status,
            move |_event, window, cx| {
                status_entity.update(cx, |view, cx| {
                    view.handle(AppCommand::OpenModelDialog, window, cx);
                });
            },
            cx,
        );
        let settings_entity = entity.clone();
        let theme_entity = entity.clone();
        let theme = self.settings.theme();
        let theme_label = theme.tooltip();
        v_flex()
            .id("skrivli-shell")
            .track_focus(&self.focus)
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(
                TabBar::new("shell-tabs")
                    .px_2()
                    .selected_index(selected)
                    .child(Tab::new().label("Record"))
                    .child(Tab::new().label("Recordings"))
                    .suffix(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .child(
                                Button::new("cycle-theme")
                                    .ghost()
                                    .icon(theme_icon(theme))
                                    .tooltip(theme_label)
                                    .accessibility_label(theme_label)
                                    .on_click(move |_event, window, cx| {
                                        theme_entity.update(cx, |view, cx| {
                                            view.cycle_theme(window, cx);
                                        });
                                    }),
                            )
                            .child(
                                Button::new("open-settings")
                                    .ghost()
                                    .icon(IconName::Settings)
                                    .accessibility_label("Settings")
                                    .on_click(move |_event, window, cx| {
                                        settings_entity.update(cx, |view, cx| {
                                            view.handle(AppCommand::OpenSettings, window, cx);
                                        });
                                    }),
                            ),
                    )
                    .on_click(move |index, window, cx| {
                        let tab = if *index == 0 {
                            AppTab::Record
                        } else {
                            AppTab::Recordings
                        };
                        entity.update(cx, |view, cx| {
                            view.handle(AppCommand::SelectTab(tab), window, cx);
                        });
                    }),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .p_4()
                    .overflow_hidden()
                    .child(div().size_full().min_h_0().child(body)),
            )
            .child(
                // With only a right region, the center is start-aligned and takes the free
                // space, so long status text truncates before the model item moves.
                StatusBar::new()
                    .h(px(28.))
                    .px_3()
                    .child(div().min_w_0().truncate().child(status))
                    .when_some(model_item, |bar, item| bar.right(item)),
            )
    }
}
