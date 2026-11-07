//! Transcription-model dialog. Selection, download, cancel, and retry stay off `render`.

use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::{DialogClose, DialogFooter};
use gpui_kit::component::list::ListItem;
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, WindowExt as _};
use gpui_kit::{
    div, px, AnyElement, App, ClickEvent, IntoElement, ParentElement, SharedString, Styled, Window,
};

use crate::domain::ids::RecordingId;
use crate::domain::recording::format_size;
use crate::services::models::ModelPhase;
use crate::ui::components::model_card::{model_tag, ModelCardTag};

use super::state::ModelInstallState;

/// Friendly text for one manifest entry. Kept here because the manifest is an integrity file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ModelCopy<'a> {
    pub(crate) name: &'a str,
    /// (speed, accuracy), each out of 3. General comparisons, not measured on this PC.
    pub(crate) ratings: Option<(u8, u8)>,
}

pub(crate) fn model_copy(id: &str) -> ModelCopy<'_> {
    match id {
        "tiny-q5_1" => ModelCopy {
            name: "Tiny",
            ratings: Some((3, 1)),
        },
        "base-q5_1" => ModelCopy {
            name: "Base",
            ratings: Some((2, 2)),
        },
        "small-q5_1" => ModelCopy {
            name: "Small",
            ratings: Some((1, 3)),
        },
        unknown => ModelCopy {
            name: unknown,
            ratings: None,
        },
    }
}

/// What the footer offers. `Retry` only appears for the model that failed, because
/// `ModelHost::retry` repeats the last requested model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModelFooter {
    CancelDownload,
    Retry,
    Download,
    Use,
    Done,
    CloseOnly,
}

pub(crate) fn model_footer(
    selected: Option<(&str, ModelInstallState)>,
    phase: ModelPhase,
    download_target: Option<&str>,
) -> ModelFooter {
    if phase == ModelPhase::Downloading {
        return ModelFooter::CancelDownload;
    }
    let Some((id, state)) = selected else {
        return ModelFooter::CloseOnly;
    };
    let failed = matches!(phase, ModelPhase::Failed | ModelPhase::Cancelled);
    if failed && download_target == Some(id) {
        return ModelFooter::Retry;
    }
    match state {
        ModelInstallState::NotInstalled => ModelFooter::Download,
        ModelInstallState::Installed => ModelFooter::Use,
        ModelInstallState::InUse => ModelFooter::Done,
    }
}

pub(crate) fn download_label(bytes: u64) -> String {
    format!("Download {}", format_size(bytes))
}

pub(crate) fn use_label(name: &str) -> String {
    format!("Use {name}")
}

pub(crate) fn row_meta(state: ModelInstallState, bytes: u64) -> String {
    match state {
        ModelInstallState::NotInstalled => format_size(bytes),
        ModelInstallState::Installed | ModelInstallState::InUse => "On this PC".to_string(),
    }
}

pub(crate) fn row_tag(state: ModelInstallState) -> Option<ModelCardTag> {
    match state {
        ModelInstallState::InUse => Some(ModelCardTag::InUse),
        ModelInstallState::Installed => Some(ModelCardTag::Installed),
        ModelInstallState::NotInstalled => None,
    }
}

pub(crate) fn row_accessibility_label(copy: &ModelCopy<'_>, state: ModelInstallState) -> String {
    let status = match state {
        ModelInstallState::InUse => "in use",
        ModelInstallState::Installed => "installed",
        ModelInstallState::NotInstalled => "not installed",
    };
    match copy.ratings {
        Some((speed, accuracy)) => format!(
            "{}, {status}, speed {speed} of 3, accuracy {accuracy} of 3",
            copy.name
        ),
        None => format!("{}, {status}", copy.name),
    }
}

/// Where a model failure appears: inside the open dialog, or in its own alert.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ErrorDisplay {
    Inline,
    Alert,
}

