//! Recordings tab: grouped library list, detail pane, playback and file actions.

use crate::ui::components::dialog_style::{dialog_footer, DIALOG_WIDTH};
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::clipboard::Clipboard;
use gpui_kit::component::dialog::{DialogAction, DialogClose};
use gpui_kit::component::empty::{Empty, EmptyDescription, EmptyHeader, EmptyTitle};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::progress::Progress;
use gpui_kit::component::slider::{Slider, SliderState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{
    h_flex, v_flex, ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _,
    StyledExt as _, WindowExt as _,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    div, px, Anchor, AnyElement, App, AppContext as _, ClickEvent, Div, Entity,
    InteractiveElement as _, IntoElement, ParentElement, SharedString, Stateful,
    StatefulInteractiveElement as _, Styled, Window,
};

// Lucide icons (ISC). Pencil and Trash are in the full catalog but not in Kit's
// default `Assets`, so they are embedded here, as are Lucide Play and Pause. Ellipsis, FolderOpen and
// TriangleAlert are in the default set.
const PENCIL_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21.174 6.812a1 1 0 0 0-3.986-3.987L3.842 16.174a2 2 0 0 0-.5.83l-1.321 4.352a.5.5 0 0 0 .623.622l4.353-1.32a2 2 0 0 0 .83-.497z"/><path d="m15 5 4 4"/></svg>"#;
const PLAY_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polygon points="6 3 20 12 6 21 6 3"/></svg>"#;
const PAUSE_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="14" y="3" width="5" height="18" rx="1"/><rect x="5" y="3" width="5" height="18" rx="1"/></svg>"#;
const TRASH_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M10 11v6"/><path d="M14 11v6"/><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6"/><path d="M3 6h18"/><path d="M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/></svg>"#;

use std::rc::Rc;

use jiff::tz::TimeZone;
use jiff::Timestamp;

use crate::domain::audio::CivilTime;
use crate::domain::ids::RecordingId;
use crate::domain::recording::{
    format_duration_ms, format_size, recording_day, RecordingContainer, RecordingDay,
    RecordingInfo, SidecarState, TranscriptStatus,
};
use crate::infrastructure::storage::library::LibraryCatalog;
use crate::services::playback::PlayerPhase;

#[derive(Clone, Debug)]
pub(crate) struct RecordingRow {
    pub(crate) id: RecordingId,
    pub(crate) name: SharedString,
    pub(crate) notes: Vec<SharedString>,
    pub(crate) day: RecordingDay,
    pub(crate) meta: SharedString,
    pub(crate) detail_line: SharedString,
    pub(crate) badge: RowBadge,
    recovered: bool,
}

impl RecordingRow {
    /// Build a row with an explicit time zone and "today", so tests are exact.
    pub(crate) fn from_info_in(info: &RecordingInfo, zone: &TimeZone, today: CivilTime) -> Self {
        let started = info
            .started_at_unix_ms()
            .and_then(|ms| civil_from_unix_ms(ms, zone).ok());
        let day = started.map_or(RecordingDay::Earlier, |time| recording_day(time, today));
        let duration = info
            .duration_ms()
            .map(format_duration_ms)
            .unwrap_or_else(|| "—".to_string());
        let size = format_size(info.size_bytes());
        let notes = recording_notes(
            info.recovered(),
            info.sidecar(),
            !info.problems().is_empty(),
        )
        .into_iter()
        .map(SharedString::from)
        .collect();
        let meta = row_meta(day, started, &duration, &size);
        let detail_line = detail_line(day, started, &duration, &size, info.container());
        Self {
            id: info.id(),
            name: SharedString::from(info.name().to_string()),
            notes,
            day,
            meta: SharedString::from(meta),
            detail_line: SharedString::from(detail_line),
            badge: RowBadge::from_status(info.transcript(), info.recovered()),
            recovered: info.recovered(),
        }
    }
}

/// The recorded time could not be converted to civil time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TimestampOutOfRange;

/// Convert Unix milliseconds to civil time in `zone`.
pub(crate) fn civil_from_unix_ms(
    unix_ms: u64,
    zone: &TimeZone,
) -> Result<CivilTime, TimestampOutOfRange> {
    let ms = i64::try_from(unix_ms).map_err(|_| TimestampOutOfRange)?;
    let timestamp = Timestamp::from_millisecond(ms).map_err(|_| TimestampOutOfRange)?;
    let dt = timestamp.to_zoned(zone.clone()).datetime();
    let year = i32::from(dt.year());
    let month = u8::try_from(dt.month()).map_err(|_| TimestampOutOfRange)?;
    let day = u8::try_from(dt.day()).map_err(|_| TimestampOutOfRange)?;
    let hour = u8::try_from(dt.hour()).map_err(|_| TimestampOutOfRange)?;
    let minute = u8::try_from(dt.minute()).map_err(|_| TimestampOutOfRange)?;
    let second = u8::try_from(dt.second()).map_err(|_| TimestampOutOfRange)?;
    CivilTime::new(year, month, day, hour, minute, second).ok_or(TimestampOutOfRange)
}

/// The system time zone and the current local civil time.
pub(crate) fn local_clock() -> (TimeZone, CivilTime) {
    let zone = TimeZone::system();
    let today = u64::try_from(Timestamp::now().as_millisecond())
        .ok()
        .and_then(|ms| civil_from_unix_ms(ms, &zone).ok())
        .unwrap_or(CivilTime {
            year: 1970,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
        });
    (zone, today)
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

fn clock_text(time: CivilTime) -> String {
    format!("{:02}:{:02}", time.hour, time.minute)
}

fn month_day_text(time: CivilTime) -> String {
    let month = MONTHS
        .get(usize::from(time.month).saturating_sub(1))
        .copied()
        .unwrap_or("—");
    format!("{month} {}", time.day)
}

/// Second line of a list row: `14:32 · 12:41 · 38 MB` or `Nov 3 · 31:04 · 93 MB`.
fn row_meta(day: RecordingDay, started: Option<CivilTime>, duration: &str, size: &str) -> String {
    let when = match (day, started) {
        (_, None) => "—".to_string(),
        (RecordingDay::Today | RecordingDay::Yesterday, Some(time)) => clock_text(time),
        (RecordingDay::Earlier, Some(time)) => month_day_text(time),
    };
    format!("{when} · {duration} · {size}")
}

/// Detail header line: `Today, 14:32 · 12:41 · 38 MB · WAV`.
fn detail_line(
    day: RecordingDay,
    started: Option<CivilTime>,
    duration: &str,
    size: &str,
    container: RecordingContainer,
) -> String {
    let when = match (day, started) {
        (_, None) => "—".to_string(),
        (RecordingDay::Today, Some(time)) => format!("Today, {}", clock_text(time)),
        (RecordingDay::Yesterday, Some(time)) => format!("Yesterday, {}", clock_text(time)),
        (RecordingDay::Earlier, Some(time)) => {
            format!("{}, {}", month_day_text(time), clock_text(time))
        }
    };
    let format = container.extension().to_uppercase();
    format!("{when} · {duration} · {size} · {format}")
}

const NOTE_RECOVERED: &str =
    "Recovered after an interruption. The end of this recording may be missing.";
const NOTE_SIDECAR_MISSING: &str =
    "Recording details are missing. The name and times come from the audio file.";
const NOTE_SIDECAR_MALFORMED: &str =
    "Recording details could not be read. The name and times come from the audio file.";
const NOTE_PROBLEMS: &str = "Problems occurred while recording. Parts of the audio may be missing.";

/// Notes shown under the title of the detail pane. Empty for a clean recording.
fn recording_notes(
    recovered: bool,
    sidecar: SidecarState,
    has_problems: bool,
) -> Vec<&'static str> {
    let mut notes = Vec::new();
    if recovered {
        notes.push(NOTE_RECOVERED);
    }
    match sidecar {
        SidecarState::Present => {}
        SidecarState::Missing => notes.push(NOTE_SIDECAR_MISSING),
        SidecarState::Malformed => notes.push(NOTE_SIDECAR_MALFORMED),
    }
    if has_problems {
        notes.push(NOTE_PROBLEMS);
    }
    notes
}

/// What the right edge of a list row shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RowBadge {
    None,
    Queued,
    Progress(u8),
    Ready,
    Failed,
    Recovered,
}

