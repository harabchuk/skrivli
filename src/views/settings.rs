//! Settings placeholder. Only close-to-tray is wired; the rest of the form is M8.

use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::WindowExt as _;
use gpui_kit::{App, IntoElement, ParentElement, SharedString, Window};

pub(crate) fn open_settings(
    window: &mut Window,
    cx: &mut App,
    checked: impl Fn(&App) -> bool + 'static,
    on_change: impl Fn(&bool, &mut Window, &mut App) + Clone + 'static,
) {
    window.open_dialog(cx, move |dialog, _, cx| {
        let value = checked(cx);
        let on_change = on_change.clone();
        dialog
            .title("Settings")
            .child(close_to_tray_checkbox(value, on_change))
    });
}

pub(crate) fn close_to_tray_checkbox(
    checked: bool,
    on_change: impl Fn(&bool, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    Checkbox::new("close-to-tray")
        .label("Close to the notification area")
        .checked(checked)
        .on_change(on_change)
}

pub(crate) fn open_about(window: &mut Window, cx: &mut App) {
    let version = SharedString::from(format!("Version {}", env!("CARGO_PKG_VERSION")));
    window.open_alert_dialog(cx, move |alert, _, _| {
        alert.title("Skrivli").description(version.clone())
    });
}

pub(crate) fn open_tray_explanation(window: &mut Window, cx: &mut App) {
    window.open_alert_dialog(cx, |alert, _, _| {
        alert
            .title("Skrivli is still running")
            .description("Skrivli stays in the notification area. Use Exit to quit.")
    });
}

pub(crate) fn open_exit_while_recording(
    window: &mut Window,
    cx: &mut App,
    on_confirm: impl Fn(&mut Window, &mut App) + Clone + 'static,
) {
    window.open_alert_dialog(cx, move |alert, _, _| {
        let on_confirm = on_confirm.clone();
        alert
            .title("Stop and save?")
            .description("A recording is in progress. Exit will stop and save it first.")
            .confirm()
            .ok_text("Stop and exit")
            .on_ok(move |_, window, cx| {
                on_confirm(window, cx);
                true
            })
    });
}