pub(crate) fn error_display(dialog_open: bool) -> ErrorDisplay {
    if dialog_open {
        ErrorDisplay::Inline
    } else {
        ErrorDisplay::Alert
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CatalogRow {
    pub(crate) id: SharedString,
    pub(crate) label: SharedString,
    pub(crate) accessibility_label: SharedString,
    pub(crate) install_state: ModelInstallState,
}

#[derive(Clone, Debug)]
pub(crate) struct ModelPromptView {
    pub(crate) catalog: Vec<CatalogRow>,
    pub(crate) selected_id: Option<SharedString>,
    pub(crate) progress: Option<u8>,
    pub(crate) failure: Option<&'static str>,
    pub(crate) footer: ModelFooter,
    /// Size of the selected model, for the Download label.
    pub(crate) selected_bytes: Option<u64>,
    /// Display name of the selected model, for the Use label.
    pub(crate) selected_name: SharedString,
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
        let busy = view.footer == ModelFooter::CancelDownload;
        // The user can always leave, except while a download is running.
        let can_close = !busy;
        let footer = footer_buttons(&view, &handlers);
        let closed = handlers.on_closed.clone();
        let mut body = v_flex()
            .w_full()
            .gap_2()
            .child("The download needs a network.");
        for row in view.catalog.clone() {
            let on_select = handlers.on_select.clone();
            let id = row.id.clone();
            let is_selected = view.selected_id.as_ref() == Some(&row.id);
            let mut line = h_flex()
                .w_full()
                .gap_2()
                .items_center()
                .child(row.label.clone());
            if let Some(tag) = row_tag(row.install_state) {
                line = line.child(model_tag(tag));
            }
            body = body.child(
                ListItem::new(SharedString::from(format!("model-row-{id}")))
                    .w_full()
                    .selected(is_selected)
                    .disabled(busy)
                    .accessibility_label(row.accessibility_label.clone())
                    .on_click(move |_event, window, cx| on_select(id.as_ref(), window, cx))
                    .child(line),
            );
        }
        if busy {
            if let Some(percent) = view.progress {
                body = body.child(format!("Downloading {percent}%"));
            }
        }
        if let Some(failure) = view.failure {
            body = body.child(div().text_color(cx.theme().danger).child(failure));
        }
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
            .footer(DialogFooter::new().gap_2().children(footer))
    });
}

fn footer_buttons(view: &ModelPromptView, handlers: &ModelPromptHandlers) -> Vec<AnyElement> {
    let close = |label: &'static str, primary: bool| {
        let button = Button::new("model-close").label(label);
        let button = if primary {
            button.primary()
        } else {
            button.outline()
        };
        DialogClose::new().child(button).into_any_element()
    };
    match view.footer {
        ModelFooter::CancelDownload => {
            let on_cancel = handlers.on_cancel.clone();
            vec![Button::new("model-cancel")
                .outline()
                .label("Cancel download")
                .on_click(move |event, window, cx| on_cancel(event, window, cx))
                .into_any_element()]
        }
        ModelFooter::Retry => {
            let on_retry = handlers.on_retry.clone();
            vec![
                close("Close", false),
                Button::new("model-retry")
                    .primary()
                    .label("Retry download")
                    .on_click(move |event, window, cx| on_retry(event, window, cx))
                    .into_any_element(),
            ]
        }
        ModelFooter::Download => {
            let on_download = handlers.on_download.clone();
            let label = download_label(view.selected_bytes.unwrap_or(0));
            vec![
                close("Close", false),
                Button::new("model-download")
                    .primary()
                    .label(label)
                    .on_click(move |event, window, cx| on_download(event, window, cx))
                    .into_any_element(),
            ]
        }
        ModelFooter::Use => {
            let on_use = handlers.on_use.clone();
            vec![
                close("Close", false),
                Button::new("model-use")
                    .primary()
                    .label(use_label(&view.selected_name))
                    .on_click(move |event, window, cx| on_use(event, window, cx))
                    .into_any_element(),
            ]
        }
        ModelFooter::Done => vec![close("Done", true)],
        ModelFooter::CloseOnly => vec![close("Close", false)],
    }
}

pub(crate) fn open_model_alert(window: &mut Window, cx: &mut App, message: &'static str) {
    window.open_alert_dialog(cx, move |alert, _, _| {
        alert.title("Transcription model").description(message)
    });
}

#[cfg(test)]
mod tests {
    use super::{
        after_model_ready, download_label, error_display, model_copy, model_footer,
        row_accessibility_label, row_meta, row_tag, use_label, AfterModelReady, ErrorDisplay,
        ModelFooter,
    };
    use crate::domain::ids::RecordingId;
    use crate::infrastructure::ai::models::ModelManifest;
    use crate::services::models::ModelPhase;
    use crate::ui::components::model_card::ModelCardTag;
    use crate::views::state::ModelInstallState;

