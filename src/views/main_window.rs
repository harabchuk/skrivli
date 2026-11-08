//! Resizable shell: Record and Recordings tabs, settings gear, and the status strip.

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::base::{v_virtual_list, VirtualListScrollHandle};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::slider::{SliderEvent, SliderState};
use gpui_kit::component::status_bar::StatusBar;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, IconName, WindowExt as _};
use gpui_kit::prelude::*;
use gpui_kit::Pixels;
use gpui_kit::{
    div, list, px, size, App, ClickEvent, Context, Entity, FocusHandle, IntoElement, KeyDownEvent,
    ListAlignment, ListOffset, ListState as ParagraphListState, ParentElement, Render,
    SharedString, Size, Styled, Subscription, Task, Window,
};

use crate::domain::audio::format_elapsed;
use crate::domain::gain::{db_fs, format_gain};
use crate::domain::ids::RecordingId;
use crate::domain::recording::{
    settled_transcript_status, MaxMeetingDuration, RecordPhase, RecordingContainer,
    TranscriptStatus,
};
use crate::domain::theme::ThemePreference;
use crate::domain::transcript::{current_paragraph, parse_transcript, TranscriptParagraph};
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
use super::recording::{banner_notice, record_tab, DeviceChoice, RecordView};
use super::recordings::{
    can_transcribe_again, default_selection, failure_reason, is_narrow, item_index_of,
    library_header, library_items, may_auto_scroll, open_delete_dialog, open_rename_dialog,
    open_transcribe_again_dialog, play_action, recording_list_row, recordings_tab,
    rows_from_catalog, seek_hold_active, seek_position_ms, seek_value, set_row_status,
    shortcut_for, shown_position_ms, step_selection, transcript_pane, ClickAction, LibraryItem,
    PlayAction, PlayerCard, RecordingRow, RecordingsActions, RecordingsView, Shortcut,
    ShortcutContext, TranscriptPane, SEEK_STEPS,
};
use super::settings::{
    open_about, open_exit_while_recording, open_settings, open_tray_explanation,
    settings_model_row, SettingsHandlers, SettingsView,
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
    transcript_paragraphs: Rc<Vec<TranscriptParagraph>>,
    transcript_list: ParagraphListState,
    /// When the user last scrolled the transcript by hand.
    transcript_scrolled_at: Rc<Cell<Option<Instant>>>,
    /// The paragraph last highlighted, so auto-scroll runs only when it changes.
    current_paragraph: Option<usize>,
    transcript_focus: FocusHandle,
    transcript_generation: u64,
    transcript_error: Option<&'static str>,
    transcript_load: Option<Task<()>>,
    prompt_model_id: String,
    model_prompt_open: bool,
    /// Set only when Transcribe found no model. Starts once a model is ready.
    pending_transcription: Option<RecordingId>,
    library_rows: Vec<RecordingRow>,
    library_items: Vec<LibraryItem>,
    library_item_sizes: Rc<Vec<Size<Pixels>>>,
    library_scroll: VirtualListScrollHandle,
    levels: Arc<LevelSnapshot>,
    gain_control: Arc<GainControl>,
    targets: Arc<MonitorTargets>,
    catalog: DeviceCatalog,
    mic_labels: Vec<SharedString>,
    mic_choices: Vec<StoredEndpoint>,
    system_labels: Vec<SharedString>,
    system_choices: Vec<StoredEndpoint>,
    gain_slider: Entity<SliderState>,
    _gain_sub: Subscription,
    seek_slider: Entity<SliderState>,
    _seek_sub: Subscription,
    /// Position under the thumb while the user drags it. Playback updates are ignored.
    seek_drag: Option<u64>,
    /// The released seek target, shown until the player position catches up.
    seek_hold: Option<(u64, Instant)>,
    /// Where Play starts when nothing is playing: the slider position, 0 by default.
    idle_position_ms: u64,
    /// The value last written to the seek slider.
    seek_slider_value: f32,
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
        let labels = vec![SharedString::from("Default communications")];
        let choices = vec![StoredEndpoint::Default];
        let system_labels = vec![SharedString::from("Default output device")];
        let system_choices = vec![StoredEndpoint::Default];
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
        let seek_slider = cx.new(|_| {
            SliderState::new()
                .min(0.0)
                .max(SEEK_STEPS)
                .step(1.0)
                .default_value(0.0)
        });
        let seek_sub = cx.subscribe_in(
            &seek_slider,
            window,
            |this, _slider, event: &SliderEvent, _window, cx| match event {
                SliderEvent::Change(value) => {
                    this.seek_drag =
                        Some(seek_position_ms(value.start(), this.selected_duration_ms()));
                    cx.notify();
                }
                SliderEvent::Release(value) => {
                    let position_ms = seek_position_ms(value.start(), this.selected_duration_ms());
                    this.seek_drag = None;
                    if this.snapshot_is_selected() {
                        this.seek_hold = Some((position_ms, Instant::now()));
                        this.player.seek(position_ms);
                    } else {
                        this.idle_position_ms = position_ms;
                    }
                    cx.notify();
                }
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
        let transcript_list = ParagraphListState::new(0, ListAlignment::Top, px(400.));
        let transcript_scrolled_at = Rc::new(Cell::new(None));
        let scrolled_at = Rc::clone(&transcript_scrolled_at);
        transcript_list.set_scroll_handler(move |_event, _window, _cx| {
            scrolled_at.set(Some(Instant::now()));
        });
        let transcript_focus = cx.focus_handle().tab_stop(true);
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
            player_snapshot: PlayerSnapshot::idle(),
            transcription_snapshot: TranscriptionSnapshot::idle(),
            transcript_paragraphs: Rc::new(Vec::new()),
            transcript_list,
            transcript_scrolled_at,
            current_paragraph: None,
            transcript_focus,
            transcript_generation: 0,
            transcript_error: None,
            transcript_load: None,
            prompt_model_id,
            model_prompt_open: false,
            pending_transcription: None,
            library_rows: Vec::new(),
            library_items: Vec::new(),
            library_item_sizes: Rc::new(Vec::new()),
            library_scroll: VirtualListScrollHandle::new(),
            levels,
            gain_control,
            targets,
            catalog: DeviceCatalog::empty(),
            mic_labels: labels,
            mic_choices: choices,
            system_labels,
            system_choices,
            gain_slider,
            _gain_sub: gain_sub,
            seek_slider,
            _seek_sub: seek_sub,
            seek_drag: None,
            seek_hold: None,
            idle_position_ms: 0,
            seek_slider_value: 0.,
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

    pub(crate) fn set_max_meeting_duration(
        &mut self,
        duration: MaxMeetingDuration,
        cx: &mut Context<Self>,
    ) {
        self.settings.set_max_meeting_duration(duration);
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
        let opens_tab = matches!(command, AppCommand::SelectTab(_));
        let effect = apply(&mut self.presentation, command);
        self.perform(effect, window, cx);
        if opens_tab {
            self.ensure_selection(window, cx);
        }
    }

    /// Select the newest recording when the Recordings tab is open with nothing selected.
    fn ensure_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.presentation.tab() != AppTab::Recordings {
            return;
        }
        let chosen = default_selection(self.library_selected, &self.library_rows);
        if chosen != self.library_selected {
            self.select_library_row(chosen, window, cx);
        }
    }

    fn on_shortcut(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let page_focused =
            self.focus.is_focused(window) || self.transcript_focus.is_focused(window);
        let context = ShortcutContext {
            on_recordings_tab: self.presentation.tab() == AppTab::Recordings,
            dialog_open: window.has_active_dialog(cx),
            page_focused,
        };
        let Some(shortcut) = shortcut_for(key, context) else {
            return false;
        };
        match shortcut {
            Shortcut::Previous => self.step_library(-1, window, cx),
            Shortcut::Next => self.step_library(1, window, cx),
            Shortcut::PlayPause => {
                self.play_selected();
                cx.notify();
            }
            Shortcut::Rename => self.rename_selected(window, cx),
            Shortcut::Delete => self.delete_selected(window, cx),
        }
        true
    }

    fn step_library(&mut self, step: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = step_selection(&self.library_rows, self.library_selected, step) else {
            return;
        };
        if self.library_selected != Some(id) {
            self.select_library_row(Some(id), window, cx);
        }
        if let Some(index) = item_index_of(&self.library_items, id) {
            self.library_scroll
                .scroll_to_item(index, gpui_kit::ScrollStrategy::Nearest);
        }
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
            self.apply_devices(catalog);
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
                let duration_entity = session.clone();
                let on_max_duration =
                    move |duration: MaxMeetingDuration, _: &mut Window, cx: &mut App| {
                        duration_entity
                            .update(cx, |view, cx| view.set_max_meeting_duration(duration, cx));
                    };
                let on_change = move |checked: &bool, _: &mut Window, cx: &mut App| {
                    session.update(cx, |view, cx| view.set_close_to_tray(*checked, cx));
                };
                let on_change_model = move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                    model_entity.update(cx, |view, cx| {
                        view.handle(AppCommand::OpenModelDialog, window, cx);
                    });
                };
                let read = move |cx: &App| watched.read_with(cx, |view, _| view.settings_view());
                open_settings(
                    window,
                    cx,
                    read,
                    SettingsHandlers {
                        on_close_to_tray: on_change,
                        on_max_duration,
                        on_change_model,
                    },
                );
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

    fn choose_microphone(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(choice) = self.mic_choices.get(index).cloned() else {
            return;
        };
        self.handle(AppCommand::SelectMicrophone(choice), window, cx);
    }

    fn choose_system(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
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
        let has_transcript_file = snapshot.recording_id.is_some_and(|id| {
            self.library_catalog
                .find(id)
                .is_some_and(|entry| entry.transcript_path.is_some())
        });
        self.presentation
            .apply_transcription(snapshot.clone(), has_transcript_file);
        self.presentation.set_tab(tab);
        if let Some(id) = snapshot.recording_id {
            let status = settled_transcript_status(
                snapshot.transcript_status(),
                snapshot.phase == JobPhase::Cancelled,
                has_transcript_file,
            );
            if set_row_status(&mut self.library_rows, id, status) {
                self.library_items = library_items(&self.library_rows);
                cx.notify();
            }
        }
        if matches!(
            snapshot.phase,
            JobPhase::Done | JobPhase::Failed | JobPhase::Cancelled | JobPhase::Queued
        ) {
            self.library.refresh();
        }
        if matches!(
            snapshot.phase,
            JobPhase::Done | JobPhase::Failed | JobPhase::Cancelled
        ) && snapshot.recording_id == self.library_selected
        {
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
            self.clear_transcript();
            return;
        };
        if self.row_transcript_status(id) != TranscriptStatus::Ready {
            self.clear_transcript();
            return;
        }
        let Some(entry) = self.library_catalog.find(id) else {
            self.clear_transcript();
            return;
        };
        let path = transcript_path_for(&entry.audio_path);
        self.transcript_generation = self.transcript_generation.saturating_add(1);
        let generation = self.transcript_generation;
        self.transcript_load = Some(cx.spawn_in(window, async move |this, async_cx| {
            let loaded = async_cx
                .background_spawn(async move {
                    read_transcript_file(path).map(|text| {
                        let paragraphs = parse_transcript(&text);
                        (text, paragraphs)
                    })
                })
                .await;
            let _ = this.update_in(async_cx, |view, _window, cx| {
                if view.transcript_generation != generation || view.library_selected != Some(id) {
                    return;
                }
                match loaded {
                    Ok((text, paragraphs)) => {
                        view.transcript_error = None;
                        view.set_transcript(text, paragraphs);
                    }
                    Err(message) => {
                        view.transcript_error = Some(message);
                        view.set_transcript(String::new(), Vec::new());
                    }
                }
                cx.notify();
            });
        }));
    }

    /// Store loaded text. The list resets only when the text changed, so a reload after
    /// a progress update keeps the scroll position.
    fn set_transcript(&mut self, text: String, paragraphs: Vec<TranscriptParagraph>) {
        if self.presentation.loaded_transcript() == text {
            return;
        }
        self.presentation.set_loaded_transcript(text);
        self.transcript_list.reset(paragraphs.len());
        self.current_paragraph = None;
        self.transcript_paragraphs = Rc::new(paragraphs);
    }

    /// The scrolling paragraph list. GPUI's `list` measures every item, so paragraphs of
    /// different lengths wrap without overlapping.
    fn transcript_list_element(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let paragraphs = self.transcript_paragraphs.clone();
        let current = self.current_paragraph;
        let state = self.transcript_list.clone();
        let key_state = state.clone();
        let focus = self.transcript_focus.clone();
        div()
            .id("transcript-list")
            .track_focus(&self.transcript_focus)
            .size_full()
            .min_h_0()
            .on_mouse_down(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, _event, window, cx| {
                    window.focus(&this.transcript_focus, cx);
                }),
            )
            .on_key_down(move |event: &KeyDownEvent, window, cx| {
                if !focus.is_focused(window) {
                    return;
                }
                if scroll_transcript(&key_state, event.keystroke.key.as_str()) {
                    window.refresh();
                    cx.stop_propagation();
                }
            })
            .child(
                list(state, move |index, _window, cx| {
                    let Some(paragraph) = paragraphs.get(index) else {
                        return div().into_any_element();
                    };
                    h_flex()
                        .items_start()
                        .gap_3()
                        .px_2()
                        .py_1()
                        .rounded(px(8.))
                        .when(current == Some(index), |row| row.bg(cx.theme().accent))
                        .child(
                            div()
                                .flex_none()
                                .w(px(64.))
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .children(paragraph.timestamp_label()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(SharedString::from(paragraph.text().to_string())),
                        )
                        .into_any_element()
                })
                .size_full(),
            )
            .into_any_element()
    }

    fn clear_transcript(&mut self) {
        self.transcript_generation = self.transcript_generation.saturating_add(1);
        self.transcript_error = None;
        self.set_transcript(String::new(), Vec::new());
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
            max_duration: self.settings.max_meeting_duration(),
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
                        meta: SharedString::from(row_meta(entry.bytes())),
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
            self.reset_player_position();
        }
        self.library_catalog = snapshot.catalog;
        self.library_selected = snapshot.selected;
        self.presentation.set_selected_recording(snapshot.selected);
        if let Some(notice) = snapshot.notice {
            self.presentation.set_notice(Some(notice));
        }
        self.library_rows = rows_from_catalog(&self.library_catalog);
        if let Some((id, status)) = self
            .presentation
            .transcript_recording()
            .zip(self.presentation.transcript_status())
        {
            let _ = set_row_status(&mut self.library_rows, id, status);
        }
        self.library_items = library_items(&self.library_rows);
        self.library_item_sizes = Rc::new(
            self.library_items
                .iter()
                .map(|item| size(px(1.), px(item.height_px())))
                .collect(),
        );
        cx.notify();
        if selection_changed {
            self.load_selected_transcript(window, cx);
        }
        self.ensure_selection(window, cx);
    }

    fn apply_player(&mut self, snapshot: PlayerSnapshot) {
        self.player_snapshot = snapshot;
        if let Some(notice) = self.player_snapshot.notice {
            self.presentation.set_notice(Some(notice));
        }
        if self.player_snapshot.phase == PlayerPhase::Idle {
            self.idle_position_ms = 0;
        }
        self.follow_playback();
    }

    fn reset_player_position(&mut self) {
        self.idle_position_ms = 0;
        self.seek_drag = None;
        self.seek_hold = None;
        self.current_paragraph = None;
    }

    /// Whether the player is playing or paused on the selected recording.
    fn snapshot_is_selected(&self) -> bool {
        matches!(
            self.player_snapshot.phase,
            PlayerPhase::Playing | PlayerPhase::Paused
        ) && self.player_snapshot.recording_id.is_some()
            && self.player_snapshot.recording_id == self.library_selected
    }

    fn selected_duration_ms(&self) -> Option<u64> {
        let catalog = || {
            self.library_selected
                .and_then(|id| self.library_catalog.find(id))
                .and_then(|entry| entry.info.duration_ms())
        };
        if self.snapshot_is_selected() {
            self.player_snapshot.duration_ms.or_else(catalog)
        } else {
            catalog()
        }
    }

    /// Highlight the paragraph being played. Scroll to it only when it changes, and not
    /// soon after the user scrolled the list by hand.
    fn follow_playback(&mut self) {
        let current = if self.snapshot_is_selected() {
            current_paragraph(
                &self.transcript_paragraphs,
                self.player_snapshot.position_ms,
            )
        } else {
            None
        };
        if current == self.current_paragraph {
            return;
        }
        self.current_paragraph = current;
        let since_scroll = self.transcript_scrolled_at.get().map(|at| at.elapsed());
        if let Some(index) = current.filter(|_| may_auto_scroll(since_scroll)) {
            self.transcript_list.scroll_to_reveal_item(index);
        }
    }

    /// Keep the seek slider on the shown position, unless the user is dragging it.
    fn sync_seek_slider(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.seek_drag.is_some() {
            return;
        }
        let position_ms = self.shown_position_ms();
        let value = seek_value(position_ms, self.selected_duration_ms());
        if (value - self.seek_slider_value).abs() < f32::EPSILON {
            return;
        }
        self.seek_slider_value = value;
        self.seek_slider
            .update(cx, |slider, cx| slider.set_value(value, window, cx));
    }

    fn shown_position_ms(&self) -> u64 {
        let hold = self.seek_hold.and_then(|(target, at)| {
            seek_hold_active(target, self.player_snapshot.position_ms, at.elapsed())
                .then_some(target)
        });
        shown_position_ms(
            self.snapshot_is_selected(),
            self.player_snapshot.position_ms,
            self.idle_position_ms,
            self.seek_drag.or(hold),
        )
    }

    fn select_library_row(
        &mut self,
        id: Option<RecordingId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.library_selected != id {
            self.player.stop();
            self.reset_player_position();
            self.clear_transcript();
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
        if entry.info.container() != RecordingContainer::Flac {
            return;
        }
        match play_action(
            self.player_snapshot.phase,
            self.snapshot_is_selected(),
            self.idle_position_ms,
        ) {
            PlayAction::Pause => self.player.pause(),
            PlayAction::Resume => self.player.resume(),
            PlayAction::Play { start_ms } => {
                self.player.play(id, entry.audio_path.clone(), start_ms);
            }
        }
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
                // The open FLAC file would make the rename fail on Windows.
                view.player.stop();
                view.library.rename(id, name);
            });
        });
    }

    fn transcribe_again_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.library_selected else {
            return;
        };
        let status = self.row_transcript_status(id);
        if !can_transcribe_again(status, self.job_active_for(Some(id))) {
            return;
        }
        let model_name = SharedString::from(model_copy(self.settings.selected_model()).name);
        let entity = cx.entity();
        open_transcribe_again_dialog(window, cx, model_name, move |window, cx| {
            entity.update(cx, |view, cx| {
                view.handle(AppCommand::Transcribe { recording_id: id }, window, cx);
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

    fn apply_devices(&mut self, catalog: DeviceCatalog) {
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
        if !choices.contains(self.settings.microphone()) {
            self.presentation.set_notice(Some(NOTICE_FALLBACK));
            self.settings.set_microphone(StoredEndpoint::Default);
            self.hooks.save_settings(self.settings.clone());
        }
        self.mic_labels = labels;
        self.mic_choices = choices;
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
        if !system_choices.contains(self.settings.system_audio()) {
            self.presentation.set_notice(Some(NOTICE_SYSTEM_FALLBACK));
            self.settings.set_system_audio(StoredEndpoint::Default);
            self.hooks.save_settings(self.settings.clone());
        }
        self.system_labels = system_labels;
        self.system_choices = system_choices;
        self.publish_targets();
    }

    fn begin_recording(&mut self) {
        match build_start_request(
            self.settings.recordings_directory().to_path_buf(),
            self.settings.microphone(),
            self.settings.system_audio(),
            &self.catalog,
            self.settings.max_meeting_duration(),
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

/// PageUp, PageDown, Home and End for the transcript list. Returns true when handled.
fn scroll_transcript(state: &ParagraphListState, key: &str) -> bool {
    match key {
        "pageup" => state.scroll_by(-state.viewport_bounds().size.height),
        "pagedown" => state.scroll_by(state.viewport_bounds().size.height),
        "home" => state.scroll_to(ListOffset {
            item_ix: 0,
            offset_in_item: px(0.),
        }),
        "end" => state.scroll_to_end(),
        _ => return false,
    }
    true
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_seek_slider(window, cx);
        let narrow = is_narrow(f32::from(window.viewport_size().width));
        let selected = match self.presentation.tab() {
            AppTab::Record => 0,
            AppTab::Recordings => 1,
        };
        let locked = self.presentation.inputs_locked();
        let phase = self.presentation.record_phase();
        let elapsed = format_elapsed(
            self.started
                .map(|started| started.elapsed())
                .unwrap_or(Duration::ZERO),
        );
        let status_elapsed = elapsed.clone();
        let entity = cx.entity();
        let toggle_entity = entity.clone();
        let mic_entity = entity.clone();
        let system_entity = entity.clone();
        let body = match self.presentation.tab() {
            AppTab::Record => record_tab(
                RecordView {
                    microphone: DeviceChoice::new(
                        &self.mic_labels,
                        &self.mic_choices,
                        self.settings.microphone(),
                        Rc::new(move |index, window, cx| {
                            mic_entity
                                .update(cx, |view, cx| view.choose_microphone(index, window, cx));
                        }),
                    ),
                    system: DeviceChoice::new(
                        &self.system_labels,
                        &self.system_choices,
                        self.settings.system_audio(),
                        Rc::new(move |index, window, cx| {
                            system_entity
                                .update(cx, |view, cx| view.choose_system(index, window, cx));
                        }),
                    ),
                    gain: &self.gain_slider,
                    gain_label: SharedString::from(format_gain(self.settings.mic_gain())),
                    locked,
                    phase,
                    elapsed: SharedString::from(elapsed),
                    notice: banner_notice(self.presentation.notice()),
                    mic_db: self.mic_meter.level,
                    mic_hold: self.mic_meter.hold,
                    system_db: self.system_meter.level,
                    system_hold: self.system_meter.hold,
                },
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
                let selected_row = self
                    .library_selected
                    .and_then(|id| self.library_rows.iter().find(|row| row.id == id).cloned());
                let library_list = (!self.library_items.is_empty()).then(|| {
                    v_virtual_list(
                        entity.clone(),
                        "recordings-list",
                        self.library_item_sizes.clone(),
                        |this, range, _window, cx| {
                            range
                                .filter_map(|index| this.library_items.get(index).cloned())
                                .map(|item| match item {
                                    LibraryItem::Header(day) => {
                                        library_header(day, cx).into_any_element()
                                    }
                                    LibraryItem::Recording(row) => {
                                        let id = row.id;
                                        let selected = this.library_selected == Some(id);
                                        recording_list_row(
                                            &row,
                                            selected,
                                            cx.listener(move |this, _event, window, cx| {
                                                // Keys go to the page, not to a button.
                                                window.focus(&this.focus, cx);
                                                this.select_library_row(Some(id), window, cx);
                                            }),
                                            cx,
                                        )
                                        .into_any_element()
                                    }
                                })
                                .collect()
                        },
                    )
                    .track_scroll(&self.library_scroll)
                    .into_any_element()
                });
                let playing = self.snapshot_is_selected()
                    && self.player_snapshot.phase == PlayerPhase::Playing;
                let error = self.library_catalog.error.map(SharedString::from);
                let selected_id = self.library_selected;
                let status = selected_id
                    .map(|id| self.row_transcript_status(id))
                    .unwrap_or(TranscriptStatus::None);
                let job_active = self.job_active_for(selected_id);
                let failure = failure_reason(
                    self.presentation.detail_text(),
                    self.presentation.transcript_recording(),
                    selected_id,
                );
                let pane = transcript_pane(
                    status,
                    job_active,
                    failure,
                    self.transcript_error.map(SharedString::from),
                );
                let can_again = selected_id.is_some() && can_transcribe_again(status, job_active);
                let transcript_list =
                    (pane == TranscriptPane::Ready).then(|| self.transcript_list_element(cx));
                let job_id = self
                    .presentation
                    .active_job()
                    .filter(|(recording, _)| Some(*recording) == selected_id)
                    .map(|(_, job)| job);
                let copy_entity = entity.clone();
                let refresh_entity = entity.clone();
                let folder_entity = entity.clone();
                let play_entity = entity.clone();
                let rename_entity = entity.clone();
                let delete_entity = entity.clone();
                let transcribe_entity = entity.clone();
                let again_entity = entity.clone();
                let cancel_entity = entity.clone();
                let retry_entity = entity.clone();
                let record_entity = entity.clone();
                let actions = RecordingsActions {
                    go_record: Rc::new(move |_event, window, cx| {
                        record_entity.update(cx, |view, cx| {
                            view.handle(AppCommand::SelectTab(AppTab::Record), window, cx);
                        });
                    }),
                    refresh: Rc::new(move |_event, window, cx| {
                        refresh_entity.update(cx, |view, cx| {
                            view.handle(AppCommand::RefreshLibrary, window, cx);
                        });
                    }),
                    open_folder: Rc::new(move |_event, window, cx| {
                        folder_entity.update(cx, |view, cx| {
                            view.handle(AppCommand::OpenRecordingsFolder, window, cx);
                        });
                    }),
                    play: Rc::new(move |_event, _window, cx| {
                        play_entity.update(cx, |view, cx| {
                            view.play_selected();
                            cx.notify();
                        });
                    }),
                    rename: Rc::new(move |_event, window, cx| {
                        rename_entity.update(cx, |view, cx| view.rename_selected(window, cx));
                    }),
                    delete: Rc::new(move |_event, window, cx| {
                        delete_entity.update(cx, |view, cx| view.delete_selected(window, cx));
                    }),
                    transcribe: Rc::new(move |_event, window, cx| {
                        let Some(id) = selected_id else {
                            return;
                        };
                        transcribe_entity.update(cx, |view, cx| {
                            view.handle(AppCommand::Transcribe { recording_id: id }, window, cx);
                        });
                    }),
                    transcribe_again: Rc::new(move |_event, window, cx| {
                        again_entity
                            .update(cx, |view, cx| view.transcribe_again_selected(window, cx));
                    }),
                    cancel_job: job_id.map(|job_id| -> ClickAction {
                        Rc::new(move |_event, window, cx| {
                            cancel_entity.update(cx, |view, cx| {
                                view.handle(AppCommand::CancelTranscription { job_id }, window, cx);
                            });
                        })
                    }),
                    retry: Rc::new(move |_event, window, cx| {
                        let Some(id) = selected_id else {
                            return;
                        };
                        retry_entity.update(cx, |view, cx| {
                            view.handle(AppCommand::Transcribe { recording_id: id }, window, cx);
                        });
                    }),
                    copy_transcript: Rc::new(move |_window, cx| {
                        SharedString::from(
                            copy_entity
                                .read(cx)
                                .presentation
                                .loaded_transcript()
                                .to_string(),
                        )
                    }),
                };
                recordings_tab(
                    RecordingsView {
                        list: library_list,
                        count: self.library_rows.len(),
                        selected: selected_row.as_ref(),
                        player: PlayerCard {
                            playing,
                            position_ms: self.shown_position_ms(),
                            duration_ms: self.selected_duration_ms(),
                            seek: &self.seek_slider,
                        },
                        error,
                        pane,
                        can_transcribe_again: can_again,
                        transcript_list,
                        narrow,
                    },
                    actions,
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
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.modifiers.modified() {
                    return;
                }
                if this.on_shortcut(event.keystroke.key.as_str(), window, cx) {
                    cx.stop_propagation();
                }
            }))
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
