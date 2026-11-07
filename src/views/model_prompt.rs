//! Speech-model dialog. Selection, download, cancel, and retry stay off `render`.

use gpui_kit::component::button::Button;
use std::rc::Rc;

use gpui_kit::component::dialog::{DialogClose, DialogFooter};
use gpui_kit::component::list::ListItem;
use gpui_kit::component::{v_flex, Disableable as _, WindowExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{px, App, ClickEvent, ParentElement, SharedString, Styled, Window};

use crate::domain::ids::RecordingId;
use crate::domain::recording::format_size;
use crate::services::models::ModelPhase;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CatalogRow {
    pub(crate) id: SharedString,
    pub(crate) label: SharedString,
}

#[derive(Clone, Debug)]
pub(crate) struct ModelPromptView {
    pub(crate) catalog: Vec<CatalogRow>,
    pub(crate) selected_id: Option<SharedString>,
    pub(crate) selected_line: SharedString,
    pub(crate) progress: Option<u8>,
    pub(crate) failure: Option<&'static str>,
    pub(crate) phase: ModelPhase,
    /// The highlighted model is installed but not yet the one in use.
    pub(crate) can_use: bool,
    /// The highlighted model is the one the last download attempt was for.
    pub(crate) selected_is_target: bool,
}

pub(crate) fn download_button_label() -> &'static str {
    "Download"
}

pub(crate) fn catalog_row_label(id: &str, bytes: u64) -> String {
    format!("{id} ({})", format_size(bytes))
}

pub(crate) fn selected_model_line(id: Option<&str>, bytes: Option<u64>) -> String {
    match (id.filter(|id| !id.is_empty()), bytes) {
        (Some(id), Some(bytes)) => format!("Selected: {id} ({})", format_size(bytes)),
        _ => "Choose a transcription model.".to_string(),
    }
}

/// What happens once a model is chosen and ready.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AfterModelReady {
    /// Transcribe opened the dialog: close it and start that job.
    CloseAndTranscribe(RecordingId),
    /// The user opened the dialog themselves: leave it open.
    StayOpen,
}

pub(crate) fn after_model_ready(pending: Option<RecordingId>) -> AfterModelReady {
    match pending {
        Some(id) => AfterModelReady::CloseAndTranscribe(id),
        None => AfterModelReady::StayOpen,
    }
}

type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;
type SelectHandler = Rc<dyn Fn(&str, &mut Window, &mut App)>;

/// Callbacks for the dialog. Shared because the builder runs again on every render.
#[derive(Clone)]
pub(crate) struct ModelPromptHandlers {
    pub(crate) on_select: SelectHandler,
    pub(crate) on_download: ClickHandler,
    pub(crate) on_use: ClickHandler,
    pub(crate) on_cancel: ClickHandler,
    pub(crate) on_retry: ClickHandler,
    /// Runs when the user dismisses the dialog by any route.
    pub(crate) on_closed: ClickHandler,
}

