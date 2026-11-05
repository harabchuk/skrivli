//! Peak meter drawn with GPUI Kit theme tokens.
//!
//! The numeric readout is the accessible indication. Color repeats the same
//! loudness and is not the only cue.

use gpui_kit::component::{h_flex, ActiveTheme as _};
use gpui_kit::{div, px, App, IntoElement, ParentElement, Styled};

use crate::domain::gain::{format_db, meter_band, meter_fraction, MeterBand};

const LABEL_WIDTH: f32 = 56.;
const TRACK_WIDTH: f32 = 160.;
const TRACK_HEIGHT: f32 = 8.;

pub(crate) fn level_meter(label: &str, db: f32, hold_db: f32, cx: &App) -> impl IntoElement {
    let fraction = meter_fraction(db);
    let fill = match meter_band(db) {
        MeterBand::Normal => cx.theme().success,
        MeterBand::Warm => cx.theme().warning,
        MeterBand::Hot => cx.theme().danger,
    };
    let readout = format!("{}  hold {}", format_db(db), format_db(hold_db));
    h_flex()
        .gap_2()
        .items_center()
        .child(
            div()
                .w(px(LABEL_WIDTH))
                .flex_shrink_0()
                .child(label.to_string()),
        )
        .child(
            div()
                .w(px(TRACK_WIDTH))
                .h(px(TRACK_HEIGHT))
                .flex_shrink_0()
                .rounded(px(4.))
                .bg(cx.theme().muted)
                .child(
                    div()
                        .h(px(TRACK_HEIGHT))
                        .w(px(TRACK_WIDTH * fraction))
                        .rounded(px(4.))
                        .bg(fill),
                ),
        )
        .child(readout)
}