impl RowBadge {
    /// An active or failed status wins over Recovered. Recovered wins over Ready or nothing.
    pub(crate) fn from_status(status: TranscriptStatus, recovered: bool) -> Self {
        match status {
            TranscriptStatus::Queued => Self::Queued,
            TranscriptStatus::Progress(percent) => Self::Progress(percent),
            TranscriptStatus::Failed => Self::Failed,
            _ if recovered => Self::Recovered,
            TranscriptStatus::Ready => Self::Ready,
            TranscriptStatus::None => Self::None,
        }
    }
}

/// Height of a day header in the list, in logical pixels.
pub(crate) const HEADER_HEIGHT_PX: f32 = 32.0;
/// Height of a recording row in the list, in logical pixels.
pub(crate) const ROW_HEIGHT_PX: f32 = 56.0;
/// Fixed width of the recordings list.
const LIST_WIDTH_PX: f32 = 330.0;

/// One entry of the grouped list: a day header or a recording.
#[derive(Clone, Debug)]
pub(crate) enum LibraryItem {
    Header(RecordingDay),
    Recording(RecordingRow),
}

impl LibraryItem {
    pub(crate) fn height_px(&self) -> f32 {
        match self {
            Self::Header(_) => HEADER_HEIGHT_PX,
            Self::Recording(_) => ROW_HEIGHT_PX,
        }
    }
}

/// Build list items in catalog order, with a header wherever the day changes.
pub(crate) fn library_items(rows: &[RecordingRow]) -> Vec<LibraryItem> {
    let mut items = Vec::with_capacity(rows.len() + 3);
    let mut current = None;
    for row in rows {
        if current != Some(row.day) {
            current = Some(row.day);
            items.push(LibraryItem::Header(row.day));
        }
        items.push(LibraryItem::Recording(row.clone()));
    }
    items
}

/// Update one row's transcript status in place. Returns false for an unknown id.
pub(crate) fn set_row_status(
    rows: &mut [RecordingRow],
    id: RecordingId,
    status: TranscriptStatus,
) -> bool {
    let Some(row) = rows.iter_mut().find(|row| row.id == id) else {
        return false;
    };
    row.badge = RowBadge::from_status(status, row.recovered);
    true
}

fn day_label(day: RecordingDay) -> &'static str {
    match day {
        RecordingDay::Today => "Today",
        RecordingDay::Yesterday => "Yesterday",
        RecordingDay::Earlier => "Earlier",
    }
}

/// A muted day heading between groups of rows.
pub(crate) fn library_header(day: RecordingDay, cx: &App) -> Div {
    div()
        .h(px(HEADER_HEIGHT_PX))
        .px_3()
        .flex()
        .items_end()
        .pb_1()
        .text_xs()
        .font_medium()
        .text_color(cx.theme().muted_foreground)
        .child(day_label(day))
}

/// One recording in the list: name, metadata, and a status badge on the right.
pub(crate) fn recording_list_row(
    row: &RecordingRow,
    selected: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> Stateful<Div> {
    let theme = cx.theme();
    let background = theme.list_active;
    let hover = theme.list_hover;
    let bar = theme.primary;
    div()
        .id(SharedString::from(format!(
            "recording-row-{}",
            row.id.raw()
        )))
        .relative()
        .h(px(ROW_HEIGHT_PX))
        .px_3()
        .rounded(px(8.))
        .flex()
        .items_center()
        .gap_2()
        .cursor_pointer()
        .hover(move |style| style.bg(hover))
        .when(selected, move |el| {
            el.bg(background).child(
                div()
                    .absolute()
                    .left_0()
                    .top(px(10.))
                    .bottom(px(10.))
                    .w(px(3.))
                    .rounded(px(2.))
                    .bg(bar),
            )
        })
        .on_click(on_click)
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(div().truncate().font_medium().child(row.name.clone()))
                .child(
                    div()
                        .truncate()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(row.meta.clone()),
                ),
        )
        .child(row_badge(row.badge, cx))
}

fn row_badge(badge: RowBadge, cx: &App) -> Div {
    let theme = cx.theme();
    let (text, color) = match badge {
        RowBadge::None => return div(),
        RowBadge::Queued => (SharedString::from("Queued"), theme.muted_foreground),
        RowBadge::Progress(percent) => (SharedString::from(format!("{percent}%")), theme.primary),
        RowBadge::Ready => (SharedString::from("Transcript"), theme.success),
        RowBadge::Failed => (SharedString::from("Failed"), theme.danger),
        RowBadge::Recovered => (SharedString::from("Recovered"), theme.warning),
    };
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap_1()
        .text_xs()
        .text_color(color)
        .when(matches!(badge, RowBadge::Progress(_)), |el| {
            el.child(Spinner::new().xsmall().color(color))
        })
        .child(text)
}

