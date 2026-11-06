//! Speech-model dialog. Browse, download, cancel, and retry stay off `render`.

use gpui_kit::component::button::Button;
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::{v_flex, Disableable as _, WindowExt as _};
use gpui_kit::{App, ClickEvent, ParentElement, SharedString, Styled, Window};

use crate::domain::recording::format_size;
use crate::services::models::ModelPhase;

#[derive(Clone, Debug)]
pub(crate) struct ModelPromptView {
    pub(crate) files: Vec<SharedString>,
    pub(crate) download_label: SharedString,
    pub(crate) progress: Option<u8>,
    pub(crate) failure: Option<&'static str>,
    pub(crate) phase: ModelPhase,
}

pub(crate) fn download_button_label(variant: &str, bytes: u64) -> String {
    format!(
        "Download {variant} ({}) — needs a network",
        format_size(bytes)
    )
}

pub(crate) fn open_model_prompt(
    window: &mut Window,
    cx: &mut App,
    read: impl Fn(&App) -> ModelPromptView + 'static,
    on_browse: impl Fn(&ClickEvent, &mut Window, &mut App) + Clone + 'static,
    on_download: impl Fn(&ClickEvent, &mut Window, &mut App) + Clone + 'static,
    on_cancel: impl Fn(&ClickEvent, &mut Window, &mut App) + Clone + 'static,
    on_retry: impl Fn(&ClickEvent, &mut Window, &mut App) + Clone + 'static,
) {
    window.open_dialog(cx, move |dialog, _, cx| {
        let view = read(cx);
        let busy = view.phase == ModelPhase::Downloading;
        let can_retry = matches!(view.phase, ModelPhase::Failed | ModelPhase::Cancelled);
        let files: Vec<SharedString> = if view.files.is_empty() {
            vec![SharedString::from("No speech models installed yet.")]
        } else {
            view.files.clone()
        };
        let mut body = v_flex().gap_2().child("Installed models").children(files);
        if busy {
            if let Some(percent) = view.progress {
                body = body.child(format!("Downloading {percent}%"));
            }
        }
        if let Some(failure) = view.failure {
            body = body.child(failure);
        }
        let browse = on_browse.clone();
        let download = on_download.clone();
        let cancel = on_cancel.clone();
        let retry = on_retry.clone();
        dialog.title("Speech model").child(body).footer(
            DialogFooter::new()
                .gap_2()
                .child(Button::new("model-browse").label("Browse").on_click(browse))
                .child(
                    Button::new("model-download")
                        .label(view.download_label.clone())
                        .disabled(busy)
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
    use super::download_button_label;

    #[test]
    fn download_label_names_size_and_network() {
        // Arrange
        let bytes = 2048_u64;

        // Act
        let label = download_button_label("base-q5_1", bytes);

        // Assert
        assert_eq!(label, "Download base-q5_1 (2.0 KB) — needs a network");
    }
}
