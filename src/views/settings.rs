//! Settings: the transcription model and app behaviour. Rendering never touches a worker.

use gpui_kit::component::button::Button;
use gpui_kit::component::link::Link;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Sizable as _, WindowExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{Anchor, App, ClickEvent, IntoElement, ParentElement, SharedString, Styled, Window};
use icu::plurals::PluralCategory;
use rust_i18n::t;

use crate::domain::language::Language;
use crate::domain::recording::MaxMeetingDuration;
use crate::ui::components::dialog_style::DIALOG_WIDTH;
use crate::ui::components::model_card::{model_tag, ModelCardTag};
use crate::ui::components::settings_rows::{settings_row, settings_section};

use super::format::{plural_category, size_text};
use super::model_prompt::model_copy;
use super::state::ModelStatus;

/// Text for the Transcription row. Pure, so each model state can be tested without GPUI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SettingsModelRow {
    pub(crate) title: String,
    pub(crate) description: String,
    pub(crate) button: String,
    pub(crate) in_use: bool,
}

pub(crate) fn settings_model_row(
    status: &ModelStatus,
    model_bytes: Option<u64>,
    language: Language,
) -> SettingsModelRow {
    let locale = language.code();
    match status {
        ModelStatus::InUse { id } => {
            let copy = model_copy(id, language);
            let description = match model_bytes {
                Some(bytes) => format!("{} · {}", copy.description, size_text(bytes, language)),
                None => copy.description.clone(),
            };
            SettingsModelRow {
                title: copy.name.to_string(),
                description,
                button: t!("app.settings.change_model", locale = locale).into_owned(),
                in_use: true,
            }
        }
        ModelStatus::Downloading { id, percent } => SettingsModelRow {
            title: model_copy(id, language).name.to_string(),
            description: t!(
                "app.settings.downloading",
                locale = locale,
                percent = percent
            )
            .into_owned(),
            button: t!("app.settings.change_model", locale = locale).into_owned(),
            in_use: false,
        },
        ModelStatus::Missing | ModelStatus::Checking => SettingsModelRow {
            title: t!("app.settings.no_model", locale = locale).into_owned(),
            description: t!("app.settings.choose_model_hint", locale = locale).into_owned(),
            button: t!("app.settings.choose_model", locale = locale).into_owned(),
            in_use: false,
        },
    }
}

/// "1 hour", "3 hours" and the like, with the plural the language needs.
pub(crate) fn duration_label(duration: MaxMeetingDuration, language: Language) -> String {
    let key = match plural_category(u64::from(duration.hours()), language) {
        PluralCategory::One => "app.settings.hours_one",
        PluralCategory::Few => "app.settings.hours_few",
        PluralCategory::Many => "app.settings.hours_many",
        PluralCategory::Zero | PluralCategory::Two | PluralCategory::Other => {
            "app.settings.hours_other"
        }
    };
    t!(key, locale = language.code(), count = duration.hours()).into_owned()
}

/// One entry of the Maximum meeting duration menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DurationChoice {
    pub(crate) label: String,
    pub(crate) duration: MaxMeetingDuration,
    pub(crate) checked: bool,
}