/// A click handler shared between buttons and menu items.
pub(crate) type ClickAction = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// Everything the Recordings tab can ask the app to do.
pub(crate) struct RecordingsActions {
    pub(crate) refresh: ClickAction,
    pub(crate) open_folder: ClickAction,
    pub(crate) play: ClickAction,
    pub(crate) rename: ClickAction,
    pub(crate) delete: ClickAction,
    pub(crate) transcribe: ClickAction,
    /// Opens the "Replace this transcript?" confirmation.
    pub(crate) transcribe_again: ClickAction,
    /// Present only when the selected recording has a job to cancel.
    pub(crate) cancel_job: Option<ClickAction>,
    pub(crate) retry: ClickAction,
    pub(crate) copy_transcript: CopyAction,
}

/// Supplies the text the Copy button puts on the clipboard.
pub(crate) type CopyAction = Rc<dyn Fn(&mut Window, &mut App) -> SharedString>;

/// What the Recordings tab shows.
pub(crate) struct RecordingsView<'a> {
    pub(crate) list: Option<AnyElement>,
    pub(crate) count: usize,
    pub(crate) selected: Option<&'a RecordingRow>,
    pub(crate) player: PlayerCard<'a>,
    pub(crate) error: Option<SharedString>,
    pub(crate) pane: TranscriptPane,
    /// Whether the `⋯` menu offers Transcribe again.
    pub(crate) can_transcribe_again: bool,
    /// The scrolling paragraph list. Used only by the Ready pane.
    pub(crate) transcript_list: Option<AnyElement>,
}

/// Data for the player card under the title row.
pub(crate) struct PlayerCard<'a> {
    /// Shows Pause instead of Play.
    pub(crate) playing: bool,
    /// False for formats the player cannot open. The card is then disabled.
    pub(crate) playable: bool,
    pub(crate) position_ms: u64,
    pub(crate) duration_ms: Option<u64>,
    pub(crate) seek: &'a Entity<SliderState>,
}

const NOT_PLAYABLE_TOOLTIP: &str = "Only WAV playback is available";

fn time_label(text: String, cx: &App) -> Div {
    div()
        .flex_none()
        .min_w(px(48.))
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(text)
}

/// Round Play/Pause button, current time, seek bar, total time.
fn player_card(card: PlayerCard<'_>, play_pause: &ClickAction, cx: &App) -> Div {
    let PlayerCard {
        playing,
        playable,
        position_ms,
        duration_ms,
        seek,
    } = card;
    let label = if playing { "Pause" } else { "Play" };
    let tooltip = if playable {
        label
    } else {
        NOT_PLAYABLE_TOOLTIP
    };
    let total = duration_ms.map_or_else(|| "—".to_string(), format_duration_ms);
    h_flex()
        .flex_shrink_0()
        .h(px(52.))
        .gap_3()
        .px_3()
        .items_center()
        .border_1()
        .border_color(cx.theme().border)
        .rounded(px(10.))
        .child(
            Button::new("play-pause")
                .primary()
                .rounded(px(20.))
                .icon(Icon::default().data(if playing { PAUSE_SVG } else { PLAY_SVG }))
                .tooltip(tooltip)
                .accessibility_label(label)
                .disabled(!playable)
                .on_click(click(play_pause)),
        )
        .child(time_label(format_duration_ms(position_ms), cx))
        .child(
            Slider::new(seek)
                .flex_1()
                .disabled(!playable || duration_ms.is_none())
                .accessibility_label("Seek"),
        )
        .child(time_label(total, cx))
}

fn click(action: &ClickAction) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    let action = action.clone();
    move |event, window, cx| action(event, window, cx)
}

fn note_line(text: SharedString, cx: &App) -> Div {
    h_flex()
        .gap_2()
        .items_start()
        .text_sm()
        .text_color(cx.theme().warning)
        .child(Icon::new(IconName::TriangleAlert).small().flex_shrink_0())
        .child(div().flex_1().min_w_0().child(text))
}

/// Name, detail line, notes, and the icon actions for the selected recording.
fn title_row(
    row: &RecordingRow,
    actions: &RecordingsActions,
    can_transcribe_again: bool,
    cx: &App,
) -> Div {
    let show_in_folder = actions.open_folder.clone();
    let transcribe_again = actions.transcribe_again.clone();
    v_flex()
        .gap_1()
        .child(
            h_flex()
                .group("recording-title")
                .gap_1()
                .items_center()
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_xl()
                        .font_semibold()
                        .child(row.name.clone()),
                )
                .child(
                    div()
                        .flex_none()
                        .opacity(0.)
                        .group_hover("recording-title", |style| style.opacity(1.))
                        .child(
                            Button::new("rename-recording")
                                .ghost()
                                .small()
                                .icon(Icon::default().data(PENCIL_SVG))
                                .tooltip("Rename recording")
                                .accessibility_label("Rename recording")
                                .on_click(click(&actions.rename)),
                        ),
                )
                .child(div().flex_1())
                .child(
                    Button::new("delete-recording")
                        .ghost()
                        .icon(Icon::default().data(TRASH_SVG))
                        .tooltip("Delete recording")
                        .accessibility_label("Delete recording")
                        .on_click(click(&actions.delete)),
                )
                .child(
                    Button::new("recording-more")
                        .ghost()
                        .icon(IconName::Ellipsis)
                        .tooltip("More")
                        .accessibility_label("More actions")
                        .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _window, _cx| {
                            let open = show_in_folder.clone();
                            let again = transcribe_again.clone();
                            menu.item(
                                PopupMenuItem::new("Transcribe again…")
                                    .icon(IconName::RefreshCw)
                                    .disabled(!can_transcribe_again)
                                    .on_click(move |event, window, cx| again(event, window, cx)),
                            )
                            .item(
                                PopupMenuItem::new("Show in folder")
                                    .icon(IconName::FolderOpen)
                                    .on_click(move |event, window, cx| open(event, window, cx)),
                            )
                        }),
                ),
        )
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(row.detail_line.clone()),
        )
        .children(row.notes.iter().map(|note| note_line(note.clone(), cx)))
}

fn pane_text(title: &'static str, description: Option<SharedString>, cx: &App) -> Div {
    v_flex()
        .gap_1()
        .child(div().font_semibold().child(title))
        .when_some(description, |el, text| {
            el.child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(text),
            )
        })
}

fn cancel_button(actions: &RecordingsActions) -> Option<Button> {
    actions.cancel_job.as_ref().map(|cancel| {
        Button::new("cancel-transcription")
            .label("Cancel")
            .on_click(click(cancel))
    })
}

fn pane_frame() -> Div {
    v_flex().gap_3().py_6().items_start()
}

