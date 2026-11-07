//! Values and small pieces every Skrivli dialog shares.

use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{div, px, App, IntoElement, ParentElement, Pixels, SharedString, Styled};

pub(crate) const DIALOG_WIDTH: Pixels = px(520.);

/// Muted line under a dialog title. The dialog body scrolls, so this scrolls with it.
pub(crate) fn dialog_subtitle(text: impl Into<SharedString>, cx: &App) -> impl IntoElement {
    div()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}

/// Footer with a divider above it. Kit keeps the footer outside the scrolling body.
pub(crate) fn dialog_footer(cx: &App) -> DialogFooter {
    DialogFooter::new()
        .w_full()
        .items_center()
        .pt_3()
        .border_t_1()
        .border_color(cx.theme().border)
}

/// Muted footer text that takes the free space and pushes the buttons right.
pub(crate) fn dialog_footer_note(text: impl Into<SharedString>, cx: &App) -> impl IntoElement {
    div()
        .flex_1()
        .min_w_0()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}
