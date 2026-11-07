//! Recordings tab: library table, detail pane, playback and file actions.

use std::ops::Range;

use crate::ui::components::dialog_style::{dialog_footer, DIALOG_WIDTH};
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::dialog::{DialogAction, DialogClose};
use gpui_kit::component::empty::{Empty, EmptyDescription, EmptyHeader, EmptyTitle};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::table::{Column, ColumnSort, DataTable, TableDelegate, TableState};
use gpui_kit::component::{h_flex, v_flex, Disableable as _, WindowExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    div, px, AnyElement, App, AppContext as _, ClickEvent, Context, Entity, IntoElement,
    ParentElement, SharedString, Styled, Window,
};

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
    pub(crate) date: SharedString,
    pub(crate) duration: SharedString,
    pub(crate) size: SharedString,
    pub(crate) transcript: SharedString,
    pub(crate) detail: SharedString,
    // The grouped list and detail header read these from phase 1 on.
    #[allow(dead_code)]
    pub(crate) day: RecordingDay,
    #[allow(dead_code)]
    pub(crate) meta: SharedString,
    #[allow(dead_code)]
    pub(crate) detail_line: SharedString,
    #[allow(dead_code)]
    pub(crate) badge: RowBadge,
}

impl RecordingRow {
    /// Build a row using the system time zone and the current time.
    #[cfg(test)]
    pub(crate) fn from_info(info: &RecordingInfo) -> Self {
        let (zone, today) = local_clock();
        Self::from_info_in(info, &zone, today)
    }

