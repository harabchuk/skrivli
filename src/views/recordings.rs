//! Recordings tab: library table, detail pane, playback and file actions.

use std::ops::Range;

use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::dialog::{DialogAction, DialogClose, DialogFooter};
use gpui_kit::component::empty::{Empty, EmptyDescription, EmptyHeader, EmptyTitle};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::table::{Column, ColumnSort, DataTable, TableDelegate, TableState};
use gpui_kit::component::{h_flex, v_flex, Disableable as _, WindowExt as _};
use gpui_kit::{
    div, px, App, AppContext as _, ClickEvent, Context, Entity, IntoElement, ParentElement,
    SharedString, Styled, Window,
};

use crate::domain::ids::RecordingId;
use crate::domain::recording::{format_duration_ms, format_size, RecordingInfo, SidecarState};
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
}

impl RecordingRow {
    pub(crate) fn from_info(info: &RecordingInfo) -> Self {
        let date = info
            .started_at_unix_ms()
            .map(format_unix_ms)
            .unwrap_or_else(|| "—".to_string());
        let duration = info
            .duration_ms()
            .map(format_duration_ms)
            .unwrap_or_else(|| "—".to_string());
        let mut detail = format!("Size {}", format_size(info.size_bytes()));
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
        Self {
            id: info.id(),
            name: SharedString::from(info.name().to_string()),
            date: SharedString::from(date),
            duration: SharedString::from(duration),
            size: SharedString::from(format_size(info.size_bytes())),
            transcript: SharedString::from(info.transcript().display()),
            detail: SharedString::from(detail),
        }
    }
}

pub(crate) struct RecordingsDelegate {
    rows: Vec<RecordingRow>,
    columns: Vec<Column>,
}

impl RecordingsDelegate {
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
    window.open_dialog(cx, move |dialog, _, _| {
        let input = input.clone();
        let on_confirm = on_confirm.clone();
        dialog
            .title("Rename recording")
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
                DialogFooter::new()
                    .gap_2()
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
    catalog
        .entries
        .iter()
        .map(|entry| RecordingRow::from_info(&entry.info))
        .collect()
}

pub(crate) fn play_button_label(phase: PlayerPhase) -> SharedString {
    match phase {
        PlayerPhase::Playing => SharedString::from("Playing…"),
        _ => SharedString::from("Play"),
    }
}

fn format_unix_ms(ms: u64) -> String {
    let secs = (ms / 1_000) as i64;
    let days = secs.div_euclid(86_400);
    let time = secs.rem_euclid(86_400) as u64;
    let hour = time / 3600;
    let minute = (time % 3600) / 60;
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}")
}

/// Howard Hinnant civil-from-days (UTC).
fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
}

#[cfg(test)]
mod tests {
    use super::{format_unix_ms, RecordingRow};
    use crate::domain::ids::RecordingId;
    use crate::domain::recording::{
        RecordingContainer, RecordingInfo, SidecarState, TranscriptStatus,
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
        let row = RecordingRow::from_info(&info);

        // Assert
        assert_eq!(row.name.as_ref(), "clip");
        assert_eq!(row.duration.as_ref(), "01:05");
        assert_eq!(row.transcript.as_ref(), "Ready");
        assert!(row.detail.as_ref().contains("Recovered"));
        assert_eq!(format_unix_ms(0), "1970-01-01 00:00");
    }
}
