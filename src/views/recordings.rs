//! Recordings tab empty state. The library scan arrives in a later milestone.

use gpui_kit::component::empty::{Empty, EmptyDescription, EmptyHeader, EmptyTitle};
use gpui_kit::{IntoElement, ParentElement as _};

pub(crate) fn recordings_placeholder() -> impl IntoElement {
    Empty::new().header(
        EmptyHeader::new()
            .title(EmptyTitle::new().child("No recordings"))
            .description(EmptyDescription::new().child("Saved recordings will appear here.")),
    )
}