fn no_transcript_pane(actions: &RecordingsActions, cx: &App) -> Div {
    pane_frame()
        .child(pane_text(
            "No transcript yet",
            Some("Runs on this PC. Your audio never leaves it.".into()),
            cx,
        ))
        .child(
            Button::new("transcribe-recording")
                .primary()
                .label("Transcribe")
                .on_click(click(&actions.transcribe)),
        )
}

fn queued_pane(actions: &RecordingsActions, cx: &App) -> Div {
    pane_frame()
        .child(pane_text(
            "Queued",
            Some("Starts when the current transcription finishes.".into()),
            cx,
        ))
        .children(cancel_button(actions))
}

fn running_pane(percent: u8, actions: &RecordingsActions, cx: &App) -> Div {
    pane_frame()
        .child(pane_text(
            "Transcribing",
            Some(format!("{percent}%").into()),
            cx,
        ))
        .child(
            Progress::new("transcription-progress")
                .value(f32::from(percent))
                .w(px(280.))
                .accessibility_label("Transcription progress"),
        )
        .children(cancel_button(actions))
}

fn failed_pane(reason: Option<SharedString>, actions: &RecordingsActions, cx: &App) -> Div {
    pane_frame()
        .child(
            pane_text("Transcription failed", reason, cx).child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("The recording itself is intact."),
            ),
        )
        .child(
            Button::new("retry-transcription")
                .primary()
                .label("Retry")
                .on_click(click(&actions.retry)),
        )
}

fn unreadable_pane(reason: SharedString, cx: &App) -> Div {
    pane_frame().child(pane_text("Could not open the transcript", Some(reason), cx))
}

fn ready_pane(list: Option<AnyElement>, actions: &RecordingsActions) -> Div {
    let copy = actions.copy_transcript.clone();
    v_flex()
        .flex_1()
        .min_h_0()
        .gap_2()
        .child(
            h_flex()
                .flex_shrink_0()
                .items_center()
                .child(div().font_semibold().child("Transcript"))
                .child(div().flex_1())
                .child(
                    Clipboard::new("copy-transcript")
                        .tooltip("Copy transcript")
                        .accessibility_label("Copy transcript")
                        .value_fn(move |window, cx| copy(window, cx)),
                ),
        )
        .child(div().flex_1().min_h_0().children(list))
}

/// The area under the player: exactly one state.
fn transcript_pane_view(
    pane: TranscriptPane,
    list: Option<AnyElement>,
    actions: &RecordingsActions,
    cx: &App,
) -> Div {
    match pane {
        TranscriptPane::NoTranscript => no_transcript_pane(actions, cx),
        TranscriptPane::Queued => queued_pane(actions, cx),
        TranscriptPane::Running { percent } => running_pane(percent, actions, cx),
        TranscriptPane::Failed { reason } => failed_pane(reason, actions, cx),
        TranscriptPane::Ready => ready_pane(list, actions),
        TranscriptPane::Unreadable { reason } => unreadable_pane(reason, cx),
    }
}

pub(crate) fn recordings_tab(
    view: RecordingsView<'_>,
    actions: RecordingsActions,
    cx: &App,
) -> impl IntoElement {
    let RecordingsView {
        list,
        count,
        selected,
        player,
        error,
        pane,
        can_transcribe_again,
        transcript_list,
    } = view;
    let list =
        if let Some(message) = error {
            Empty::new()
                .header(
                    EmptyHeader::new()
                        .title(EmptyTitle::new().child("Recordings unavailable"))
                        .description(EmptyDescription::new().child(message)),
                )
                .into_any_element()
        } else if let Some(list) = list {
            list
        } else {
            Empty::new()
                .header(
                    EmptyHeader::new()
                        .title(EmptyTitle::new().child("No recordings"))
                        .description(EmptyDescription::new().child(
                            "Saved recordings will appear here. Record from the Record tab.",
                        )),
                )
                .into_any_element()
        };

    let detail = match selected {
        Some(row) => v_flex()
            .size_full()
            .min_h_0()
            .gap_3()
            .child(title_row(row, &actions, can_transcribe_again, cx))
            .child(player_card(player, &actions.play, cx))
            .child(transcript_pane_view(pane, transcript_list, &actions, cx))
            .into_any_element(),
        None => Empty::new()
            .header(
                EmptyHeader::new()
                    .title(EmptyTitle::new().child("Select a recording"))
                    .description(
                        EmptyDescription::new().child("Choose a row to play, rename, or delete."),
                    ),
            )
            .into_any_element(),
    };

    v_flex().size_full().min_h_0().child(
        h_flex()
            .flex_1()
            .min_h_0()
            .items_stretch()
            .gap_3()
            .overflow_hidden()
            .child(
                v_flex()
                    .flex_none()
                    .w(px(LIST_WIDTH_PX))
                    .min_h_0()
                    .h_full()
                    .overflow_hidden()
                    .gap_1()
                    .child(
                        h_flex()
                            .flex_shrink_0()
                            .px_3()
                            .py_1()
                            .gap_2()
                            .items_center()
                            .child(div().font_semibold().child("Recordings"))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(count.to_string()),
                            )
                            .child(div().flex_1())
                            .child(
                                Button::new("refresh-library")
                                    .ghost()
                                    .icon(IconName::RefreshCw)
                                    .tooltip("Refresh")
                                    .accessibility_label("Refresh recordings")
                                    .on_click(click(&actions.refresh)),
                            )
                            .child(
                                Button::new("open-recordings-folder-tab")
                                    .ghost()
                                    .icon(IconName::FolderOpen)
                                    .tooltip("Open recordings folder")
                                    .accessibility_label("Open recordings folder")
                                    .on_click(click(&actions.open_folder)),
                            ),
                    )
                    .child(div().flex_1().min_h_0().child(list)),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w(px(200.))
                    .min_h_0()
                    .h_full()
                    .overflow_hidden()
                    .gap_2()
                    .child(detail),
            ),
    )
}

pub(crate) fn open_rename_dialog(
    window: &mut Window,
    cx: &mut App,
    current: SharedString,
    on_confirm: impl Fn(String, &mut Window, &mut App) + Clone + 'static,
) {
    let input = cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder("Recording name")
            .default_value(current.to_string())
    });
    window.open_dialog(cx, move |dialog, _, cx| {
        let input = input.clone();
        let on_confirm = on_confirm.clone();
        dialog
            .title("Rename recording")
            .w(DIALOG_WIDTH)
            .child(Input::new(&input))
            .on_ok({
                let input = input.clone();
                let on_confirm = on_confirm.clone();
                move |_, window, cx| {
                    let value = input.read(cx).value().to_string();
                    on_confirm(value, window, cx);
                    true
                }
            })
            .footer(
                dialog_footer(cx)
                    .child(
                        DialogClose::new()
                            .child(Button::new("rename-cancel").label("Cancel").outline()),
                    )
                    .child(
                        DialogAction::new()
                            .child(Button::new("rename-ok").label("Rename").primary()),
                    ),
            )
    });
}