    /// Build a row with an explicit time zone and "today", so tests are exact.
    pub(crate) fn from_info_in(info: &RecordingInfo, zone: &TimeZone, today: CivilTime) -> Self {
        let started = info
            .started_at_unix_ms()
            .and_then(|ms| civil_from_unix_ms(ms, zone).ok());
        let day = started.map_or(RecordingDay::Earlier, |time| recording_day(time, today));
        let date = started
            .map(|t| {
                format!(
                    "{:04}-{:02}-{:02} {:02}:{:02}",
                    t.year, t.month, t.day, t.hour, t.minute
                )
            })
            .unwrap_or_else(|| "—".to_string());
        let duration = info
            .duration_ms()
            .map(format_duration_ms)
            .unwrap_or_else(|| "—".to_string());
        let size = format_size(info.size_bytes());
        let mut detail = format!("Size {size}");
        if info.recovered() {
            detail.push_str(" · Recovered");
        }
        match info.sidecar() {
            SidecarState::Present => {}
            SidecarState::Missing => detail.push_str(" · Metadata missing"),
            SidecarState::Malformed => detail.push_str(" · Metadata unreadable"),
        }
        if !info.problems().is_empty() {
            detail.push_str(" · Had recording issues");
        }
        let meta = row_meta(day, started, &duration, &size);
        let detail_line = detail_line(day, started, &duration, &size, info.container());
        Self {
            id: info.id(),
            name: SharedString::from(info.name().to_string()),
            date: SharedString::from(date),
            duration: SharedString::from(duration),
            size: SharedString::from(size),
            transcript: SharedString::from(info.transcript().display()),
            detail: SharedString::from(detail),
            day,
            meta: SharedString::from(meta),
            detail_line: SharedString::from(detail_line),
            badge: RowBadge::from_status(info.transcript(), info.recovered()),
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

pub(crate) struct RecordingsDelegate {
    rows: Vec<RecordingRow>,
    columns: Vec<Column>,
}

impl RecordingsDelegate {
    pub(crate) fn set_transcript_display(
        &mut self,
        id: RecordingId,
        display: impl Into<SharedString>,
    ) -> bool {
        let Some(row) = self.rows.iter_mut().find(|row| row.id == id) else {
            return false;
        };
        row.transcript = display.into();
        true
    }

    pub(crate) fn new() -> Self {
        Self {
            rows: Vec::new(),
            columns: vec![
                Column::new("name", "Name").width(px(180.)).sortable(),
                Column::new("date", "Date")
                    .width(px(130.))
                    .sortable()
                    .descending(),
                Column::new("duration", "Duration")
                    .width(px(80.))
                    .sortable(),
                Column::new("size", "Size").width(px(80.)).sortable(),
                Column::new("transcript", "Transcript").width(px(90.)),
            ],
        }
    }

    pub(crate) fn set_rows(&mut self, rows: Vec<RecordingRow>) {
        self.rows = rows;
    }

    pub(crate) fn rows(&self) -> &[RecordingRow] {
        &self.rows
    }

    pub(crate) fn row_id(&self, index: usize) -> Option<RecordingId> {
        self.rows.get(index).map(|row| row.id)
    }

    pub(crate) fn index_of(&self, id: RecordingId) -> Option<usize> {
        self.rows.iter().position(|row| row.id == id)
    }
}

impl TableDelegate for RecordingsDelegate {
    fn columns_count(&self, _: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.rows.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        self.columns[col_ix].clone()
    }

    fn perform_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) {
        let key = self.columns.get(col_ix).map(|col| col.key.clone());
        let Some(key) = key else {
            return;
        };
        match key.as_ref() {
            "name" => match sort {
                ColumnSort::Ascending => self.rows.sort_by(|a, b| a.name.cmp(&b.name)),
                ColumnSort::Descending => self.rows.sort_by(|a, b| b.name.cmp(&a.name)),
                ColumnSort::Default => self.rows.sort_by(|a, b| b.date.cmp(&a.date)),
            },
            "date" => match sort {
                ColumnSort::Ascending => self.rows.sort_by(|a, b| a.date.cmp(&b.date)),
                ColumnSort::Descending | ColumnSort::Default => {
                    self.rows.sort_by(|a, b| b.date.cmp(&a.date))
                }
            },
            "duration" => match sort {
                ColumnSort::Ascending => self.rows.sort_by(|a, b| a.duration.cmp(&b.duration)),
                ColumnSort::Descending => self.rows.sort_by(|a, b| b.duration.cmp(&a.duration)),
                ColumnSort::Default => self.rows.sort_by(|a, b| b.date.cmp(&a.date)),
            },
            "size" => match sort {
                ColumnSort::Ascending => self.rows.sort_by(|a, b| a.size.cmp(&b.size)),
                ColumnSort::Descending => self.rows.sort_by(|a, b| b.size.cmp(&a.size)),
                ColumnSort::Default => self.rows.sort_by(|a, b| b.date.cmp(&a.date)),
            },
            _ => {}
        }
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let row = &self.rows[row_ix];
        let text = match self.columns[col_ix].key.as_ref() {
            "name" => row.name.clone(),
            "date" => row.date.clone(),
            "duration" => row.duration.clone(),
            "size" => row.size.clone(),
            "transcript" => row.transcript.clone(),
            _ => SharedString::from(""),
        };
        div().child(text)
    }

    fn visible_rows_changed(
        &mut self,
        _: Range<usize>,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) {
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn recordings_tab(
    table: &Entity<TableState<RecordingsDelegate>>,
    catalog: &LibraryCatalog,
    selected: Option<&RecordingRow>,
    playing: bool,
    play_label: SharedString,
    error: Option<SharedString>,
    refresh: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    open_folder: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    play: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    stop: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    rename: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    delete: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    transcript_view: AnyElement,
    controls: TranscriptControls,
    failure: Option<SharedString>,
    transcribe: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cancel_job: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    retry: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    open_model: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> impl IntoElement {
    let _ = cx;
    let list = if let Some(message) = error {
        Empty::new()
            .header(
                EmptyHeader::new()
                    .title(EmptyTitle::new().child("Recordings unavailable"))
                    .description(EmptyDescription::new().child(message)),
            )
            .into_any_element()
    } else if catalog.entries.is_empty() {
        Empty::new()
            .header(
                EmptyHeader::new()
                    .title(EmptyTitle::new().child("No recordings"))
                    .description(EmptyDescription::new().child(
                        "Saved recordings will appear here. Record from the Record tab or refresh.",
                    )),
            )
            .into_any_element()
    } else {
        DataTable::new(table)
            .stripe(true)
            .bordered(true)
            .into_any_element()
    };

    let detail = match selected {
        Some(row) => v_flex()
            .gap_2()
            .child(div().text_lg().child(row.name.clone()))
            .child(row.detail.clone())
            .child(format!("Date {}", row.date))
            .child(format!("Duration {}", row.duration))
            .child(format!("Transcript {}", row.transcript))
            .child(failure.unwrap_or_else(|| SharedString::from("")))
            .child(if playing {
                SharedString::from("Playing")
            } else {
                SharedString::from("")
            })
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("play-recording")
                            .primary()
                            .label(play_label)
                            .on_click(play),
                    )
                    .child(
                        Button::new("stop-recording-playback")
                            .label("Stop")
                            .disabled(!playing)
                            .on_click(stop),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("transcribe-recording")
                            .label("Transcribe")
                            .disabled(!controls.transcribe_enabled)
                            .on_click(transcribe),
                    )
                    .child(
                        Button::new("cancel-transcription")
                            .label("Cancel")
                            .disabled(!controls.cancel_enabled)
                            .on_click(cancel_job),
                    )
                    .when(controls.retry_enabled, |row| {
                        row.child(
                            Button::new("retry-transcription")
                                .label("Retry")
                                .on_click(retry),
                        )
                    })
                    .child(
                        Button::new("speech-model")
                            .label("Model")
                            .on_click(open_model),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("rename-recording")
                            .label("Rename")
                            .on_click(rename),
                    )
                    .child(
                        Button::new("delete-recording")
                            .danger()
                            .label("Delete")
                            .on_click(delete),
                    ),
            )
            .child(transcript_view)
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

    v_flex()
        .size_full()
        .min_h_0()
        .gap_3()
        .child(
            h_flex()
                .gap_2()
                .flex_shrink_0()
                .child(
                    Button::new("refresh-library")
                        .label("Refresh")
                        .on_click(refresh),
                )
                .child(
                    Button::new("open-recordings-folder-tab")
                        .label("Open folder")
                        .on_click(open_folder),
                ),
        )
        .child(
            // h_flex centres on the cross axis by default; stretch so the table
            // column gets a real height for uniform_list virtualization.
            h_flex()
                .flex_1()
                .min_h_0()
                .items_stretch()
                .gap_3()
                .overflow_hidden()
                .child(
                    v_flex()
                        .flex_1()
                        .min_w(px(280.))
                        .min_h_0()
                        .h_full()
                        .overflow_hidden()
                        .child(div().size_full().min_h_0().child(list)),
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

pub(crate) fn rows_from_catalog(catalog: &LibraryCatalog) -> Vec<RecordingRow> {
    let (zone, today) = local_clock();
    catalog
        .entries
        .iter()
        .map(|entry| RecordingRow::from_info_in(&entry.info, &zone, today))
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TranscriptControls {
    pub(crate) transcribe_enabled: bool,
    pub(crate) cancel_enabled: bool,
    pub(crate) retry_enabled: bool,
}

/// Transcribe is off while that row is queued or running. Cancel is on only then.
/// Retry is the Transcribe action for a Failed row.
pub(crate) fn transcript_controls(
    status: crate::domain::recording::TranscriptStatus,
    job_active: bool,
) -> TranscriptControls {
    use crate::domain::recording::TranscriptStatus;
    let active = job_active
        || matches!(
            status,
            TranscriptStatus::Queued | TranscriptStatus::Progress(_)
        );
    TranscriptControls {
        transcribe_enabled: !active && status != TranscriptStatus::Failed,
        cancel_enabled: active,
        retry_enabled: status == TranscriptStatus::Failed && !active,
    }
}

pub(crate) fn play_button_label(phase: PlayerPhase) -> SharedString {
    match phase {
        PlayerPhase::Playing => SharedString::from("Playing…"),
        _ => SharedString::from("Play"),
    }
}

#[cfg(test)]
mod tests {
    use super::{RecordingRow, RowBadge};
    use crate::domain::ids::RecordingId;
    use crate::domain::recording::{
        RecordingContainer, RecordingDay, RecordingInfo, SidecarState, TranscriptStatus,
    };

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
        assert_eq!(row.duration.as_ref(), "01:05");
        assert_eq!(row.transcript.as_ref(), "Ready");
        assert!(row.detail.as_ref().contains("Recovered"));
        assert_eq!(row.date.as_ref(), "1970-01-01 00:00");
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
        assert_eq!(row.date.as_ref(), "—");
        assert_eq!(row.meta.as_ref(), "— · 12:41 · 38.0 MB");
        assert_eq!(row.detail_line.as_ref(), "— · 12:41 · 38.0 MB · WAV");
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
    fn transcript_controls_follow_row_status() {
        use super::{transcript_controls, TranscriptControls};
        use crate::domain::recording::TranscriptStatus;

        // Arrange
        let idle = TranscriptStatus::None;
        let queued = TranscriptStatus::Queued;
        let running = TranscriptStatus::Progress(42);
        let failed = TranscriptStatus::Failed;

        // Act
        let idle_controls = transcript_controls(idle, false);
        let queued_controls = transcript_controls(queued, true);
        let running_controls = transcript_controls(running, true);
        let failed_controls = transcript_controls(failed, false);

        // Assert
        assert_eq!(
            idle_controls,
            TranscriptControls {
                transcribe_enabled: true,
                cancel_enabled: false,
                retry_enabled: false,
            }
        );
        assert_eq!(
            queued_controls,
            TranscriptControls {
                transcribe_enabled: false,
                cancel_enabled: true,
                retry_enabled: false,
            }
        );
        assert_eq!(
            running_controls,
            TranscriptControls {
                transcribe_enabled: false,
                cancel_enabled: true,
                retry_enabled: false,
            }
        );
        assert_eq!(
            failed_controls,
            TranscriptControls {
                transcribe_enabled: false,
                cancel_enabled: false,
                retry_enabled: true,
            }
        );
    }

    #[test]
    fn progress_tick_updates_transcript_cell_without_reordering_rows() {
        use super::RecordingsDelegate;
        use crate::domain::recording::{
            RecordingContainer, RecordingInfo, SidecarState, TranscriptStatus,
        };

        // Arrange
        let mut delegate = RecordingsDelegate::new();
        let first = RecordingRow::from_info(&RecordingInfo::new(
            RecordingId::from_raw(1),
            "alpha",
            RecordingContainer::Wav,
            Some(2),
            Some(1),
            1,
            None,
            TranscriptStatus::None,
            SidecarState::Present,
            false,
            Vec::new(),
        ));
        let second = RecordingRow::from_info(&RecordingInfo::new(
            RecordingId::from_raw(2),
            "beta",
            RecordingContainer::Wav,
            Some(1),
            Some(1),
            1,
            None,
            TranscriptStatus::None,
            SidecarState::Present,
            false,
            Vec::new(),
        ));
        delegate.set_rows(vec![first, second]);
        let names_before: Vec<_> = delegate
            .rows()
            .iter()
            .map(|row| row.name.to_string())
            .collect();

        // Act
        let updated = delegate.set_transcript_display(RecordingId::from_raw(2), "42%");

        // Assert
        assert!(updated);
        let names_after: Vec<_> = delegate
            .rows()
            .iter()
            .map(|row| row.name.to_string())
            .collect();
        assert_eq!(names_after, names_before);
        assert_eq!(delegate.rows()[0].transcript.as_ref(), "—");
        assert_eq!(delegate.rows()[1].transcript.as_ref(), "42%");
    }
}
