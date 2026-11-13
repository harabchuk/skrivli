//! Picks the interface language from the system locale.

use crate::domain::language::{Language, LanguagePreference};
use crate::services::ports::SystemLocale;

/// The language Windows asks for: the first of its display languages that the
/// interface is translated into, otherwise English.
pub(crate) fn system_language(locale: &dyn SystemLocale) -> Language {
    LanguagePreference::System.resolve(&locale.preferred_tags())
}

#[cfg(test)]
mod tests {
    use super::system_language;
    use crate::domain::language::Language;
    use crate::services::ports::SystemLocale;

    struct FakeLocale(Vec<String>);

    impl SystemLocale for FakeLocale {
        fn preferred_tags(&self) -> Vec<String> {
            self.0.clone()
        }
    }

    #[test]
    fn system_language_uses_fake_locale() {
        // Arrange
        let swedish = FakeLocale(vec!["pl-PL".into(), "sv-SE".into()]);
        let nothing = FakeLocale(Vec::new());

        // Act
        let from_list = system_language(&swedish);
        let from_nothing = system_language(&nothing);

        // Assert
        assert_eq!(from_list, Language::Swedish);
        assert_eq!(from_nothing, Language::English);
    }
}
