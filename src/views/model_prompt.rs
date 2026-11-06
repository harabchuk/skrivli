//! Speech-model dialog. Selection, download, cancel, and retry stay off `render`.

use gpui_kit::component::button::Button;
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::list::ListItem;
use gpui_kit::component::{v_flex, Disableable as _, WindowExt as _};
use gpui_kit::{px, App, ClickEvent, ParentElement, SharedString, Styled, Window};

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
        _ => "Choose a speech model.".to_string(),
    }
}

pub(crate) fn prompt_can_dismiss(phase: ModelPhase) -> bool {
    phase == ModelPhase::Ready
}

pub(crate) fn open_model_prompt(
    window: &mut Window,
    cx: &mut App,
    read: impl Fn(&App) -> ModelPromptView + 'static,
    on_select: impl Fn(&str, &mut Window, &mut App) + Clone + 'static,
    on_download: impl Fn(&ClickEvent, &mut Window, &mut App) + Clone + 'static,
    on_cancel: impl Fn(&ClickEvent, &mut Window, &mut App) + Clone + 'static,
    on_retry: impl Fn(&ClickEvent, &mut Window, &mut App) + Clone + 'static,
) {
    window.open_dialog(cx, move |dialog, _, cx| {
        let view = read(cx);
        let busy = view.phase == ModelPhase::Downloading;
        let can_retry = matches!(view.phase, ModelPhase::Failed | ModelPhase::Cancelled);
        let can_download = !busy && view.selected_id.is_some();
        let selected = view.selected_id.clone();
        let mut body = v_flex()
            .w_full()
            .gap_2()
            .child("The download needs a network.");
        for row in view.catalog.clone() {
            let on_select = on_select.clone();
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
        let download = on_download.clone();
        let cancel = on_cancel.clone();
        let retry = on_retry.clone();
        dialog
            .title("Speech model")
            .w(px(560.))
            .close_button(false)
            .overlay_closable(false)
            .keyboard(false)
            .child(body)
            .footer(
                DialogFooter::new()
                    .gap_2()
                    .child(
                        Button::new("model-download")
                            .label(download_button_label())
                            .disabled(!can_download)
                            .on_click(download),
                    )
                    .child(
                        Button::new("model-cancel")
                            .label("Cancel")
                            .disabled(!busy)
                            .on_click(cancel),
                    )
                    .child(
                        Button::new("model-retry")
                            .label("Retry")
                            .disabled(!can_retry)
                            .on_click(retry),
                    ),
            )
    });
}

pub(crate) fn open_model_alert(window: &mut Window, cx: &mut App, message: &'static str) {
    window.open_alert_dialog(cx, move |alert, _, _| {
        alert.title("Speech model").description(message)
    });
}

#[cfg(test)]
mod tests {
    use super::{
        catalog_row_label, download_button_label, prompt_can_dismiss, selected_model_line,
    };
    use crate::infrastructure::ai::models::ModelManifest;
    use crate::services::models::ModelPhase;

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
        assert_eq!(empty, "Choose a speech model.");
        assert!(rows.iter().all(|row| row != &selected));
    }

    #[test]
    fn prompt_stays_open_until_the_model_is_ready() {
        // Arrange
        let closed = [
            ModelPhase::Idle,
            ModelPhase::Downloading,
            ModelPhase::Failed,
            ModelPhase::Cancelled,
        ];

        // Act
        let ready = prompt_can_dismiss(ModelPhase::Ready);
        let blocked = closed.map(prompt_can_dismiss);

        // Assert
        assert!(ready);
        assert_eq!(blocked, [false, false, false, false]);
    }
}
