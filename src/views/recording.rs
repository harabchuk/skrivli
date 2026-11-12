//! Record tab: status, timer, the main action, and one card per audio source.

use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::slider::{Slider, SliderState};
use gpui_kit::component::{
    h_flex, v_flex, ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _,
    StyledExt as _,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    div, ease_in_out, px, Anchor, Animation, AnimationExt as _, AnyElement, App, ClickEvent,
    Entity, FontWeight, IntoElement, ParentElement, SharedString, Styled, Window,
};

use rust_i18n::t;

use crate::domain::recording::RecordPhase;
use crate::services::recording::NOTICE_SAVED;
use crate::ui::components::level_meter::level_meter;

use super::notices::localize;

const COLUMN_MAX_WIDTH: f32 = 520.;
const ACTION_WIDTH: f32 = 200.;
const ACTION_HEIGHT: f32 = 36.;
const GAIN_SLIDER_WIDTH: f32 = 72.;
const TILE_SIZE: f32 = 32.;
/// Lines the meter up under the device name: tile width plus the row gap.
const METER_INDENT: f32 = 42.;
const MENU_MIN_WIDTH: f32 = 280.;
const MENU_MAX_HEIGHT: f32 = 240.;
/// Always reserved, so the centered column does not jump when a notice comes or goes.
const NOTICE_SLOT_HEIGHT: f32 = 34.;

// Lucide icons (ISC) that Kit's default asset set does not bundle.
const VOLUME_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M11 4.702a.705.705 0 0 0-1.203-.498L6.413 7.587A1.4 1.4 0 0 1 5.416 8H3a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2.416a1.4 1.4 0 0 1 .997.413l3.383 3.384A.705.705 0 0 0 11 19.298z"/><path d="M16 9a5 5 0 0 1 0 6"/><path d="M19.364 18.364a9 9 0 0 0 0-12.728"/></svg>"#;
const LOCK_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect width="18" height="11" x="3" y="11" rx="2" ry="2"/><path d="M7 11V7a5 5 0 0 1 10 0v4"/></svg>"#;
const RECORD_DOT_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor"><circle cx="12" cy="12" r="8"/></svg>"#;

pub(crate) type DevicePick = Rc<dyn Fn(usize, &mut Window, &mut App)>;

/// The device list for one source. The first option is the Windows default and
/// the last is Disabled.
pub(crate) struct DeviceChoice {
    options: Vec<SharedString>,
    selected: usize,
    on_pick: DevicePick,
}

impl DeviceChoice {
    pub(crate) fn new<T: PartialEq>(
        options: &[SharedString],
        choices: &[T],
        current: &T,
        on_pick: DevicePick,
    ) -> Self {
        Self {
            options: options.to_vec(),
            selected: selected_index(choices, current),
            on_pick,
        }
    }

    fn name(&self) -> SharedString {
        self.options.get(self.selected).cloned().unwrap_or_default()
    }
}

pub(crate) struct RecordView<'a> {
    pub(crate) microphone: DeviceChoice,
    pub(crate) system: DeviceChoice,
    pub(crate) gain: &'a Entity<SliderState>,
    pub(crate) gain_label: SharedString,
    pub(crate) system_gain: &'a Entity<SliderState>,
    pub(crate) system_gain_label: SharedString,
    pub(crate) locked: bool,
    pub(crate) phase: RecordPhase,
    pub(crate) elapsed: SharedString,
    pub(crate) notice: Option<&'static str>,
    pub(crate) mic_db: f32,
    pub(crate) mic_hold: f32,
    pub(crate) system_db: f32,
    pub(crate) system_hold: f32,
}

struct SourceRow {
    id: &'static str,
    caption: SharedString,
    icon: Icon,
    device: DeviceChoice,
    extra: Option<AnyElement>,
    locked: bool,
    db: f32,
    hold: f32,
}

