//! Transcription-model controls. Takes plain data, so it knows nothing about services.

use gpui_kit::component::tag::Tag;
use gpui_kit::component::{h_flex, Icon, IconName, Sizable as _};
use gpui_kit::{IntoElement, ParentElement, Styled};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModelCardTag {
    InUse,
    Installed,
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
