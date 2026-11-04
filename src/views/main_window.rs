//! Resizable shell: menus, Record and Recordings tabs, and the status strip.

use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use gpui_kit::component::menu::AppMenuBar;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _};
use gpui_kit::prelude::*;
use gpui_kit::{
    px, App, Context, Entity, FocusHandle, IntoElement, ParentElement, Render, SharedString,
    Styled, Task, Window,
};

use crate::app::actions::{drain_bridge, AppCommand, BridgeMessage, Effect};
use crate::app::settings_writer::SettingsWriter;
use crate::app::shutdown::ShutdownController;
use crate::app::state::{AppTab, Presentation};
use crate::domain::window_frame::LogicalRect;
use crate::infrastructure::storage::settings::{Settings, StoredWindowBounds};
use crate::infrastructure::windows::recordings_folder::open_recordings_folder;
use crate::infrastructure::windows::shell_window;

use super::recording::recording_placeholder;
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
    last_saved: Option<LogicalRect>,
    pending_bounds: Option<(LogicalRect, Instant)>,
    _bridge: Task<()>,
}

impl MainWindow {
    pub(crate) fn new(
        presentation: Presentation,
        settings: Settings,
        writer: SettingsWriter,
        commands: Receiver<BridgeMessage>,
        shutdown: ShutdownController,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle().tab_stop(true);
        window.focus(&focus, cx);
        let menu_bar = AppMenuBar::new(cx);
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
        let effects = drain_bridge(&self.commands, &mut self.presentation);
        let mut changed = !effects.is_empty();
        for effect in effects {
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
            Effect::Quit => {
                self.flush_bounds(window);
                self.writer.save(self.settings.clone());
                self.shutdown.request();
                window.remove_window();
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
        let entity = cx.entity();
        let folder = self.settings.recordings_directory().display().to_string();
        let body = match self.presentation.tab() {
            AppTab::Record => recording_placeholder(&folder).into_any_element(),
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
