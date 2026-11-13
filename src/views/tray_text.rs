//! The tray menu and balloon text in a chosen language.

use rust_i18n::t;

use crate::domain::language::Language;
use crate::services::ports::TrayText;

/// All tray text for `language`. The tray thread receives it as a plain value.
pub(crate) fn tray_text(language: Language) -> TrayText {
    let locale = language.code();
    TrayText {
        show: t!("app.tray.show", locale = locale).into_owned(),
        start_recording: t!("app.tray.start_recording", locale = locale).into_owned(),
        stop_recording: t!("app.tray.stop_recording", locale = locale).into_owned(),
        open_recordings_folder: t!("app.tray.open_recordings_folder", locale = locale).into_owned(),
        settings: t!("app.tray.settings", locale = locale).into_owned(),
        exit: t!("app.tray.exit", locale = locale).into_owned(),
        balloon_title: t!("app.tray.balloon_title", locale = locale).into_owned(),
        balloon_body: t!("app.tray.balloon_body", locale = locale).into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::tray_text;
    use crate::domain::language::Language;
    use crate::services::ports::TrayText;

    #[test]
    fn tray_text_for_english_matches_the_original_menu() {
        // Arrange
        let language = Language::English;

        // Act
        let text = tray_text(language);

        // Assert
        assert_eq!(
            text,
            TrayText {
                show: "Show Skrivli".to_string(),
                start_recording: "Start recording".to_string(),
                stop_recording: "Stop recording".to_string(),
                open_recordings_folder: "Open recordings folder".to_string(),
                settings: "Settings".to_string(),
                exit: "Exit".to_string(),
                balloon_title: "Transcript ready".to_string(),
                balloon_body: "A recording has a transcript.".to_string(),
            }
        );
    }

    #[test]
    fn tray_text_for_russian_keeps_non_ascii_text() {
        // Arrange
        let language = Language::Russian;

        // Act
        let text = tray_text(language);

        // Assert
        assert_eq!(text.show, "Показать Skrivli");
        assert_eq!(text.balloon_title, "Расшифровка готова");
        assert_ne!(text, tray_text(Language::English));
    }

    #[test]
    fn tray_text_for_swedish() {
        // Arrange
        let language = Language::Swedish;

        // Act
        let text = tray_text(language);

        // Assert
        assert_eq!(text.show, "Visa Skrivli");
        assert_eq!(text.exit, "Avsluta");
        assert_eq!(text.balloon_title, "Transkriptionen är klar");
        assert_ne!(text, tray_text(Language::English));
    }
}
