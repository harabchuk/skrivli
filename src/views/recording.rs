//! Record tab placeholder. Device enumeration and capture arrive in M2.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{v_flex, Disableable as _};
use gpui_kit::{IntoElement, ParentElement, Styled};

use crate::app::actions::NOT_AVAILABLE;

pub(crate) fn recording_placeholder(folder: &str) -> impl IntoElement {
    v_flex()
        .gap_3()
        .child("Input")
        .child(
            Button::new("mic-unavailable")
                .label("Microphone unavailable")
                .disabled(true),
        )
        .child(
            Button::new("system-unavailable")
                .label("System audio unavailable")
                .disabled(true),
        )
        .child("Output")
        .child(format!("Save to {folder}"))
        .child(
            Button::new("browse-unavailable")
                .label("Browse")
                .disabled(true),
        )
        .child(
            Button::new("start-unavailable")
                .primary()
                .label("Start recording")
                .disabled(true),
        )
        .child(NOT_AVAILABLE)
}
