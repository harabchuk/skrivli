//! Record tab: microphone, system audio, destination, gain, and meters.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::select::{SearchableVec, Select, SelectState};
use gpui_kit::component::slider::{Slider, SliderState};
use gpui_kit::component::{h_flex, v_flex, Disableable as _, Sizable as _};
use gpui_kit::{
    div, px, App, ClickEvent, Entity, IntoElement, ParentElement, SharedString, Styled,
};

use crate::ui::components::level_meter::level_meter;

const FIELD_LABEL_WIDTH: f32 = 110.;
const START_BUTTON_WIDTH: f32 = 240.;
const GAIN_SLIDER_WIDTH: f32 = 100.;

#[allow(clippy::too_many_arguments)]
pub(crate) fn record_tab<Browse, Toggle>(
    microphone: &Entity<SelectState<SearchableVec<SharedString>>>,
    system: &Entity<SelectState<SearchableVec<SharedString>>>,
    gain: &Entity<SliderState>,
    destination: SharedString,
    locked: bool,
    show_stop: bool,
    saving: bool,
    gain_label: SharedString,
    elapsed: SharedString,
    phase: SharedString,
    notice: SharedString,
    mic_db: f32,
    mic_hold: f32,
    system_db: f32,
    system_hold: f32,
    browse: Browse,
    toggle: Toggle,
    cx: &App,
) -> impl IntoElement
where
    Browse: Fn(&ClickEvent, &mut gpui_kit::Window, &mut App) + 'static,
    Toggle: Fn(&ClickEvent, &mut gpui_kit::Window, &mut App) + 'static,
{
    let toggle_label = if show_stop {
        "Stop recording"
    } else {
        "Start recording"
    };
    v_flex()
        .gap_3()
        .child("Input")
        .child(labeled_select(
            "Microphone",
            Select::new(microphone)
                .disabled(locked)
                .accessibility_label("Microphone"),
        ))
        .child(labeled_select(
            "System audio",
            Select::new(system)
                .disabled(locked)
                .accessibility_label("System audio"),
        ))
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
            h_flex().w_full().justify_center().child(
                Button::new("toggle-recording")
                    .primary()
                    .large()
                    .w(px(START_BUTTON_WIDTH))
                    .label(toggle_label)
                    .disabled(saving)
                    .on_click(toggle),
            ),
        )
        .child(h_flex().w_full().justify_center().child(elapsed))
        .child(h_flex().w_full().justify_center().child(phase))
        .child(
            h_flex()
                .gap_3()
                .items_center()
                .w_full()
                .child(level_meter("Mic", mic_db, mic_hold, cx))
                .child(gain_controls(gain, gain_label, saving)),
        )
        .child(level_meter("System", system_db, system_hold, cx))
        .child(notice)
}

fn labeled_select(label: &str, select: impl IntoElement) -> impl IntoElement {
    h_flex()
        .gap_3()
        .items_center()
        .w_full()
        .child(
            div()
                .w(px(FIELD_LABEL_WIDTH))
                .flex_shrink_0()
                .child(label.to_string()),
        )
        .child(div().flex_1().min_w_0().child(select))
}

fn gain_controls(
    gain: &Entity<SliderState>,
    gain_label: SharedString,
    saving: bool,
) -> impl IntoElement {
    h_flex()
        .gap_2()
        .items_center()
        .flex_shrink_0()
        .child("Gain")
        .child(
            Slider::new(gain)
                .disabled(saving)
                .w(px(GAIN_SLIDER_WIDTH))
                .accessibility_label("Microphone gain"),
        )
        .child(gain_label)
}