pub(crate) fn record_tab<Toggle>(view: RecordView<'_>, toggle: Toggle, cx: &App) -> impl IntoElement
where
    Toggle: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
{
    let saving = view.phase == RecordPhase::Stopping;
    let gain = gain_controls(
        view.gain,
        view.gain_label,
        ts!("app.record.microphone_gain"),
        saving,
        cx,
    )
    .into_any_element();
    let system_gain = gain_controls(
        view.system_gain,
        view.system_gain_label,
        ts!("app.record.system_gain"),
        saving,
        cx,
    )
    .into_any_element();
    let microphone = SourceRow {
        id: "microphone",
        caption: ts!("app.record.microphone"),
        icon: Icon::new(IconName::Mic),
        device: view.microphone,
        extra: Some(gain),
        locked: view.locked,
        db: view.mic_db,
        hold: view.mic_hold,
    };
    let system = SourceRow {
        id: "system",
        caption: ts!("app.record.system_audio"),
        icon: Icon::default().data(VOLUME_SVG),
        device: view.system,
        extra: Some(system_gain),
        locked: view.locked,
        db: view.system_db,
        hold: view.system_hold,
    };
    div().size_full().overflow_y_scrollbar().child(
        v_flex()
            .w_full()
            .min_h_full()
            .items_center()
            .justify_center()
            .child(
                v_flex()
                    .w_full()
                    .max_w(px(COLUMN_MAX_WIDTH))
                    .gap(px(14.))
                    .child(hero(view.phase, view.elapsed, toggle, cx))
                    .child(
                        v_flex()
                            .w_full()
                            .border_1()
                            .border_color(cx.theme().border)
                            .rounded(cx.theme().radius_lg)
                            .child(source_row(microphone, cx))
                            .child(div().w_full().h(px(1.)).bg(cx.theme().border))
                            .child(source_row(system, cx)),
                    )
                    .child(
                        div()
                            .w_full()
                            .min_h(px(NOTICE_SLOT_HEIGHT))
                            .when_some(view.notice, |this, notice| {
                                this.child(notice_banner(notice, cx))
                            }),
                    ),
            ),
    )
}

fn hero<Toggle>(
    phase: RecordPhase,
    elapsed: SharedString,
    toggle: Toggle,
    cx: &App,
) -> impl IntoElement
where
    Toggle: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
{
    let look = phase_look(phase);
    let button = Button::new("toggle-recording")
        .w(px(ACTION_WIDTH))
        .h(px(ACTION_HEIGHT))
        .disabled(phase == RecordPhase::Stopping)
        .on_click(toggle);
    let button = if look.active {
        button
            .danger()
            .icon(Icon::new(IconName::Square).xsmall())
            .label(ts!("app.record.stop"))
    } else {
        button
            .primary()
            .icon(
                Icon::default()
                    .data(RECORD_DOT_SVG)
                    .xsmall()
                    .text_color(cx.theme().danger),
            )
            .label(ts!("app.record.start"))
    };
    v_flex()
        .items_center()
        .child(status_chip(look, cx))
        .child(
            div()
                .text_size(px(36.))
                .line_height(px(42.))
                .font_weight(FontWeight::LIGHT)
                .when(!look.active, |this| {
                    this.text_color(cx.theme().muted_foreground)
                })
                .child(elapsed),
        )
        .child(div().mt_2().child(button))
}

fn status_chip(look: PhaseLook, cx: &App) -> impl IntoElement {
    let (background, foreground) = if look.live {
        (cx.theme().danger.opacity(0.12), cx.theme().danger)
    } else {
        (cx.theme().muted, cx.theme().muted_foreground)
    };
    let marker = div().size(px(7.)).rounded_full().bg(foreground);
    h_flex()
        .h(px(22.))
        .px(px(9.))
        .gap(px(6.))
        .items_center()
        .rounded_full()
        .bg(background)
        .text_color(foreground)
        .text_xs()
        .font_medium()
        .map(|this| {
            if look.live {
                this.child(
                    marker.with_animation(
                        "recording-pulse",
                        Animation::new(Duration::from_millis(1400))
                            .repeat()
                            .with_easing(ease_in_out),
                        |this, delta| this.opacity(0.35 + 0.65 * (2. * delta - 1.).abs()),
                    ),
                )
            } else {
                this.child(marker)
            }
        })
        .child(ts!(look.label_key))
}

fn source_row(row: SourceRow, cx: &App) -> impl IntoElement {
    let name = row.device.name();
    v_flex()
        .w_full()
        .px_3()
        .py(px(10.))
        .gap(px(6.))
        .child(
            h_flex()
                .w_full()
                .gap(px(10.))
                .items_center()
                .child(
                    div()
                        .flex()
                        .flex_shrink_0()
                        .size(px(TILE_SIZE))
                        .items_center()
                        .justify_center()
                        .rounded(cx.theme().radius)
                        .bg(cx.theme().muted)
                        .text_color(cx.theme().muted_foreground)
                        .child(row.icon.small()),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(row.caption.clone()),
                        )
                        .child(div().text_sm().font_semibold().truncate().child(name)),
                )
                .children(row.extra)
                .child(device_control(row.id, row.caption, row.device, row.locked)),
        )
        .child(
            div()
                .w_full()
                .pl(px(METER_INDENT))
                .child(level_meter(row.id, row.db, row.hold, cx)),
        )
}

