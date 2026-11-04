//! Record tab: microphone, destination, and start/stop.
//! The system-audio control stays disabled until loopback exists.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::select::{SearchableVec, Select, SelectState};
use gpui_kit::component::{h_flex, v_flex, Disableable as _};
use gpui_kit::{App, ClickEvent, Entity, IntoElement, ParentElement, SharedString, Styled, Window};

#[allow(clippy::too_many_arguments)]
pub(crate) fn record_tab<Browse, Toggle>(
    microphone: &Entity<SelectState<SearchableVec<SharedString>>>,
    destination: SharedString,
    locked: bool,
    show_stop: bool,
    saving: bool,
    elapsed: SharedString,
    phase: SharedString,
    notice: SharedString,
    browse: Browse,
    toggle: Toggle,
) -> impl IntoElement
where
    Browse: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    Toggle: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
{
    let toggle_label = if show_stop {
        "Stop recording"
    } else {
        "Start recording"
    };
    v_flex()
        .gap_3()
        .child("Input")
        .child("Microphone")
        .child(
            Select::new(microphone)
                .disabled(locked)
                .accessibility_label("Microphone"),
        )
        .child("System audio")
        .child(
            Button::new("system-later")
                .label("Default output device")
                .disabled(true),
        )
        .child("Available in a later update.")
        .child("Output")
        .child(
            h_flex().gap_2().items_center().child(destination).child(
                Button::new("browse-recordings")
                    .label("Browse")
                    .disabled(locked)
                    .on_click(browse),
            ),
        )
        .child(
            Button::new("toggle-recording")
                .primary()
                .label(toggle_label)
                .disabled(saving)
                .on_click(toggle),
        )
        .child(elapsed)
        .child(phase)
        .child(notice)
}