pub(crate) fn open_model_prompt(
    window: &mut Window,
    cx: &mut App,
    read: impl Fn(&App) -> ModelPromptView + 'static,
    handlers: ModelPromptHandlers,
) {
    window.open_dialog(cx, move |dialog, _, cx| {
        let view = read(cx);
        let busy = view.phase == ModelPhase::Downloading;
        // The user can always leave, except while a download is running.
        let can_close = !busy;
        // The worker retries its last request, so Retry only fits the model that failed.
        let can_retry = view.selected_is_target
            && matches!(view.phase, ModelPhase::Failed | ModelPhase::Cancelled);
        let can_download = !busy && view.selected_id.is_some();
        let can_use = !busy && view.can_use;
        let selected = view.selected_id.clone();
        let mut body = v_flex()
            .w_full()
            .gap_2()
            .child("The download needs a network.");
        for row in view.catalog.clone() {
            let on_select = handlers.on_select.clone();
            let id = row.id.clone();
            let is_selected = selected.as_ref() == Some(&row.id);
            body = body.child(
                ListItem::new(SharedString::from(format!("model-row-{id}")))
                    .w_full()
                    .selected(is_selected)
                    .disabled(busy)
                    .accessibility_label(row.label.clone())
                    .on_click(move |_event, window, cx| on_select(id.as_ref(), window, cx))
                    .child(row.label),
            );
        }
        body = body.child(view.selected_line.clone());
        if busy {
            if let Some(percent) = view.progress {
                body = body.child(format!("Downloading {percent}%"));
            }
        }
        if let Some(failure) = view.failure {
            body = body.child(failure);
        }
        let download = handlers.on_download.clone();
        let use_model = handlers.on_use.clone();
        let cancel = handlers.on_cancel.clone();
        let retry = handlers.on_retry.clone();
        let closed = handlers.on_closed.clone();
        dialog
            .title("Transcription model")
            .w(px(560.))
            .close_button(can_close)
            .overlay_closable(can_close)
            .keyboard(can_close)
            // Enabling the keyboard also enables Enter, which must not confirm and close.
            .on_ok(|_, _, _| false)
            .on_close(move |event, window, cx| closed(event, window, cx))
            .child(body)
            .footer(
                DialogFooter::new()
                    .gap_2()
                    .child(
                        Button::new("model-download")
                            .label(download_button_label())
                            .disabled(!can_download)
                            .on_click(move |event, window, cx| download(event, window, cx)),
                    )
                    .child(
                        Button::new("model-use")
                            .label("Use")
                            .disabled(!can_use)
                            .on_click(move |event, window, cx| use_model(event, window, cx)),
                    )
                    .child(
                        Button::new("model-cancel")
                            .label("Cancel")
                            .disabled(!busy)
                            .on_click(move |event, window, cx| cancel(event, window, cx)),
                    )
                    .child(
                        Button::new("model-retry")
                            .label("Retry")
                            .disabled(!can_retry)
                            .on_click(move |event, window, cx| retry(event, window, cx)),
                    )
                    .when(can_close, |footer| {
                        footer.child(
                            DialogClose::new()
                                .child(Button::new("model-close").label("Close").outline()),
                        )
                    }),
            )
    });
}

pub(crate) fn open_model_alert(window: &mut Window, cx: &mut App, message: &'static str) {
    window.open_alert_dialog(cx, move |alert, _, _| {
        alert.title("Transcription model").description(message)
    });
}

#[cfg(test)]
mod tests {
    use super::{
        after_model_ready, catalog_row_label, download_button_label, selected_model_line,
        AfterModelReady,
    };
    use crate::domain::ids::RecordingId;
    use crate::infrastructure::ai::models::ModelManifest;

    #[test]
    fn download_button_is_the_word_download() {
        // Arrange

        // Act
        let label = download_button_label();

        // Assert
        assert_eq!(label, "Download");
    }

    #[test]
    fn catalog_rows_list_every_manifest_variant_and_size() {
        // Arrange
        let manifest = ModelManifest::load_bundled().expect("manifest");

        // Act
        let rows: Vec<String> = manifest
            .entries()
            .iter()
            .map(|entry| catalog_row_label(entry.id(), entry.bytes()))
            .collect();

        // Assert
        assert_eq!(
            rows,
            vec![
                "tiny-q5_1 (30.7 MB)".to_string(),
                "base-q5_1 (56.9 MB)".to_string(),
                "small-q5_1 (181.3 MB)".to_string(),
            ]
        );
    }

    #[test]
    fn selected_line_sits_outside_the_catalog_rows() {
        // Arrange
        let manifest = ModelManifest::load_bundled().expect("manifest");
        let rows: Vec<String> = manifest
            .entries()
            .iter()
            .map(|entry| catalog_row_label(entry.id(), entry.bytes()))
            .collect();

        // Act
        let selected = selected_model_line(Some("base-q5_1"), Some(59_707_625));
        let empty = selected_model_line(None, None);

        // Assert
        assert_eq!(selected, "Selected: base-q5_1 (56.9 MB)");
        assert_eq!(empty, "Choose a transcription model.");
        assert!(rows.iter().all(|row| row != &selected));
    }

    #[test]
    fn a_pending_transcription_starts_once_a_model_is_ready() {
        // Arrange
        let pending = Some(RecordingId::from_raw(7));

        // Act
        let with_job = after_model_ready(pending);
        let without_job = after_model_ready(None);

        // Assert
        assert_eq!(
            with_job,
            AfterModelReady::CloseAndTranscribe(RecordingId::from_raw(7))
        );
        assert_eq!(without_job, AfterModelReady::StayOpen);
    }
}
