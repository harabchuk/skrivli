//! Peak meter drawn with GPUI Kit theme tokens.
//!
//! The numeric readout is the accessible indication. Color repeats the same
//! loudness and is not the only cue. The track uses the band color at low
//! opacity so empty meters stay visible in both light and dark themes. Peak
//! hold is a tick on the track, with its value in the readout's tooltip.

use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{h_flex, ActiveTheme as _};
use gpui_kit::prelude::*;
use gpui_kit::{div, px, relative, App, IntoElement, ParentElement, SharedString, Styled};
use rust_i18n::t;

use crate::domain::gain::{format_db, meter_band, meter_fraction, MeterBand};

const READOUT_WIDTH: f32 = 44.;
const TRACK_HEIGHT: f32 = 6.;
const TRACK_OPACITY: f32 = 0.2;
const TICK_WIDTH: f32 = 2.;
const TICK_OVERHANG: f32 = 2.;

pub(crate) fn level_meter(id: &'static str, db: f32, hold_db: f32, cx: &App) -> impl IntoElement {
    let fraction = meter_fraction(db);
    let hold_fraction = meter_fraction(hold_db);
    let fill = match meter_band(db) {
        MeterBand::Normal => cx.theme().success,
        MeterBand::Warm => cx.theme().warning,
        MeterBand::Hot => cx.theme().danger,
    };
    let hold_text =
        SharedString::from(t!("app.record.peak_hold", value = format_db(hold_db)).into_owned());
    h_flex()
        .w_full()
        .gap(px(10.))
        .items_center()
        .child(
            div()
                .relative()
                .flex_1()
                .min_w_0()
                .h(px(TRACK_HEIGHT))
                .rounded(px(3.))
                .bg(fill.opacity(TRACK_OPACITY))
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .h_full()
                        .w(relative(fraction))
                        .rounded(px(3.))
                        .bg(fill),
                )
                .when(hold_fraction > 0., |this| {
                    this.child(
                        div()
                            .absolute()
                            .left(relative(hold_fraction))
                            .top(px(-TICK_OVERHANG))
                            .w(px(TICK_WIDTH))
                            .h(px(TRACK_HEIGHT + 2. * TICK_OVERHANG))
                            .rounded(px(1.))
                            .bg(cx.theme().foreground),
                    )
                }),
        )
        .child(
            div()
                .id(SharedString::from(format!("{id}-level")))
                .w(px(READOUT_WIDTH))
                .flex_shrink_0()
                .flex()
                .justify_end()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .tooltip(move |window, cx| Tooltip::new(hold_text.clone()).build(window, cx))
                .child(format_db(db)),
        )
}