pub(crate) fn open_delete_dialog(
    window: &mut Window,
    cx: &mut App,
    name: SharedString,
    on_confirm: impl Fn(&mut Window, &mut App) + Clone + 'static,
) {
    window.open_alert_dialog(cx, move |alert, _, _| {
        let on_confirm = on_confirm.clone();
        let description = format!("Send “{name}” and its related files to the Recycle Bin?");
        alert
            .title("Delete recording?")
            .w(DIALOG_WIDTH)
            .description(description)
            .confirm()
            .ok_text("Delete")
            .ok_variant(ButtonVariant::Danger)
            .on_ok(move |_, window, cx| {
                on_confirm(window, cx);
                true
            })
    });
}

pub(crate) fn open_transcribe_again_dialog(
    window: &mut Window,
    cx: &mut App,
    model_name: SharedString,
    on_confirm: impl Fn(&mut Window, &mut App) + Clone + 'static,
) {
    window.open_alert_dialog(cx, move |alert, _, _| {
        let on_confirm = on_confirm.clone();
        alert
            .title("Replace this transcript?")
            .w(DIALOG_WIDTH)
            .description(transcribe_again_description(&model_name))
            .confirm()
            .ok_text("Replace")
            .on_ok(move |_, window, cx| {
                on_confirm(window, cx);
                true
            })
    });
}

pub(crate) fn rows_from_catalog(catalog: &LibraryCatalog) -> Vec<RecordingRow> {
    let (zone, today) = local_clock();
    catalog
        .entries
        .iter()
        .map(|entry| RecordingRow::from_info_in(&entry.info, &zone, today))
        .collect()
}

/// The one state shown below the player.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TranscriptPane {
    NoTranscript,
    Queued,
    Running { percent: u8 },
    Failed { reason: Option<SharedString> },
    Ready,
    Unreadable { reason: SharedString },
}

/// Pick the pane for the selected recording.
///
/// `failure` is the reason for this recording's failure, if any. `read_error` is set
/// when a Ready transcript could not be read.
pub(crate) fn transcript_pane(
    status: TranscriptStatus,
    job_active: bool,
    failure: Option<SharedString>,
    read_error: Option<SharedString>,
) -> TranscriptPane {
    match status {
        TranscriptStatus::Queued => TranscriptPane::Queued,
        TranscriptStatus::Progress(percent) => TranscriptPane::Running { percent },
        // A job that has not reported yet still shows as queued.
        TranscriptStatus::None | TranscriptStatus::Failed if job_active => TranscriptPane::Queued,
        TranscriptStatus::Failed => TranscriptPane::Failed { reason: failure },
        TranscriptStatus::Ready => match read_error {
            Some(reason) => TranscriptPane::Unreadable { reason },
            None => TranscriptPane::Ready,
        },
        TranscriptStatus::None => TranscriptPane::NoTranscript,
    }
}

/// The `⋯` menu offers Transcribe again only for a Ready recording with no active job.
pub(crate) fn can_transcribe_again(status: TranscriptStatus, job_active: bool) -> bool {
    status == TranscriptStatus::Ready && !job_active
}

/// Body of the "Replace this transcript?" dialog.
pub(crate) fn transcribe_again_description(model_name: &str) -> String {
    format!(
        "The current transcript stays until the new one is ready, then it is replaced.          Transcribing uses the {model_name} model and runs on this PC."
    )
}

/// The failure reason belongs to the recording that failed, not to whichever is selected.
pub(crate) fn failure_reason(
    detail: &str,
    failed_recording: Option<RecordingId>,
    selected: Option<RecordingId>,
) -> Option<SharedString> {
    if detail.is_empty() || selected.is_none() || failed_recording != selected {
        return None;
    }
    Some(SharedString::from(detail.to_string()))
}

/// How long after the user scrolls the transcript before playback may scroll it again.
pub(crate) const AUTO_SCROLL_PAUSE: std::time::Duration = std::time::Duration::from_secs(5);

/// Whether the list may follow playback, given the time since the user last scrolled it.
pub(crate) fn may_auto_scroll(since_user_scroll: Option<std::time::Duration>) -> bool {
    since_user_scroll.is_none_or(|elapsed| elapsed >= AUTO_SCROLL_PAUSE)
}

/// Slider resolution. The slider runs 0..=SEEK_STEPS and maps to the recording length.
pub(crate) const SEEK_STEPS: f32 = 100_000.;

/// What the round Play/Pause button does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PlayAction {
    Play { start_ms: u64 },
    Pause,
    Resume,
}

/// Idle (or another recording, or an error) plays from the slider position, Playing
/// pauses, and Paused resumes.
pub(crate) fn play_action(
    phase: PlayerPhase,
    snapshot_is_selected: bool,
    slider_position_ms: u64,
) -> PlayAction {
    match phase {
        PlayerPhase::Playing if snapshot_is_selected => PlayAction::Pause,
        PlayerPhase::Paused if snapshot_is_selected => PlayAction::Resume,
        _ => PlayAction::Play {
            start_ms: slider_position_ms,
        },
    }
}

/// Position to show for the selected recording: the drag position while dragging, the
/// player position when the snapshot is for this recording, and otherwise the resting
/// position (0 unless the user moved the slider while idle).
pub(crate) fn shown_position_ms(
    snapshot_is_selected: bool,
    snapshot_position_ms: u64,
    resting_ms: u64,
    seek_drag: Option<u64>,
) -> u64 {
    seek_drag.unwrap_or(if snapshot_is_selected {
        snapshot_position_ms
    } else {
        resting_ms
    })
}

/// Slider value for a position. Zero when the length is unknown.
pub(crate) fn seek_value(position_ms: u64, duration_ms: Option<u64>) -> f32 {
    match duration_ms {
        Some(length) if length > 0 => {
            (position_ms.min(length) as f64 / length as f64 * f64::from(SEEK_STEPS)) as f32
        }
        _ => 0.,
    }
}

/// Position for a slider value, clamped to the recording length.
pub(crate) fn seek_position_ms(value: f32, duration_ms: Option<u64>) -> u64 {
    let Some(length) = duration_ms else {
        return 0;
    };
    let fraction = f64::from(value.clamp(0., SEEK_STEPS)) / f64::from(SEEK_STEPS);
    ((fraction * length as f64) as u64).min(length)
}

