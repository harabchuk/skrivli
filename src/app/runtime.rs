//! Foreground dispatch from GPUI actions onto the shell entity.

use gpui_kit::{AnyWindowHandle, App, Entity, Global, Subscription};

use crate::views::main_window::MainWindow;

use super::actions::AppCommand;

pub(crate) struct ShellGlobal {
    session: Entity<MainWindow>,
    window: AnyWindowHandle,
    _closed: Subscription,
}

impl ShellGlobal {
    pub(crate) fn new(
        session: Entity<MainWindow>,
        window: AnyWindowHandle,
        closed: Subscription,
    ) -> Self {
        Self {
            session,
            window,
            _closed: closed,
        }
    }
}

impl Global for ShellGlobal {}

pub(crate) fn dispatch(command: AppCommand, cx: &mut App) {
    if !cx.has_global::<ShellGlobal>() {
        return;
    }
    let session = cx.global::<ShellGlobal>().session.clone();
    let window = cx.global::<ShellGlobal>().window;
    cx.defer(move |cx| {
        if window
            .update(cx, |_root, window, cx| {
                session.update(cx, |shell, cx| shell.handle(command, window, cx));
            })
            .is_err()
        {
            tracing::debug!("menu command arrived after the window closed");
        }
    });
}
