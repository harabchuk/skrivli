//! Grouped rows for Settings: a section label, a bordered group, text left and control right.

use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, StyledExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{div, AnyElement, App, IntoElement, ParentElement, SharedString, Styled};

pub(crate) fn settings_section(
    title: &'static str,
    rows: impl IntoIterator<Item = AnyElement>,
    cx: &App,
) -> impl IntoElement {
    let border = cx.theme().border;
    v_flex()
        .w_full()
        .gap_1p5()
        .child(
            div()
                .text_xs()
                .font_semibold()
                .text_color(cx.theme().muted_foreground)
                .child(title.to_uppercase()),
        )
        .child(
            v_flex()
                .w_full()
                .border_1()
                .border_color(border)
                .rounded(cx.theme().radius_lg)
                .children(rows.into_iter().enumerate().map(|(index, row)| {
                    div()
                        .w_full()
                        .when(index > 0, |this| this.border_t_1().border_color(border))
                        .child(row)
                })),
        )
}

pub(crate) fn settings_row(
    title: impl IntoElement,
    description: impl Into<SharedString>,
    control: impl IntoElement,
    cx: &App,
) -> AnyElement {
    h_flex()
        .w_full()
        .gap_3()
        .px_3()
        .py_2p5()
        .items_center()
        .justify_between()
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .child(div().text_sm().font_semibold().child(title))
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(description.into()),
                ),
        )
        .child(div().flex_shrink_0().child(control))
        .into_any_element()
}