#[cfg(test)]
mod tests {
    use super::{
        library_items, may_auto_scroll, play_action, seek_position_ms, seek_value, set_row_status,
        shown_position_ms, LibraryItem, PlayAction, RecordingRow, RowBadge, HEADER_HEIGHT_PX,
        ROW_HEIGHT_PX,
    };
    use crate::domain::ids::RecordingId;
    use crate::domain::recording::{
        RecordingContainer, RecordingDay, RecordingInfo, SidecarState, TranscriptStatus,
    };
    use crate::services::playback::PlayerPhase;
    use gpui_kit::SharedString;
    use std::time::Duration;

    #[test]
    fn row_maps_metadata_for_display() {
        // Arrange
        let info = RecordingInfo::new(
            RecordingId::from_raw(1),
            "clip",
            RecordingContainer::Wav,
            Some(0),
            Some(65_000),
            2048,
            None,
            TranscriptStatus::Ready,
            SidecarState::Present,
            true,
            vec!["gap".into()],
        );

        // Act
        let row = RecordingRow::from_info_in(&info, &jiff::tz::TimeZone::UTC, today_utc());

        // Assert
        assert_eq!(row.name.as_ref(), "clip");
        assert_eq!(row.badge, RowBadge::Recovered);
        assert_eq!(row.notes.len(), 2);
        assert_eq!(
            row.detail_line.as_ref(),
            "Jan 1, 00:00 · 01:05 · 2.0 KB · WAV"
        );
    }

    fn ms(utc: &str) -> u64 {
        let ts: jiff::Timestamp = utc.parse().expect("timestamp");
        u64::try_from(ts.as_millisecond()).expect("positive")
    }

    fn today_utc() -> crate::domain::audio::CivilTime {
        crate::domain::audio::CivilTime::new(2026, 11, 7, 12, 0, 0).expect("civil")
    }

    fn info_at(
        started: Option<u64>,
        transcript: TranscriptStatus,
        recovered: bool,
    ) -> RecordingInfo {
        RecordingInfo::new(
            RecordingId::from_raw(1),
            "clip",
            RecordingContainer::Wav,
            started,
            Some(761_000),
            38 * 1024 * 1024,
            None,
            transcript,
            SidecarState::Present,
            recovered,
            Vec::new(),
        )
    }

    #[test]
    fn civil_conversion_uses_the_given_zone() {
        use super::civil_from_unix_ms;
        use jiff::tz::{self, TimeZone};

        // Arrange
        let zone = TimeZone::fixed(tz::offset(2));

        // Act
        let civil = civil_from_unix_ms(0, &zone).expect("in range");

        // Assert
        assert_eq!(
            (civil.year, civil.month, civil.day, civil.hour, civil.minute),
            (1970, 1, 1, 2, 0)
        );
    }

    #[test]
    fn civil_conversion_follows_daylight_saving_changes() {
        use super::civil_from_unix_ms;
        use jiff::tz::TimeZone;

        // Arrange
        let zone = TimeZone::get("America/New_York").expect("tz database");
        let before = ms("2026-03-08T06:59:00Z");
        let after = ms("2026-03-08T07:01:00Z");

        // Act
        let before = civil_from_unix_ms(before, &zone).expect("in range");
        let after = civil_from_unix_ms(after, &zone).expect("in range");

        // Assert
        assert_eq!((before.hour, before.minute), (1, 59));
        assert_eq!((after.hour, after.minute), (3, 1));
    }

    #[test]
    fn civil_conversion_rejects_out_of_range_timestamps() {
        use super::{civil_from_unix_ms, TimestampOutOfRange};
        use jiff::tz::TimeZone;

        // Arrange
        let zone = TimeZone::UTC;

        // Act
        let too_big = civil_from_unix_ms(u64::MAX, &zone);
        let beyond_calendar = civil_from_unix_ms(i64::MAX as u64, &zone);

        // Assert
        assert_eq!(too_big, Err(TimestampOutOfRange));
        assert_eq!(beyond_calendar, Err(TimestampOutOfRange));
    }

    #[test]
    fn row_text_for_today_and_earlier_recordings() {
        use jiff::tz::TimeZone;

        // Arrange
        let today_info = info_at(
            Some(ms("2026-11-07T14:32:00Z")),
            TranscriptStatus::None,
            false,
        );
        let earlier_info = info_at(
            Some(ms("2026-11-03T09:05:00Z")),
            TranscriptStatus::None,
            false,
        );
        let yesterday_info = info_at(
            Some(ms("2026-11-06T23:59:00Z")),
            TranscriptStatus::None,
            false,
        );

        // Act
        let today_row = RecordingRow::from_info_in(&today_info, &TimeZone::UTC, today_utc());
        let earlier_row = RecordingRow::from_info_in(&earlier_info, &TimeZone::UTC, today_utc());
        let yesterday_row =
            RecordingRow::from_info_in(&yesterday_info, &TimeZone::UTC, today_utc());

        // Assert
        assert_eq!(today_row.day, RecordingDay::Today);
        assert_eq!(today_row.meta.as_ref(), "14:32 · 12:41 · 38.0 MB");
        assert_eq!(
            today_row.detail_line.as_ref(),
            "Today, 14:32 · 12:41 · 38.0 MB · WAV"
        );
        assert_eq!(yesterday_row.day, RecordingDay::Yesterday);
        assert_eq!(
            yesterday_row.detail_line.as_ref(),
            "Yesterday, 23:59 · 12:41 · 38.0 MB · WAV"
        );
        assert_eq!(earlier_row.day, RecordingDay::Earlier);
        assert_eq!(earlier_row.meta.as_ref(), "Nov 3 · 12:41 · 38.0 MB");
        assert_eq!(
            earlier_row.detail_line.as_ref(),
            "Nov 3, 09:05 · 12:41 · 38.0 MB · WAV"
        );
    }

    #[test]
    fn row_groups_by_local_day_not_utc_day() {
        use jiff::tz::{self, TimeZone};

        // Arrange: 23:30 UTC on Nov 6 is 01:30 on Nov 7 at UTC+2.
        let zone = TimeZone::fixed(tz::offset(2));
        let info = info_at(
            Some(ms("2026-11-06T23:30:00Z")),
            TranscriptStatus::None,
            false,
        );

        // Act
        let row = RecordingRow::from_info_in(&info, &zone, today_utc());

        // Assert
        assert_eq!(row.day, RecordingDay::Today);
        assert_eq!(row.meta.as_ref(), "01:30 · 12:41 · 38.0 MB");
    }

