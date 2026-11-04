//! Resizable shell: menus, Record and Recordings tabs, and the status strip.

use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::component::menu::AppMenuBar;
use gpui_kit::component::select::{SearchableVec, SelectEvent, SelectState};
use gpui_kit::component::slider::{SliderEvent, SliderState};
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::table::{TableEvent, TableSelection, TableState};
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _};
use gpui_kit::prelude::*;
use gpui_kit::{
    px, App, Context, Entity, FocusHandle, IntoElement, ParentElement, Render, SharedString,
    Styled, Subscription, Task, Window,
};

use crate::app::actions::{drain_bridge, AppCommand, BridgeMessage, Effect};
use crate::app::settings_writer::SettingsWriter;
use crate::app::shutdown::ShutdownController;
use crate::app::state::{AppTab, Presentation, RecordPhase};
use crate::domain::audio::{
    format_elapsed, resolve_endpoint, EndpointChoice, EndpointFlow, EndpointId,
};
use crate::domain::gain::{db_fs, format_gain};
use crate::domain::ids::RecordingId;
use crate::domain::window_frame::LogicalRect;
use crate::infrastructure::audio::devices::DeviceCatalog;
use crate::infrastructure::audio::levels::{GainControl, LevelSnapshot};
use crate::infrastructure::files::player::{PlayerHost, PlayerPhase, PlayerSnapshot};
use crate::infrastructure::storage::library::LibraryCatalog;
use crate::infrastructure::storage::settings::{Settings, StoredEndpoint, StoredWindowBounds};
use crate::infrastructure::windows::dialogs::pick_recordings_folder;
use crate::infrastructure::windows::recordings_folder::open_recordings_folder;
use crate::infrastructure::windows::shell_window;
use crate::services::level_monitor::MonitorTargets;
use crate::services::library::{LibraryHost, LibrarySnapshot};
use crate::services::recording::{
    RecordPhase as ServicePhase, RecordSnapshot, RecordingHost, StartRequest, NOTICE_FALLBACK,
    NOTICE_MIC, NOTICE_MIC_MISSING, NOTICE_SYSTEM_FALLBACK, NOTICE_SYSTEM_MISSING,
};

use super::recording::record_tab;
use super::recordings::{
    open_delete_dialog, open_rename_dialog, play_button_label, recordings_tab, rows_from_catalog,
    RecordingsDelegate,
};
use super::settings::{
    open_about, open_exit_while_recording, open_settings, open_tray_explanation,
};

const BOUNDS_DEBOUNCE: Duration = Duration::from_millis(400);

pub(crate) struct MainWindow {
    focus: FocusHandle,
    menu_bar: Entity<AppMenuBar>,
    presentation: Presentation,
    settings: Settings,
    writer: SettingsWriter,
    commands: Receiver<BridgeMessage>,
    shutdown: ShutdownController,
    recording: RecordingHost,
    library: LibraryHost,
    player: PlayerHost,
    library_catalog: LibraryCatalog,
    library_selected: Option<RecordingId>,
    player_snapshot: PlayerSnapshot,
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
}