fn device_control(
    id: &'static str,
    caption: SharedString,
    device: DeviceChoice,
    locked: bool,
) -> AnyElement {
    if locked {
        let hint = ts!("app.record.locked_hint");
        return Button::new(SharedString::from(format!("{id}-locked")))
            .ghost()
            .small()
            .icon(Icon::default().data(LOCK_SVG))
            .tooltip(hint.clone())
            .accessibility_label(format!("{caption}: {hint}"))
            .into_any_element();
    }
    let DeviceChoice {
        options,
        selected,
        on_pick,
    } = device;
    Button::new(SharedString::from(format!("{id}-change")))
        .outline()
        .small()
        .label(ts!("app.record.change"))
        .dropdown_caret(true)
        .accessibility_label(t!("app.record.change_device", source = caption).into_owned())
        .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _window, _cx| {
            let menu = menu
                .min_w(px(MENU_MIN_WIDTH))
                .max_h(px(MENU_MAX_HEIGHT))
                .scrollable(true);
            let count = options.len();
            options
                .iter()
                .enumerate()
                .fold(menu, |menu, (index, label)| {
                    let pick = on_pick.clone();
                    let menu = if separator_before(index, count) {
                        menu.separator()
                    } else {
                        menu
                    };
                    menu.item(
                        PopupMenuItem::new(label.clone())
                            .checked(index == selected)
                            .on_click(move |_event, window, cx| pick(index, window, cx)),
                    )
                })
        })
        .into_any_element()
}

fn gain_controls(
    gain: &Entity<SliderState>,
    gain_label: SharedString,
    accessibility_label: SharedString,
    saving: bool,
    cx: &App,
) -> impl IntoElement {
    h_flex()
        .gap(px(6.))
        .items_center()
        .flex_shrink_0()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(ts!("app.record.gain"))
        .child(
            Slider::new(gain)
                .disabled(saving)
                .w(px(GAIN_SLIDER_WIDTH))
                .accessibility_label(accessibility_label),
        )
        .child(gain_label)
}

