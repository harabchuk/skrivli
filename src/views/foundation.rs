use gpui_kit::component::button::*;
use gpui_kit::component::select::{SearchableVec, Select, SelectState};
use gpui_kit::component::*;
use gpui_kit::*;

pub(crate) struct FoundationView {
    focus: FocusHandle,
    status: Entity<SelectState<SearchableVec<&'static str>>>,
}

impl FoundationView {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle().tab_stop(true);
        let status = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(vec!["Foundation"]),
                Some(IndexPath::new(0)),
                window,
                cx,
            )
        });
        window.focus(&focus, cx);
        Self { focus, status }
    }
}

impl Render for FoundationView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("foundation")
            .track_focus(&self.focus)
            .size_full()
            .p_4()
            .gap_2()
            .bg(cx.theme().background)
            .child("Foundation")
            .child(
                Button::new("foundation-ready")
                    .primary()
                    .label("Ready")
                    .on_click(|_, _, _| {}),
            )
            .child(Select::new(&self.status).w(px(220.)).placeholder("Status"))
    }
}
