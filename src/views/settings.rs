//! Settings: the transcription model and app behaviour. Rendering never touches a worker.

use gpui_kit::component::button::Button;
use gpui_kit::component::link::Link;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Sizable as _, WindowExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{Anchor, App, ClickEvent, IntoElement, ParentElement, SharedString, Styled, Window};

use crate::domain::recording::{format_size, MaxMeetingDuration};
use crate::ui::components::dialog_style::DIALOG_WIDTH;
use crate::ui::components::model_card::{model_tag, ModelCardTag};
use crate::ui::components::settings_rows::{settings_row, settings_section};

use super::model_prompt::model_copy;
use super::state::ModelStatus;

/// Text for the Transcription row. Pure, so each model state can be tested without GPUI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SettingsModelRow {
    pub(crate) title: String,
    pub(crate) description: String,
    pub(crate) button: &'static str,
    pub(crate) in_use: bool,
}

pub(crate) fn settings_model_row(
    status: &ModelStatus,
    model_bytes: Option<u64>,
) -> SettingsModelRow {
    match status {
        ModelStatus::InUse { id } => {
            let copy = model_copy(id);
            let description = match model_bytes {
                Some(bytes) => format!("{} · {}", copy.description, format_size(bytes)),
                None => copy.description.to_string(),
            };
            SettingsModelRow {
                title: copy.name.to_string(),
                description,
                button: "Change model…",
                in_use: true,
            }
        }
        ModelStatus::Downloading { id, percent } => SettingsModelRow {
            title: model_copy(id).name.to_string(),
            description: format!("Downloading {percent}%"),
            button: "Change model…",
            in_use: false,
        },
        ModelStatus::Missing | ModelStatus::Checking => SettingsModelRow {
            title: "No model installed".to_string(),
            description: "Choose a model to create transcripts.".to_string(),
            button: "Choose model…",
            in_use: false,
        },
    }
}

pub(crate) const MAX_DURATION_TITLE: &str = "Maximum meeting duration";
pub(crate) const MAX_DURATION_DESCRIPTION: &str =
    "Stops a forgotten recording so it does not write a very long file.";

/// One entry of the Maximum meeting duration menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DurationChoice {
    pub(crate) label: String,
    pub(crate) duration: MaxMeetingDuration,
    pub(crate) checked: bool,
}