fn notice_banner(notice: &'static str, cx: &App) -> impl IntoElement {
    let text = localize(notice);
    let row = h_flex()
        .w_full()
        .min_h(px(NOTICE_SLOT_HEIGHT))
        .gap_2()
        .items_center()
        .px(px(10.))
        .py(px(7.))
        .rounded(cx.theme().radius)
        .border_1()
        .text_xs();
    match notice_tone(notice) {
        NoticeTone::Info => row
            .border_color(cx.theme().border)
            .bg(cx.theme().muted)
            .text_color(cx.theme().muted_foreground)
            .child(Icon::new(IconName::Info).small().flex_shrink_0())
            .child(text),
        NoticeTone::Warning => row
            .border_color(cx.theme().warning.opacity(0.5))
            .bg(cx.theme().warning.opacity(0.12))
            .child(
                Icon::new(IconName::TriangleAlert)
                    .small()
                    .flex_shrink_0()
                    .text_color(cx.theme().warning),
            )
            .child(text),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PhaseLook {
    /// Catalog key of the chip text.
    label_key: &'static str,
    /// Stop is offered and the timer reads as running.
    active: bool,
    /// Capture is running, so the chip turns red and pulses.
    live: bool,
}

fn phase_look(phase: RecordPhase) -> PhaseLook {
    let (label_key, active, live) = match phase {
        RecordPhase::Idle => ("app.record.phase_ready", false, false),
        RecordPhase::Starting => ("app.record.phase_starting", true, false),
        RecordPhase::Recording => ("app.record.phase_recording", true, true),
        RecordPhase::Stopping => ("app.record.phase_saving", true, false),
        RecordPhase::Error => ("app.record.phase_not_recording", false, false),
    };
    PhaseLook {
        label_key,
        active,
        live,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NoticeTone {
    Info,
    Warning,
}

fn notice_tone(notice: &str) -> NoticeTone {
    if notice == NOTICE_SAVED {
        NoticeTone::Info
    } else {
        NoticeTone::Warning
    }
}

/// Good news ("Saved.") lives in the shared status bar; only warnings get the banner.
pub(crate) fn banner_notice(notice: Option<&'static str>) -> Option<&'static str> {
    notice.filter(|notice| notice_tone(notice) == NoticeTone::Warning)
}

/// A separator sits after the Windows default and before the last option
/// (Disabled), so Disabled stays apart from the device list.
fn separator_before(index: usize, count: usize) -> bool {
    index == 1 || (count > 2 && index + 1 == count)
}

/// An unknown choice shows the first option, the Windows default.
fn selected_index<T: PartialEq>(choices: &[T], current: &T) -> usize {
    choices
        .iter()
        .position(|choice| choice == current)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PHASES: [RecordPhase; 5] = [
        RecordPhase::Idle,
        RecordPhase::Starting,
        RecordPhase::Recording,
        RecordPhase::Stopping,
        RecordPhase::Error,
    ];

    #[test]
    fn only_a_running_capture_looks_live() {
        // Arrange
        let phases = PHASES;

        // Act
        let live = phases.map(|phase| phase_look(phase).live);

        // Assert
        assert_eq!(live, [false, false, true, false, false]);
    }

    #[test]
    fn stop_is_offered_from_starting_through_saving() {
        // Arrange
        let phases = PHASES;

        // Act
        let active = phases.map(|phase| phase_look(phase).active);

        // Assert
        assert_eq!(active, [false, true, true, true, false]);
    }

    #[test]
    fn good_news_is_not_shown_as_a_warning() {
        // Arrange
        let failure = "The microphone is not available.";

        // Act
        let tones = (notice_tone(NOTICE_SAVED), notice_tone(failure));

        // Assert
        assert_eq!(tones, (NoticeTone::Info, NoticeTone::Warning));
    }

    #[test]
    fn saved_stays_out_of_the_banner_but_warnings_stay_in() {
        // Arrange
        let failure = "The microphone is not available.";

        // Act
        let shown = (
            banner_notice(Some(NOTICE_SAVED)),
            banner_notice(Some(failure)),
            banner_notice(None),
        );

        // Assert
        assert_eq!(shown, (None, Some(failure), None));
    }

    #[test]
    fn selected_index_finds_the_current_choice() {
        // Arrange
        let choices = ["default", "usb"];

        // Act
        let index = selected_index(&choices, &"usb");

        // Assert
        assert_eq!(index, 1);
    }

    #[test]
    fn selected_index_falls_back_to_the_default_option() {
        // Arrange
        let choices = ["default", "usb"];

        // Act
        let index = selected_index(&choices, &"gone");

        // Assert
        assert_eq!(index, 0);
    }

    #[test]
    fn selected_index_finds_a_disabled_choice() {
        // Arrange
        use crate::infrastructure::storage::settings::StoredEndpoint;
        let choices = [
            StoredEndpoint::Default,
            StoredEndpoint::explicit("usb"),
            StoredEndpoint::Disabled,
        ];

        // Act
        let index = selected_index(&choices, &StoredEndpoint::Disabled);

        // Assert
        assert_eq!(index, 2);
    }

    #[test]
    fn separators_frame_the_device_list_and_disabled_option() {
        // Arrange
        let count = 4;

        // Act
        let marks: Vec<bool> = (0..count).map(|i| separator_before(i, count)).collect();
        let only_two: Vec<bool> = (0..2).map(|i| separator_before(i, 2)).collect();

        // Assert
        assert_eq!(marks, [false, true, false, true]);
        assert_eq!(only_two, [false, true]);
    }
}