impl MainWindow {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        presentation: Presentation,
        settings: Settings,
        writer: SettingsWriter,
        commands: Receiver<BridgeMessage>,
        shutdown: ShutdownController,
        recording: RecordingHost,
        library: LibraryHost,
        player: PlayerHost,
        levels: Arc<LevelSnapshot>,
        gain_control: Arc<GainControl>,
        targets: Arc<MonitorTargets>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle().tab_stop(true);
        window.focus(&focus, cx);
        let menu_bar = AppMenuBar::new(cx);
        let recordings_table = cx.new(|cx| {
            TableState::new(RecordingsDelegate::new(), window, cx)
                .row_selectable(true)
                .sortable(true)
        });
        let table_sub = cx.subscribe_in(
            &recordings_table,
            window,
            |this, table, event: &TableEvent, _window, cx| {
                if let TableEvent::SelectRow(row) = event {
                    let id = table.read(cx).delegate().row_id(*row);
                    this.select_library_row(id, cx);
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
                if view.shutdown.is_requested() {
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
        Self {
            focus,
            menu_bar,
            presentation,
            settings,
            writer,
            commands,
            shutdown,
            recording,
            library,
            player,
            library_catalog: LibraryCatalog::empty(),
            library_selected: None,
            player_snapshot: PlayerSnapshot {
                phase: PlayerPhase::Idle,
                recording_id: None,
                notice: None,
            },
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
        }
    }

    pub(crate) fn close_to_tray(&self) -> bool {
        self.settings.close_to_tray()
    }

    pub(crate) fn set_close_to_tray(&mut self, close_to_tray: bool, cx: &mut Context<Self>) {
        self.settings.set_close_to_tray(close_to_tray);
        self.presentation.set_close_to_tray(close_to_tray);
        self.writer.save(self.settings.clone());
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
                self.writer.save(self.settings.clone());
                self.publish_targets();
            }
            cx.notify();
            return;
        }
        if let AppCommand::SelectSystem(choice) = command {
            if !self.presentation.inputs_locked() {
                self.settings.set_system_audio(choice);
                self.writer.save(self.settings.clone());
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
            self.writer.save(self.settings.clone());
            self.shutdown.request();
            true
        }
    }

    fn on_tick(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.shutdown.is_requested() {
            return false;
        }
        let batch = drain_bridge(&self.commands, &mut self.presentation);
        let mut changed = !batch.effects.is_empty();
        if let Some(snapshot) = batch.recording {
            self.apply_snapshot(snapshot, window);
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
        for effect in batch.effects {
            self.perform(effect, window, cx);
            changed = true;
        }
        if self.presentation.pending_show() {
            self.show_window(window);
            changed = true;
        }
        if self.presentation.close_to_tray() && shell_window::main_window_is_iconic() {
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
        !self.shutdown.is_requested()
    }

    fn perform(&mut self, effect: Effect, window: &mut Window, cx: &mut Context<Self>) {
        match effect {
            Effect::None | Effect::Unavailable => {}
            Effect::Show => self.show_window(window),
            Effect::Hide { explain } => {
                self.flush_bounds(window);
                let _ = shell_window::set_main_window_visible(false);
                if explain {
                    self.settings.set_tray_explanation_seen(true);
                    self.writer.save(self.settings.clone());
                    open_tray_explanation(window, cx);
                }
            }
            Effect::OpenFolder => {
                if let Err(error) = open_recordings_folder(self.settings.recordings_directory()) {
                    tracing::error!(error = %error, "recordings folder was not opened");
                    self.presentation
                        .set_notice(Some("Could not open the recordings folder."));
                }
            }
            Effect::OpenSettings => {
                let session = cx.entity();
                let watched = session.clone();
                let on_change = move |checked: &bool, _: &mut Window, cx: &mut App| {
                    session.update(cx, |view, cx| view.set_close_to_tray(*checked, cx));
                };
                let checked = move |cx: &App| watched.read_with(cx, |view, _| view.close_to_tray());
                open_settings(window, cx, checked, on_change);
            }
            Effect::OpenAbout => open_about(window, cx),
            Effect::StartRecording => self.begin_recording(),
            Effect::StopRecording => self.recording.stop(),
            Effect::BrowseRecordings => self.browse_folder(),
            Effect::RefreshLibrary => self.library.refresh(),
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
        }
        cx.notify();
    }

    fn show_window(&mut self, window: &mut Window) {
        if shell_window::set_main_window_visible(true) {
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
        self.writer.save(self.settings.clone());
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
        self.writer.save(self.settings.clone());
        cx.notify();
    }

    fn set_agc(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.gain_control.set_agc_enabled(enabled);
        self.settings.set_agc_enabled(enabled);
        self.writer.save(self.settings.clone());
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

    fn apply_snapshot(&mut self, snapshot: RecordSnapshot, window: &mut Window) {
        let phase = match snapshot.phase {
            ServicePhase::Idle => RecordPhase::Idle,
            ServicePhase::Starting => RecordPhase::Starting,
            ServicePhase::Recording => {
                if self.started.is_none() {
                    self.started = Some(Instant::now());
                }
                RecordPhase::Recording
            }
            ServicePhase::Stopping => RecordPhase::Stopping,
            ServicePhase::Error => RecordPhase::Error,
        };
        let became_idle = matches!(phase, RecordPhase::Idle | RecordPhase::Error)
            && self.presentation.record_phase() != phase;
        if matches!(phase, RecordPhase::Idle | RecordPhase::Error) {
            self.started = None;
        }
        self.presentation.set_record_phase(phase);
        self.presentation.set_notice(snapshot.notice);
        if became_idle && snapshot.id.is_some() {
            self.library.refresh();
        }
        if self.exit_after_stop && matches!(phase, RecordPhase::Idle | RecordPhase::Error) {
            self.exit_after_stop = false;
            self.quit_now(window);
        }
    }

    fn apply_library(
        &mut self,
        snapshot: LibrarySnapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.library_selected != snapshot.selected {
            self.player.stop();
        }
        self.library_catalog = snapshot.catalog;
        self.library_selected = snapshot.selected;
        if let Some(notice) = snapshot.notice {
            self.presentation.set_notice(Some(notice));
        }
        let rows = rows_from_catalog(&self.library_catalog);
        let selected = self.library_selected;
        self.recordings_table.update(cx, |table, cx| {
            table.delegate_mut().set_rows(rows);
            if let Some(id) = selected {
                if let Some(index) = table.delegate().index_of(id) {
                    table.set_selection(TableSelection::Row(index), cx);
                }
            }
            cx.notify();
        });
        let _ = window;
    }

    fn apply_player(&mut self, snapshot: PlayerSnapshot) {
        self.player_snapshot = snapshot;
        if let Some(notice) = self.player_snapshot.notice {
            self.presentation.set_notice(Some(notice));
        }
    }

    fn select_library_row(&mut self, id: Option<RecordingId>, cx: &mut Context<Self>) {
        if self.library_selected != id {
            self.player.stop();
        }
        self.library_selected = id;
        self.library.select(id);
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
            self.writer.save(self.settings.clone());
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
            self.writer.save(self.settings.clone());
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
        let mic = resolve_choice(
            self.settings.microphone(),
            EndpointFlow::Capture,
            &self.catalog.capture,
            self.catalog.default_capture.as_ref(),
        );
        let system = resolve_choice(
            self.settings.system_audio(),
            EndpointFlow::Render,
            &self.catalog.render,
            self.catalog.default_render.as_ref(),
        );
        let (open_microphone, endpoint_id, microphone_id, mut notice) = match mic {
            Ok(resolved) => {
                let id = resolved.id;
                let label = if id.is_empty() {
                    "default".to_string()
                } else {
                    id.clone()
                };
                let notice = resolved.fell_back.then_some(NOTICE_FALLBACK);
                (true, id, label, notice)
            }
            Err(()) => (
                false,
                String::new(),
                "unavailable".to_string(),
                Some(NOTICE_MIC_MISSING),
            ),
        };
        let (system_endpoint_id, system_audio_id) = match system {
            Ok(resolved) => {
                if resolved.fell_back && notice.is_none() {
                    notice = Some(NOTICE_SYSTEM_FALLBACK);
                }
                let label = if resolved.id.is_empty() {
                    "default".to_string()
                } else {
                    resolved.id.clone()
                };
                (Some(resolved.id), label)
            }
            Err(()) => {
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
            self.presentation.set_notice(Some(NOTICE_MIC));
            return;
        }
        self.recording.start(StartRequest {
            directory: self.settings.recordings_directory().to_path_buf(),
            endpoint_id,
            microphone_id,
            system_endpoint_id,
            system_audio_id,
            open_microphone,
            degraded_notice: notice,
        });
    }

    fn browse_folder(&mut self) {
        match pick_recordings_folder() {
            Ok(Some(path)) => {
                if self.settings.set_recordings_directory(path.clone()).is_ok() {
                    self.writer.save(self.settings.clone());
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
        self.writer.save(self.settings.clone());
        self.shutdown.request();
        window.remove_window();
    }
}

fn apply(state: &mut Presentation, command: AppCommand) -> Effect {
    crate::app::actions::apply_command(state, command)
}

struct ResolvedChoice {
    id: String,
    fell_back: bool,
}

fn resolve_choice(
    choice: &StoredEndpoint,
    flow: EndpointFlow,
    devices: &[crate::domain::audio::EndpointDescriptor],
    default_id: Option<&EndpointId>,
) -> Result<ResolvedChoice, ()> {
    let endpoint_choice = match choice {
        StoredEndpoint::Default => {
            return Ok(ResolvedChoice {
                id: default_id
                    .map(|id| id.as_str().to_string())
                    .unwrap_or_default(),
                fell_back: false,
            });
        }
        StoredEndpoint::Explicit(id) => {
            let Some(endpoint) = EndpointId::new(id) else {
                return Err(());
            };
            EndpointChoice::Explicit(endpoint)
        }
    };
    match resolve_endpoint(&endpoint_choice, flow, devices, default_id) {
        Ok(resolved) => {
            tracing::debug!(endpoint = resolved.name(), "endpoint resolved");
            Ok(ResolvedChoice {
                id: resolved.id().as_str().to_string(),
                fell_back: resolved.fell_back(),
            })
        }
        Err(_) => Err(()),
    }
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
        let folder = SharedString::from(self.settings.recordings_directory().display().to_string());
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
        let phase_label = match phase {
            RecordPhase::Idle => "Ready",
            RecordPhase::Starting => "Starting",
            RecordPhase::Recording => "Recording",
            RecordPhase::Stopping => "Saving",
            RecordPhase::Error => "Not recording",
        };
        let notice = SharedString::from(self.presentation.notice().unwrap_or(""));
        let entity = cx.entity();
        let browse_entity = entity.clone();
        let toggle_entity = entity.clone();
        let agc_entity = entity.clone();
        let gain_value = if self.settings.agc_enabled()
            && matches!(phase, RecordPhase::Recording | RecordPhase::Starting)
        {
            self.gain_control.effective()
        } else {
            self.settings.mic_gain()
        };
        let body = match self.presentation.tab() {
            AppTab::Record => record_tab(
                &self.microphone,
                &self.system,
                &self.gain_slider,
                folder,
                locked,
                show_stop,
                phase == RecordPhase::Stopping,
                self.settings.agc_enabled(),
                SharedString::from(format_gain(gain_value)),
                SharedString::from(elapsed),
                SharedString::from(phase_label),
                notice,
                self.mic_meter.level,
                self.mic_meter.hold,
                self.system_meter.level,
                self.system_meter.hold,
                move |_event, window, cx| {
                    browse_entity.update(cx, |view, cx| {
                        view.handle(AppCommand::BrowseRecordings, window, cx);
                    });
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
                move |checked, _window, cx| {
                    agc_entity.update(cx, |view, cx| view.set_agc(*checked, cx));
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
                let refresh_entity = entity.clone();
                let folder_entity = entity.clone();
                let play_entity = entity.clone();
                let stop_entity = entity.clone();
                let rename_entity = entity.clone();
                let delete_entity = entity.clone();
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
                    cx,
                )
                .into_any_element()
            }
        };
        let status = SharedString::from(self.presentation.status_line());
        v_flex()
            .id("skrivli-shell")
            .track_focus(&self.focus)
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(
                h_flex()
                    .h(px(32.))
                    .px_2()
                    .items_center()
                    .child(self.menu_bar.clone()),
            )
            .child(
                TabBar::new("shell-tabs")
                    .px_2()
                    .selected_index(selected)
                    .child(Tab::new().label("Record"))
                    .child(Tab::new().label("Recordings"))
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
            .child(v_flex().flex_1().p_4().overflow_hidden().child(body))
            .child(
                h_flex()
                    .h(px(28.))
                    .px_3()
                    .items_center()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(status),
            )
    }
}