pub(crate) fn duration_choices(
    selected: MaxMeetingDuration,
    language: Language,
) -> Vec<DurationChoice> {
    MaxMeetingDuration::CHOICES_HOURS
        .iter()
        .filter_map(|hours| MaxMeetingDuration::from_hours(*hours))
        .map(|duration| DurationChoice {
            label: duration_label(duration, language),
            duration,
            checked: duration == selected,
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SettingsView {
    pub(crate) close_to_tray: bool,
    pub(crate) max_duration: MaxMeetingDuration,
    pub(crate) language: Language,
    pub(crate) model: SettingsModelRow,
}

/// Settings dialog callbacks. Each runs on the GPUI thread and only updates the session.
pub(crate) struct SettingsHandlers<T, D, L, M> {
    pub(crate) on_close_to_tray: T,
    pub(crate) on_max_duration: D,
    pub(crate) on_language: L,
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

/// Languages listed in the Language menu, each under its own name, with the current one checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LanguageChoice {
    pub(crate) label: &'static str,
    pub(crate) language: Language,
    pub(crate) checked: bool,
}

pub(crate) fn language_choices(selected: Language) -> Vec<LanguageChoice> {
    Language::ALL
        .iter()
        .map(|language| LanguageChoice {
            label: language.native_name(),
            language: *language,
            checked: *language == selected,
        })
        .collect()
}

pub(crate) fn open_settings(
    window: &mut Window,
    cx: &mut App,
    read: impl Fn(&App) -> SettingsView + 'static,
    handlers: SettingsHandlers<
        impl Fn(&bool, &mut Window, &mut App) + Clone + 'static,
        impl Fn(MaxMeetingDuration, &mut Window, &mut App) + Clone + 'static,
        impl Fn(Language, &mut Window, &mut App) + Clone + 'static,
        impl Fn(&ClickEvent, &mut Window, &mut App) + Clone + 'static,
    >,
) {
    let SettingsHandlers {
        on_close_to_tray,
        on_max_duration,
        on_language,
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
        dialog
            .title(ts!("app.settings.title"))
            .w(DIALOG_WIDTH)
            .child(
                v_flex()
                    .w_full()
                    .gap_4()
                    .child(settings_section(
                        ts!("app.settings.section_transcription"),
                        [settings_row(
                            model_title,
                            model.description,
                            Button::new("settings-change-model")
                                .outline()
                                .small()
                                .label(SharedString::from(model.button))
                                .on_click(on_change_model.clone()),
                            cx,
                        )],
                        cx,
                    ))
                    .child(settings_section(
                        ts!("app.settings.section_recording"),
                        [settings_row(
                            ts!("app.settings.max_duration_title"),
                            ts!("app.settings.max_duration_description"),
                            max_duration_menu(
                                view.max_duration,
                                view.language,
                                on_max_duration.clone(),
                            ),
                            cx,
                        )],
                        cx,
                    ))
                    .child(settings_section(
                        ts!("app.settings.section_general"),
                        [
                            settings_row(
                                ts!("app.settings.language_title"),
                                ts!("app.settings.language_description"),
                                language_menu(view.language, on_language.clone()),
                                cx,
                            ),
                            settings_row(
                                ts!("app.settings.close_to_tray_title"),
                                ts!("app.settings.close_to_tray_description"),
                                close_to_tray_switch(view.close_to_tray, on_close_to_tray.clone()),
                                cx,
                            ),
                        ],
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
    language: Language,
    on_pick: impl Fn(MaxMeetingDuration, &mut Window, &mut App) + Clone + 'static,
) -> impl IntoElement {
    let choices = duration_choices(selected, language);
    Button::new("settings-max-duration")
        .outline()
        .small()
        .label(SharedString::from(duration_label(selected, language)))
        .dropdown_caret(true)
        .accessibility_label(ts!("app.settings.max_duration_title"))
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

fn language_menu(
    selected: Language,
    on_pick: impl Fn(Language, &mut Window, &mut App) + Clone + 'static,
) -> impl IntoElement {
    let choices = language_choices(selected);
    Button::new("settings-language")
        .outline()
        .small()
        .label(selected.native_name())
        .dropdown_caret(true)
        .accessibility_label(ts!("app.settings.language_title"))
        .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _window, _cx| {
            choices.iter().fold(menu, |menu, choice| {
                let pick = on_pick.clone();
                let language = choice.language;
                menu.item(
                    PopupMenuItem::new(choice.label)
                        .checked(choice.checked)
                        .on_click(move |_event, window, cx| pick(language, window, cx)),
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
        .accessibility_label(ts!("app.settings.close_to_tray_title"))
        .on_change(on_change)
}

/// Reached only through the reserved `ShowAbout` action. Settings shows the version itself.
pub(crate) fn open_about(window: &mut Window, cx: &mut App) {
    let version = SharedString::from(
        t!("app.settings.version", version = env!("CARGO_PKG_VERSION")).into_owned(),
    );
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
            .title(ts!("app.settings.still_running_title"))
            .w(DIALOG_WIDTH)
            .description(ts!("app.settings.still_running_body"))
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
            .title(ts!("app.settings.exit_title"))
            .w(DIALOG_WIDTH)
            .description(ts!("app.settings.exit_body"))
            .confirm()
            .ok_text(ts!("app.settings.exit_confirm"))
            .on_ok(move |_, window, cx| {
                on_confirm(window, cx);
                true
            })
    });
}

#[cfg(test)]
mod tests {
    use super::{
        copyright_text, duration_label, language_choices, settings_model_row, version_text,
        LanguageChoice, SettingsModelRow, REPO_URL,
    };
    use crate::domain::language::Language;
    use crate::domain::recording::MaxMeetingDuration;
    use crate::views::state::ModelStatus;

    #[test]
    fn row_shows_the_model_in_use_with_its_size() {
        // Arrange
        let status = ModelStatus::InUse {
            id: "nemotron-3.5-streaming-0.6b-q8_0".to_string(),
        };

        // Act
        let row = settings_model_row(&status, Some(983_696_512), Language::English);

        // Assert
        assert_eq!(
            row,
            SettingsModelRow {
                title: "Nemotron 3.5 (Multilingual)".to_string(),
                description: "Accurate in 40 languages, including English and Swedish · 938.1 MB"
                    .to_string(),
                button: "Change model…".to_string(),
                in_use: true,
            }
        );
    }

    #[test]
    fn row_offers_a_choice_when_nothing_is_installed() {
        // Arrange
        let status = ModelStatus::Missing;

        // Act
        let row = settings_model_row(&status, None, Language::English);

        // Assert
        assert_eq!(row.title, "No model installed");
        assert_eq!(row.button, "Choose model…");
        assert!(!row.in_use);
    }

    #[test]
    fn swedish_row_translates_the_text_and_keeps_the_model_name() {
        // Arrange
        let status = ModelStatus::InUse {
            id: "nemotron-3.5-streaming-0.6b-q8_0".to_string(),
        };

        // Act
        let row = settings_model_row(&status, Some(983_696_512), Language::Swedish);

        // Assert
        assert_eq!(row.title, "Nemotron 3.5 (Multilingual)");
        assert_eq!(
            row.description,
            "Noggrann på 40 språk, bland annat engelska och svenska · 938,1 MB"
        );
        assert_eq!(row.button, "Byt modell…");
    }

    #[test]
    fn duration_choices_list_every_option_and_check_the_selected_one() {
        // Arrange
        let selected = crate::domain::recording::MaxMeetingDuration::default();

        // Act
        let choices = super::duration_choices(selected, Language::English);

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
            rust_i18n::t!("app.settings.max_duration_description"),
            "Stops a forgotten recording so it does not write a very long file."
        );
    }

    #[test]
    fn duration_labels_use_the_plural_each_language_needs() {
        // Arrange
        let one = MaxMeetingDuration::from_hours(1).expect("one");
        let three = MaxMeetingDuration::default();

        // Act
        let english = (
            duration_label(one, Language::English),
            duration_label(three, Language::English),
        );
        let swedish = (
            duration_label(one, Language::Swedish),
            duration_label(three, Language::Swedish),
        );

        // Assert
        assert_eq!(english, ("1 hour".to_string(), "3 hours".to_string()));
        assert_eq!(swedish, ("1 timme".to_string(), "3 timmar".to_string()));
    }

    #[test]
    fn ukrainian_hours_use_three_plural_forms() {
        // Arrange
        let hours = [1, 2, 4, 6, 12];

        // Act
        let labels: Vec<_> = hours
            .iter()
            .map(|h| {
                duration_label(
                    MaxMeetingDuration::from_hours(*h).expect("choice"),
                    Language::Ukrainian,
                )
            })
            .collect();

        // Assert
        assert_eq!(
            labels,
            ["1 година", "2 години", "4 години", "6 годин", "12 годин"]
        );
    }

    #[test]
    fn settings_view_shows_the_current_language() {
        // Arrange
        let selected = Language::Swedish;

        // Act
        let choices = language_choices(selected);

        // Assert
        assert_eq!(
            choices,
            vec![
                LanguageChoice {
                    label: "English",
                    language: Language::English,
                    checked: false,
                },
                LanguageChoice {
                    label: "Svenska",
                    language: Language::Swedish,
                    checked: true,
                },
                LanguageChoice {
                    label: "Українська",
                    language: Language::Ukrainian,
                    checked: false,
                },
            ]
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