    #[test]
    fn every_manifest_entry_has_a_friendly_name() {
        // Arrange
        let manifest = ModelManifest::load_bundled().expect("manifest");

        // Act
        let missing: Vec<&str> = manifest
            .entries()
            .iter()
            .map(|entry| entry.id())
            .filter(|id| model_copy(id).ratings.is_none())
            .collect();

        // Assert
        assert!(missing.is_empty(), "no copy for {missing:?}");
    }

    #[test]
    fn unknown_model_falls_back_to_its_id() {
        // Arrange
        let id = "large-v9";

        // Act
        let copy = model_copy(id);

        // Assert
        assert_eq!(copy.name, "large-v9");
        assert_eq!(copy.ratings, None);
    }

    #[test]
    fn footer_follows_the_selected_model_state() {
        // Arrange
        let idle = ModelPhase::Idle;

        // Act
        let missing = model_footer(Some(("a", ModelInstallState::NotInstalled)), idle, None);
        let installed = model_footer(Some(("a", ModelInstallState::Installed)), idle, None);
        let in_use = model_footer(Some(("a", ModelInstallState::InUse)), idle, None);
        let none = model_footer(None, idle, None);

        // Assert
        assert_eq!(missing, ModelFooter::Download);
        assert_eq!(installed, ModelFooter::Use);
        assert_eq!(in_use, ModelFooter::Done);
        assert_eq!(none, ModelFooter::CloseOnly);
    }

    #[test]
    fn downloading_offers_only_cancel() {
        // Arrange
        let selected = Some(("a", ModelInstallState::NotInstalled));

        // Act
        let footer = model_footer(selected, ModelPhase::Downloading, Some("a"));

        // Assert
        assert_eq!(footer, ModelFooter::CancelDownload);
    }

    #[test]
    fn retry_is_only_offered_for_the_model_that_failed() {
        // Arrange
        let same = Some(("a", ModelInstallState::NotInstalled));
        let other = Some(("b", ModelInstallState::NotInstalled));

        // Act
        let failed_same = model_footer(same, ModelPhase::Failed, Some("a"));
        let cancelled_same = model_footer(same, ModelPhase::Cancelled, Some("a"));
        let failed_other = model_footer(other, ModelPhase::Failed, Some("a"));

        // Assert
        assert_eq!(failed_same, ModelFooter::Retry);
        assert_eq!(cancelled_same, ModelFooter::Retry);
        assert_eq!(failed_other, ModelFooter::Download);
    }

    #[test]
    fn labels_name_the_action() {
        // Arrange
        let bytes = 190_085_487;

        // Act
        let download = download_label(bytes);
        let use_model = use_label("Small");

        // Assert
        assert_eq!(download, "Download 181.3 MB");
        assert_eq!(use_model, "Use Small");
    }

    #[test]
    fn installed_rows_replace_the_size() {
        // Arrange
        let bytes = 32_152_673;

        // Act
        let missing = row_meta(ModelInstallState::NotInstalled, bytes);
        let installed = row_meta(ModelInstallState::Installed, bytes);
        let in_use = row_meta(ModelInstallState::InUse, bytes);

        // Assert
        assert_eq!(missing, "30.7 MB");
        assert_eq!(installed, "On this PC");
        assert_eq!(in_use, "On this PC");
    }

    #[test]
    fn rows_are_tagged_by_state() {
        // Arrange
        let states = [
            ModelInstallState::InUse,
            ModelInstallState::Installed,
            ModelInstallState::NotInstalled,
        ];

        // Act
        let tags: Vec<Option<ModelCardTag>> = states.into_iter().map(row_tag).collect();

        // Assert
        assert_eq!(
            tags,
            vec![
                Some(ModelCardTag::InUse),
                Some(ModelCardTag::Installed),
                None
            ]
        );
    }

    #[test]
    fn accessibility_label_includes_the_state() {
        // Arrange
        let copy = model_copy("base-q5_1");

        // Act
        let in_use = row_accessibility_label(&copy, ModelInstallState::InUse);
        let missing = row_accessibility_label(&copy, ModelInstallState::NotInstalled);

        // Assert
        assert_eq!(in_use, "Base, in use, speed 2 of 3, accuracy 2 of 3");
        assert_eq!(
            missing,
            "Base, not installed, speed 2 of 3, accuracy 2 of 3"
        );
    }

    #[test]
    fn errors_show_inline_only_while_the_dialog_is_open() {
        // Arrange
        let open = true;

        // Act
        let while_open = error_display(open);
        let while_closed = error_display(!open);

        // Assert
        assert_eq!(while_open, ErrorDisplay::Inline);
        assert_eq!(while_closed, ErrorDisplay::Alert);
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
