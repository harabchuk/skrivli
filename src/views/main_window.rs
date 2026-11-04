//! Resizable shell: menus, Record and Recordings tabs, and the status strip.

use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use gpui_kit::component::menu::AppMenuBar;
use gpui_kit::component::select::{SearchableVec, SelectEvent, SelectState};
use gpui_kit::component::tab::{Tab, TabBar};
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
use crate::domain::window_frame::LogicalRect;
use crate::infrastructure::audio::devices::DeviceCatalog;
use crate::infrastructure::storage::settings::{Settings, StoredEndpoint, StoredWindowBounds};
use crate::infrastructure::windows::dialogs::pick_recordings_folder;
use crate::infrastructure::windows::recordings_folder::open_recordings_folder;
use crate::infrastructure::windows::shell_window;
use crate::services::recording::{
    RecordPhase as ServicePhase, RecordSnapshot, RecordingHost, StartRequest, NOTICE_FALLBACK,
    NOTICE_MIC,
};

use super::recording::record_tab;
use super::recordings::recordings_placeholder;
use super::settings::{open_about, open_settings, open_tray_explanation};

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
    catalog: DeviceCatalog,
    microphone: Entity<SelectState<SearchableVec<SharedString>>>,
    mic_labels: Vec<SharedString>,
    mic_choices: Vec<StoredEndpoint>,
    _mic_sub: Subscription,
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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle().tab_stop(true);
        window.focus(&focus, cx);
        let menu_bar = AppMenuBar::new(cx);
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
        let executor = cx.background_executor().clone();
        let bridge = cx.spawn_in(window, async move |this, async_cx| loop {
            executor.timer(Duration::from_millis(150)).await;
            let step = this.update_in(async_cx, |view, window, cx| view.on_tick(window, cx));
            if !matches!(step, Ok(true)) {
                break;
            }
        });
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
            catalog: DeviceCatalog::empty(),
            microphone,
            mic_labels: labels,
            mic_choices: choices,
            _mic_sub: mic_sub,
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
            Effect::Quit => {
                if self.presentation.inputs_locked() {
                    self.exit_after_stop = true;
                    self.recording.stop();
                } else {
                    self.quit_now(window);
                }
            }
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
        if matches!(phase, RecordPhase::Idle | RecordPhase::Error) {
            self.started = None;
        }
        self.presentation.set_record_phase(phase);
        self.presentation.set_notice(snapshot.notice);
        if self.exit_after_stop && matches!(phase, RecordPhase::Idle | RecordPhase::Error) {
            self.exit_after_stop = false;
            self.quit_now(window);
        }
    }

    fn apply_devices(
        &mut self,
        catalog: DeviceCatalog,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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

    fn begin_recording(&mut self) {
        let endpoint_id = match self.settings.microphone().clone() {
            StoredEndpoint::Default => self
                .catalog
                .default_capture
                .as_ref()
                .map(|id| id.as_str().to_string())
                .unwrap_or_default(),
            StoredEndpoint::Explicit(id) => {
                let Some(endpoint) = EndpointId::new(id) else {
                    self.presentation.set_notice(Some(NOTICE_MIC));
                    return;
                };
                match resolve_endpoint(
                    &EndpointChoice::Explicit(endpoint),
                    EndpointFlow::Capture,
                    &self.catalog.capture,
                    self.catalog.default_capture.as_ref(),
                ) {
                    Ok(resolved) => {
                        if resolved.fell_back() {
                            self.presentation.set_notice(Some(NOTICE_FALLBACK));
                        }
                        tracing::debug!(microphone = resolved.name(), "microphone resolved");
                        resolved.id().as_str().to_string()
                    }
                    Err(_) => {
                        self.presentation.set_notice(Some(NOTICE_MIC));
                        return;
                    }
                }
            }
        };
        let microphone_id = if endpoint_id.is_empty() {
            "default".to_string()
        } else {
            endpoint_id.clone()
        };
        self.recording.start(StartRequest {
            directory: self.settings.recordings_directory().to_path_buf(),
            endpoint_id,
            microphone_id,
        });
    }

    fn browse_folder(&mut self) {
        match pick_recordings_folder() {
            Ok(Some(path)) => {
                if self.settings.set_recordings_directory(path).is_ok() {
                    self.writer.save(self.settings.clone());
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
        let body = match self.presentation.tab() {
            AppTab::Record => record_tab(
                &self.microphone,
                folder,
                locked,
                show_stop,
                phase == RecordPhase::Stopping,
                SharedString::from(elapsed),
                SharedString::from(phase_label),
                notice,
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
            )
            .into_any_element(),
            AppTab::Recordings => recordings_placeholder().into_any_element(),
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