    #[test]
    fn row_without_start_time_is_earlier_and_shows_dash() {
        use jiff::tz::TimeZone;

        // Arrange
        let info = info_at(None, TranscriptStatus::None, false);

        // Act
        let row = RecordingRow::from_info_in(&info, &TimeZone::UTC, today_utc());

        // Assert
        assert_eq!(row.day, RecordingDay::Earlier);
        assert_eq!(row.meta.as_ref(), "— · 12:41 · 38.0 MB");
        assert_eq!(row.detail_line.as_ref(), "— · 12:41 · 38.0 MB · WAV");
    }

    #[test]
    fn notes_cover_recovered_sidecar_and_problems() {
        use super::{recording_notes, NOTE_PROBLEMS, NOTE_RECOVERED};

        // Arrange
        let clean = (false, SidecarState::Present, false);

        // Act
        let none = recording_notes(clean.0, clean.1, clean.2);
        let recovered = recording_notes(true, SidecarState::Present, false);
        let missing = recording_notes(false, SidecarState::Missing, false);
        let malformed = recording_notes(false, SidecarState::Malformed, false);
        let problems = recording_notes(false, SidecarState::Present, true);
        let all = recording_notes(true, SidecarState::Missing, true);

        // Assert
        assert!(none.is_empty());
        assert_eq!(recovered, [NOTE_RECOVERED]);
        assert_eq!(missing.len(), 1);
        assert_eq!(malformed.len(), 1);
        assert_ne!(missing, malformed);
        assert_eq!(problems, [NOTE_PROBLEMS]);
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn row_carries_notes_from_the_recording() {
        use super::NOTE_RECOVERED;
        use jiff::tz::TimeZone;

        // Arrange
        let recovered = info_at(None, TranscriptStatus::None, true);
        let clean = info_at(None, TranscriptStatus::None, false);

        // Act
        let recovered_row = RecordingRow::from_info_in(&recovered, &TimeZone::UTC, today_utc());
        let clean_row = RecordingRow::from_info_in(&clean, &TimeZone::UTC, today_utc());

        // Assert
        assert_eq!(recovered_row.notes, [SharedString::from(NOTE_RECOVERED)]);
        assert!(clean_row.notes.is_empty());
    }

    #[test]
    fn badge_maps_status_and_recovered_flag() {
        // Arrange
        let cases = [
            (TranscriptStatus::None, false, RowBadge::None),
            (TranscriptStatus::None, true, RowBadge::Recovered),
            (TranscriptStatus::Ready, false, RowBadge::Ready),
            (TranscriptStatus::Ready, true, RowBadge::Recovered),
            (TranscriptStatus::Queued, true, RowBadge::Queued),
            (TranscriptStatus::Progress(42), true, RowBadge::Progress(42)),
            (TranscriptStatus::Failed, true, RowBadge::Failed),
            (TranscriptStatus::Failed, false, RowBadge::Failed),
        ];

        // Act
        let actual = cases.map(|(status, recovered, _)| RowBadge::from_status(status, recovered));

        // Assert
        assert_eq!(actual, cases.map(|(_, _, badge)| badge));
    }

    #[test]
    fn transcribe_again_is_enabled_only_for_ready_without_a_job() {
        use super::{can_transcribe_again, transcribe_again_description};
        // Arrange
        let statuses = [
            TranscriptStatus::None,
            TranscriptStatus::Queued,
            TranscriptStatus::Progress(10),
            TranscriptStatus::Failed,
            TranscriptStatus::Ready,
        ];

        for status in statuses {
            for job_active in [false, true] {
                // Act
                let enabled = can_transcribe_again(status, job_active);

                // Assert
                let expected = status == TranscriptStatus::Ready && !job_active;
                assert_eq!(enabled, expected, "{status:?} {job_active}");
            }
        }
        assert!(transcribe_again_description("Base").contains("Base"));
    }

    #[test]
    fn pane_follows_status() {
        use super::{transcript_pane, TranscriptPane};

        // Arrange
        let reason = Some(SharedString::from("model crashed"));

        // Act
        let none = transcript_pane(TranscriptStatus::None, false, None, None);
        let queued = transcript_pane(TranscriptStatus::Queued, true, None, None);
        let running = transcript_pane(TranscriptStatus::Progress(42), true, None, None);
        let failed = transcript_pane(TranscriptStatus::Failed, false, reason.clone(), None);
        let ready = transcript_pane(TranscriptStatus::Ready, false, None, None);

        // Assert
        assert_eq!(none, TranscriptPane::NoTranscript);
        assert_eq!(queued, TranscriptPane::Queued);
        assert_eq!(running, TranscriptPane::Running { percent: 42 });
        assert_eq!(failed, TranscriptPane::Failed { reason });
        assert_eq!(ready, TranscriptPane::Ready);
    }

    #[test]
    fn pane_with_an_active_job_and_no_status_is_queued() {
        use super::{transcript_pane, TranscriptPane};

        // Arrange / Act
        let none = transcript_pane(TranscriptStatus::None, true, None, None);
        let failed = transcript_pane(TranscriptStatus::Failed, true, None, None);
        let ready = transcript_pane(TranscriptStatus::Ready, true, None, None);

        // Assert
        assert_eq!(none, TranscriptPane::Queued);
        assert_eq!(failed, TranscriptPane::Queued);
        assert_eq!(ready, TranscriptPane::Ready);
    }

    #[test]
    fn unreadable_transcript_on_a_ready_recording_shows_the_reason() {
        use super::{transcript_pane, TranscriptPane};

        // Arrange
        let error = SharedString::from("The transcript could not be read.");

        // Act
        let pane = transcript_pane(TranscriptStatus::Ready, false, None, Some(error.clone()));
        let ignored = transcript_pane(TranscriptStatus::None, false, None, Some(error.clone()));

        // Assert
        assert_eq!(pane, TranscriptPane::Unreadable { reason: error });
        assert_eq!(ignored, TranscriptPane::NoTranscript);
    }

    #[test]
    fn failure_reason_belongs_to_the_failed_recording_only() {
        use super::failure_reason;

        // Arrange
        let failed = Some(RecordingId::from_raw(1));
        let other = Some(RecordingId::from_raw(2));

        // Act
        let own = failure_reason("disk full", failed, failed);
        let foreign = failure_reason("disk full", failed, other);
        let empty = failure_reason("", failed, failed);
        let nothing_selected = failure_reason("disk full", failed, None);

        // Assert
        assert_eq!(own.as_deref(), Some("disk full"));
        assert_eq!(foreign, None);
        assert_eq!(empty, None);
        assert_eq!(nothing_selected, None);
    }

    fn row_for(id: u64, name: &str, started: Option<u64>) -> RecordingRow {
        let info = RecordingInfo::new(
            RecordingId::from_raw(id),
            name,
            RecordingContainer::Wav,
            started,
            Some(1_000),
            1,
            None,
            TranscriptStatus::None,
            SidecarState::Present,
            false,
            Vec::new(),
        );
        RecordingRow::from_info_in(&info, &jiff::tz::TimeZone::UTC, today_utc())
    }

    fn item_names(items: &[LibraryItem]) -> Vec<String> {
        items
            .iter()
            .map(|item| match item {
                LibraryItem::Header(day) => format!("#{day:?}"),
                LibraryItem::Recording(row) => row.name.to_string(),
            })
            .collect()
    }

    #[test]
    fn items_have_a_header_only_for_days_with_recordings() {
        // Arrange
        let rows = vec![
            row_for(1, "a", Some(ms("2026-11-07T10:00:00Z"))),
            row_for(2, "b", Some(ms("2026-11-07T09:00:00Z"))),
            row_for(3, "c", Some(ms("2026-10-01T09:00:00Z"))),
        ];

        // Act
        let items = library_items(&rows);

        // Assert
        assert_eq!(
            item_names(&items),
            ["#Today", "a", "b", "#Earlier", "c"].map(String::from)
        );
    }

    #[test]
    fn items_keep_catalog_order_and_handle_an_empty_catalog() {
        // Arrange
        let rows = vec![
            row_for(1, "newest", Some(ms("2026-11-06T10:00:00Z"))),
            row_for(2, "middle", Some(ms("2026-11-03T10:00:00Z"))),
            row_for(3, "oldest", Some(ms("2026-10-01T10:00:00Z"))),
        ];

        // Act
        let items = library_items(&rows);
        let empty = library_items(&[]);

        // Assert
        assert_eq!(
            item_names(&items),
            ["#Yesterday", "newest", "#Earlier", "middle", "oldest"].map(String::from)
        );
        assert!(empty.is_empty());
    }

    #[test]
    fn progress_tick_updates_one_row_without_reordering() {
        // Arrange
        let mut rows = vec![
            row_for(1, "alpha", Some(ms("2026-11-07T10:00:00Z"))),
            row_for(2, "beta", Some(ms("2026-11-07T09:00:00Z"))),
        ];

        // Act
        let updated = set_row_status(
            &mut rows,
            RecordingId::from_raw(2),
            TranscriptStatus::Progress(42),
        );
        let missing = set_row_status(&mut rows, RecordingId::from_raw(9), TranscriptStatus::Ready);

        // Assert
        assert!(updated);
        assert!(!missing);
        let names: Vec<_> = rows.iter().map(|row| row.name.to_string()).collect();
        assert_eq!(names, ["alpha", "beta"]);
        assert_eq!(rows[0].badge, RowBadge::None);
        assert_eq!(rows[1].badge, RowBadge::Progress(42));
    }

    #[test]
    fn item_heights_differ_for_headers_and_rows() {
        // Arrange
        let items = library_items(&[row_for(1, "a", Some(ms("2026-11-07T10:00:00Z")))]);

        // Act
        let heights: Vec<_> = items.iter().map(LibraryItem::height_px).collect();

        // Assert
        assert_eq!(heights, [HEADER_HEIGHT_PX, ROW_HEIGHT_PX]);
    }

    #[test]
    fn play_action_maps_each_phase_to_a_command() {
        // Arrange
        let slider_ms = 42_000;

        // Act
        let idle = play_action(PlayerPhase::Idle, false, slider_ms);
        let playing = play_action(PlayerPhase::Playing, true, slider_ms);
        let paused = play_action(PlayerPhase::Paused, true, slider_ms);
        let failed = play_action(PlayerPhase::Error, false, slider_ms);

        // Assert
        assert_eq!(idle, PlayAction::Play { start_ms: 42_000 });
        assert_eq!(playing, PlayAction::Pause);
        assert_eq!(paused, PlayAction::Resume);
        assert_eq!(failed, PlayAction::Play { start_ms: 42_000 });
    }

    #[test]
    fn play_action_plays_when_the_snapshot_is_for_another_recording() {
        // Arrange
        let slider_ms = 5_000;

        // Act
        let action = play_action(PlayerPhase::Playing, false, slider_ms);

        // Assert
        assert_eq!(action, PlayAction::Play { start_ms: 5_000 });
    }

    #[test]
    fn position_is_zero_when_the_snapshot_is_for_another_recording() {
        // Arrange / Act
        let other = shown_position_ms(false, 9_000, 0, None);
        let own = shown_position_ms(true, 9_000, 0, None);

        // Assert
        assert_eq!(other, 0);
        assert_eq!(own, 9_000);
    }

    #[test]
    fn resting_position_shows_when_idle() {
        // Arrange / Act
        let shown = shown_position_ms(false, 0, 12_000, None);

        // Assert
        assert_eq!(shown, 12_000);
    }

    #[test]
    fn auto_scroll_waits_five_seconds_after_the_user_scrolls() {
        // Arrange
        let never = None;
        let just_now = Some(Duration::from_millis(4_999));
        let later = Some(Duration::from_secs(5));

        // Act / Assert
        assert!(may_auto_scroll(never));
        assert!(!may_auto_scroll(just_now));
        assert!(may_auto_scroll(later));
    }

    #[test]
    fn drag_position_wins_over_playback_position() {
        // Arrange / Act
        let shown = shown_position_ms(true, 9_000, 0, Some(30_000));

        // Assert
        assert_eq!(shown, 30_000);
    }

    #[test]
    fn seek_value_and_position_round_trip() {
        // Arrange
        let duration = Some(4_000_000);

        // Act
        let value = seek_value(1_000_000, duration);
        let position = seek_position_ms(value, duration);

        // Assert
        assert!((value - 25_000.).abs() < 0.5);
        assert!(position.abs_diff(1_000_000) <= 40);
    }

    #[test]
    fn seek_handles_unknown_length_and_overshoot() {
        // Arrange / Act
        let value = seek_value(5_000, None);
        let position = seek_position_ms(50_000., None);
        let clamped = seek_position_ms(1_000_000., Some(10_000));
        let past_end = seek_value(20_000, Some(10_000));

        // Assert
        assert_eq!(value, 0.);
        assert_eq!(position, 0);
        assert_eq!(clamped, 10_000);
        assert_eq!(past_end, 100_000.);
    }
}