pub(crate) fn duration_choices(selected: MaxMeetingDuration) -> Vec<DurationChoice> {
    MaxMeetingDuration::CHOICES_HOURS
        .iter()
        .filter_map(|hours| MaxMeetingDuration::from_hours(*hours))
        .map(|duration| DurationChoice {
            label: duration.label(),
            duration,
            checked: duration == selected,
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SettingsView {
    pub(crate) close_to_tray: bool,
    pub(crate) max_duration: MaxMeetingDuration,
    pub(crate) model: SettingsModelRow,
}

/// Settings dialog callbacks. Each runs on the GPUI thread and only updates the session.
pub(crate) struct SettingsHandlers<T, D, M> {
    pub(crate) on_close_to_tray: T,
    pub(crate) on_max_duration: D,
    pub(crate) on_change_model: M,
}

/// Where the copyright line in Settings links to.
pub(crate) const REPO_URL: &str = "https://github.com/harabchuk/skrivli";

pub(crate) fn copyright_text() -> &'static str {
    "© 2026 Alexis Harabchuk"
}

pub(crate) fn version_text() -> String {
    format!("Skrivli {}", env!("CARGO_PKG_VERSION"))
}

pub(crate) fn open_settings(
    window: &mut Window,
    cx: &mut App,
    read: impl Fn(&App) -> SettingsView + 'static,
    handlers: SettingsHandlers<
        impl Fn(&bool, &mut Window, &mut App) + Clone + 'static,
        impl Fn(MaxMeetingDuration, &mut Window, &mut App) + Clone + 'static,
        impl Fn(&ClickEvent, &mut Window, &mut App) + Clone + 'static,
    >,
) {
    let SettingsHandlers {
        on_close_to_tray,
        on_max_duration,
        on_change_model,
    } = handlers;
    window.open_dialog(cx, move |dialog, _, cx| {
        let view = read(cx);
        let model = view.model;
        let model_title = h_flex()
            .gap_2()
            .items_center()
            .child(SharedString::from(model.title))
            .when(model.in_use, |this| {
                this.child(model_tag(ModelCardTag::InUse))
            });
        dialog.title("Settings").w(DIALOG_WIDTH).child(
            v_flex()
                .w_full()
                .gap_4()
                .child(settings_section(
                    "Transcription",
                    [settings_row(
                        model_title,
                        model.description,
                        Button::new("settings-change-model")
                            .outline()
                            .small()
                            .label(model.button)
                            .on_click(on_change_model.clone()),
                        cx,
                    )],
                    cx,
                ))
                .child(settings_section(
                    "Recording",
                    [settings_row(
                        MAX_DURATION_TITLE,
                        MAX_DURATION_DESCRIPTION,
                        max_duration_menu(view.max_duration, on_max_duration.clone()),
                        cx,
                    )],
                    cx,
                ))
                .child(settings_section(
                    "General",
                    [settings_row(
                        "Close to the notification area",
                        "Skrivli keeps running when you close the window.",
                        close_to_tray_switch(view.close_to_tray, on_close_to_tray.clone()),
                        cx,
                    )],
                    cx,
                ))
                .child(
                    h_flex()
                        .gap_2()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(version_text())
                        .child(
                            Link::new("settings-copyright")
                                .href(REPO_URL)
                                .child(copyright_text()),
                        ),
                ),
        )
    });
}

fn max_duration_menu(
    selected: MaxMeetingDuration,
    on_pick: impl Fn(MaxMeetingDuration, &mut Window, &mut App) + Clone + 'static,
) -> impl IntoElement {
    let choices = duration_choices(selected);
    Button::new("settings-max-duration")
        .outline()
        .small()
        .label(SharedString::from(selected.label()))
        .dropdown_caret(true)
        .accessibility_label(MAX_DURATION_TITLE)
        .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _window, _cx| {
            choices.iter().fold(menu, |menu, choice| {
                let pick = on_pick.clone();
                let duration = choice.duration;
                menu.item(
                    PopupMenuItem::new(SharedString::from(choice.label.clone()))
                        .checked(choice.checked)
                        .on_click(move |_event, window, cx| pick(duration, window, cx)),
                )
            })
        })
}

fn close_to_tray_switch(
    checked: bool,
    on_change: impl Fn(&bool, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    Switch::new("close-to-tray")
        .checked(checked)
        .accessibility_label("Close to the notification area")
        .on_change(on_change)
}

/// Reached only through the reserved `ShowAbout` action. Settings shows the version itself.
pub(crate) fn open_about(window: &mut Window, cx: &mut App) {
    let version = SharedString::from(format!("Version {}", env!("CARGO_PKG_VERSION")));
    window.open_alert_dialog(cx, move |alert, _, _| {
        alert
            .title("Skrivli")
            .w(DIALOG_WIDTH)
            .description(version.clone())
    });
}

pub(crate) fn open_tray_explanation(window: &mut Window, cx: &mut App) {
    window.open_alert_dialog(cx, |alert, _, _| {
        alert
            .title("Skrivli is still running")
            .w(DIALOG_WIDTH)
            .description("Skrivli stays in the notification area. Use Exit to quit.")
    });
}

pub(crate) fn open_exit_while_recording(
    window: &mut Window,
    cx: &mut App,
    on_confirm: impl Fn(&mut Window, &mut App) + Clone + 'static,
) {
    window.open_alert_dialog(cx, move |alert, _, _| {
        let on_confirm = on_confirm.clone();
        alert
            .title("Stop and save?")
            .w(DIALOG_WIDTH)
            .description("A recording is in progress. Exit will stop and save it first.")
            .confirm()
            .ok_text("Stop and exit")
            .on_ok(move |_, window, cx| {
                on_confirm(window, cx);
                true
            })
    });
}

#[cfg(test)]
mod tests {
    use super::{copyright_text, settings_model_row, version_text, SettingsModelRow, REPO_URL};
    use crate::views::state::ModelStatus;

    #[test]
    fn row_shows_the_model_in_use_with_its_size() {
        // Arrange
        let status = ModelStatus::InUse {
            id: "nemotron-3.5-streaming-0.6b-q8_0".to_string(),
        };

        // Act
        let row = settings_model_row(&status, Some(983_696_512));

        // Assert
        assert_eq!(
            row,
            SettingsModelRow {
                title: "Nemotron 3.5 (Multilingual)".to_string(),
                description: "Accurate in 40 languages, including English and Swedish · 938.1 MB"
                    .to_string(),
                button: "Change model…",
                in_use: true,
            }
        );
    }

    #[test]
    fn row_offers_a_choice_when_nothing_is_installed() {
        // Arrange
        let status = ModelStatus::Missing;

        // Act
        let row = settings_model_row(&status, None);

        // Assert
        assert_eq!(row.title, "No model installed");
        assert_eq!(row.button, "Choose model…");
        assert!(!row.in_use);
    }

    #[test]
    fn duration_choices_list_every_option_and_check_the_selected_one() {
        // Arrange
        let selected = crate::domain::recording::MaxMeetingDuration::default();

        // Act
        let choices = super::duration_choices(selected);

        // Assert
        let labels: Vec<_> = choices.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(
            labels,
            ["1 hour", "2 hours", "3 hours", "4 hours", "6 hours", "8 hours", "12 hours"]
        );
        let checked: Vec<_> = choices.iter().filter(|c| c.checked).collect();
        assert_eq!(checked.len(), 1);
        assert_eq!(checked[0].duration, selected);
        assert_eq!(
            super::MAX_DURATION_DESCRIPTION,
            "Stops a forgotten recording so it does not write a very long file."
        );
    }

    #[test]
    fn version_text_names_the_app() {
        // Arrange

        // Act
        let text = version_text();

        // Assert
        assert!(text.starts_with("Skrivli "));
    }

    #[test]
    fn copyright_links_to_the_repository() {
        // Arrange

        // Act
        let text = copyright_text();

        // Assert
        assert_eq!(text, "© 2026 Alexis Harabchuk");
        assert_eq!(REPO_URL, "https://github.com/harabchuk/skrivli");
    }
}
