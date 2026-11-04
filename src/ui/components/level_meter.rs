//! Peak meter drawn with GPUI Kit theme tokens.
//!
//! The numeric readout is the accessible indication. Color repeats the same
//! loudness and is not the only cue.

use gpui_kit::component::{h_flex, ActiveTheme as _};
use gpui_kit::{div, px, App, IntoElement, ParentElement, Styled};

use crate::domain::gain::{format_db, meter_band, meter_fraction, MeterBand};

pub(crate) fn level_meter(label: &str, db: f32, hold_db: f32, cx: &App) -> impl IntoElement {
    let fraction = meter_fraction(db);
    let fill = match meter_band(db) {
        MeterBand::Normal => cx.theme().success,
        MeterBand::Warm => cx.theme().warning,
        MeterBand::Hot => cx.theme().danger,
    };
    let readout = format!("{label}  {}  hold {}", format_db(db), format_db(hold_db));
    h_flex()
        .gap_2()
        .items_center()
        .child(label.to_string())
        .child(
            div()
                .w(px(160.))
                .h(px(8.))
                .rounded(px(4.))
                .bg(cx.theme().muted)
                .child(
                    div()
                        .h(px(8.))
                        .w(px(160. * fraction))
                        .rounded(px(4.))
                        .bg(fill),
                ),
        )
        .child(readout)
}
