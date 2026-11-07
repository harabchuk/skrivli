//! One selectable transcription model. Takes plain data, so it knows nothing about services.

use gpui_kit::component::list::ListItem;
use gpui_kit::component::progress::Progress;
use gpui_kit::component::radio::Radio;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    h_flex, v_flex, ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{div, px, App, IntoElement, ParentElement, SharedString, Styled, Window};

const RATING_MAX: u8 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModelCardTag {
    InUse,
    Installed,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ModelCardProgress {
    pub(crate) percent: u8,
    pub(crate) left: SharedString,
    pub(crate) right: SharedString,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ModelCardData {
    pub(crate) id: SharedString,
    pub(crate) name: SharedString,
    pub(crate) description: SharedString,
    pub(crate) meta: SharedString,
    pub(crate) tag: Option<ModelCardTag>,
    pub(crate) ratings: Option<(u8, u8)>,
    pub(crate) selected: bool,
    pub(crate) disabled: bool,
    pub(crate) progress: Option<ModelCardProgress>,
    pub(crate) failure: Option<SharedString>,
    pub(crate) accessibility_label: SharedString,
}

pub(crate) fn model_tag(tag: ModelCardTag) -> impl IntoElement {
    match tag {
        ModelCardTag::InUse => Tag::success().small().rounded_full().child(
            h_flex()
                .gap_1()
                .items_center()
                .child(Icon::new(IconName::Check).xsmall())
                .child("In use"),
        ),
        ModelCardTag::Installed => Tag::success()
            .outline()
            .small()
            .rounded_full()
            .child("Installed"),
    }
}

fn rating(label: &'static str, filled: u8, cx: &App) -> impl IntoElement {
    let on = cx.theme().foreground;
    let off = cx.theme().border;
    h_flex()
        .gap_1()
        .items_center()
        .child(label)
        .children((0..RATING_MAX).map(|index| {
            div()
                .size(px(6.))
                .rounded_full()
                .bg(if index < filled { on } else { off })
        }))
}

/// `on_select` runs from both the card and its radio. The radio is the keyboard
/// stop, because `ListItem` has no focus handle. Selecting twice is harmless.
pub(crate) fn model_card(
    data: ModelCardData,
    on_select: impl Fn(&mut Window, &mut App) + Clone + 'static,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    let border = if data.selected {
        theme.primary
    } else {
        theme.border
    };
    let muted = theme.muted_foreground;
    let danger = theme.danger;
    let on_radio = on_select.clone();
    ListItem::new(SharedString::from(format!("model-card-{}", data.id)))
        .w_full()
        .p_3()
        .border_1()
        .border_color(border)
        .rounded(theme.radius_lg)
        .selected(data.selected)
        .disabled(data.disabled)
        .when(data.disabled && !data.selected, |this| this.opacity(0.55))
        .accessibility_label(data.accessibility_label.clone())
        .on_click(move |_event, window, cx| on_select(window, cx))
        .child(
            v_flex()
                .w_full()
                .gap_2()
                .child(
                    h_flex()
                        .w_full()
                        .gap_3()
                        .items_start()
                        .child(
                            Radio::new(SharedString::from(format!("model-radio-{}", data.id)))
                                .checked(data.selected)
                                .disabled(data.disabled)
                                .accessibility_label(data.accessibility_label)
                                .on_click(move |_checked, window, cx| on_radio(window, cx)),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .gap_1()
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .items_center()
                                        .child(div().font_semibold().child(data.name))
                                        .when_some(data.tag, |this, tag| {
                                            this.child(model_tag(tag))
                                        }),
                                )
                                .child(div().text_xs().text_color(muted).child(data.description))
                                .when_some(data.ratings, |this, (speed, accuracy)| {
                                    this.child(
                                        h_flex()
                                            .gap_3()
                                            .text_xs()
                                            .text_color(muted)
                                            .child(rating("Speed", speed, cx))
                                            .child(rating("Accuracy", accuracy, cx)),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_xs()
                                .text_color(muted)
                                .child(data.meta),
                        ),
                )
                .when_some(data.progress, |this, progress| {
                    this.child(
                        Progress::new(SharedString::from(format!("model-progress-{}", data.id)))
                            .w_full()
                            .value(f32::from(progress.percent))
                            .accessibility_label(progress.left.clone()),
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .justify_between()
                            .text_xs()
                            .text_color(muted)
                            .child(progress.left)
                            .child(progress.right),
                    )
                })
                .when_some(data.failure, |this, failure| {
                    this.child(div().text_xs().text_color(danger).child(failure))
                }),
        )
}
