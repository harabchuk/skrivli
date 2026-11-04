//! Record tab: microphone, system audio, destination, gain, and meters.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::select::{SearchableVec, Select, SelectState};
use gpui_kit::component::slider::{Slider, SliderState};
use gpui_kit::component::{h_flex, v_flex, Disableable as _};
use gpui_kit::{px, App, ClickEvent, Entity, IntoElement, ParentElement, SharedString, Styled};

use crate::ui::components::level_meter::level_meter;

#[allow(clippy::too_many_arguments)]
pub(crate) fn record_tab<Browse, Toggle, Agc>(
    microphone: &Entity<SelectState<SearchableVec<SharedString>>>,
    system: &Entity<SelectState<SearchableVec<SharedString>>>,
    gain: &Entity<SliderState>,
    destination: SharedString,
    locked: bool,
    show_stop: bool,
    saving: bool,
    agc: bool,
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
    toggle_agc: Agc,
    cx: &App,
) -> impl IntoElement
where
    Browse: Fn(&ClickEvent, &mut gpui_kit::Window, &mut App) + 'static,
    Toggle: Fn(&ClickEvent, &mut gpui_kit::Window, &mut App) + 'static,
    Agc: Fn(&bool, &mut gpui_kit::Window, &mut App) + 'static,
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
        .child(level_meter("Mic", mic_db, mic_hold, cx))
        .child("System audio")
        .child(
            Select::new(system)
                .disabled(locked)
                .accessibility_label("System audio"),
        )
        .child(level_meter("System", system_db, system_hold, cx))
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
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .child("Gain")
                .child(div_slider(gain, agc || saving))
                .child(gain_label)
                .child(
                    Checkbox::new("mic-agc")
                        .label("Auto")
                        .checked(agc)
                        .disabled(saving)
                        .on_click(toggle_agc),
                ),
        )
        .child(notice)
}

fn div_slider(gain: &Entity<SliderState>, disabled: bool) -> impl IntoElement {
    Slider::new(gain)
        .disabled(disabled)
        .w(px(180.))
        .accessibility_label("Microphone gain")
}
