//! The interface language the user picks in Settings.

use serde::{Deserialize, Deserializer, Serialize};

/// Languages the interface is translated into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Language {
    #[default]
    English,
    Swedish,
    Spanish,
}

impl Language {
    /// Every language, in the order Settings lists them.
    pub(crate) const ALL: [Language; 3] = [Language::English, Language::Swedish, Language::Spanish];

    /// BCP 47 code used for the translation catalog and for ICU4X.
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::Swedish => "sv",
            Self::Spanish => "es",
        }
    }

    /// The language's own name. It is never translated, so a user who
    /// cannot read the current language can still find theirs.
    pub(crate) fn native_name(self) -> &'static str {
        match self {
            Self::English => "English",
            Self::Swedish => "Svenska",
            Self::Spanish => "Español",
        }
    }
}

/// A stored value this build does not know reads as English, so a settings file
/// from a build with more languages still opens.
impl<'de> Deserialize<'de> for Language {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Ok(match text.as_str() {
            "swedish" => Self::Swedish,
            "spanish" => Self::Spanish,
            _ => Self::English,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::Language;

    #[test]
    fn language_codes_and_native_names() {
        // Arrange
        let languages = Language::ALL;

        // Act
        let codes: Vec<_> = languages.iter().map(|l| l.code()).collect();
        let names: Vec<_> = languages.iter().map(|l| l.native_name()).collect();

        // Assert
        assert_eq!(codes, ["en", "sv", "es"]);
        assert_eq!(names, ["English", "Svenska", "Español"]);
        assert_eq!(Language::default(), Language::English);
    }

    #[test]
    fn unknown_language_in_settings_reads_as_english() {
        // Arrange
        let known = r#""swedish""#;
        let unknown = r#""fr""#;

        // Act
        let swedish: Language = serde_json::from_str(known).expect("known");
        let fallback: Language = serde_json::from_str(unknown).expect("unknown");

        // Assert
        assert_eq!(swedish, Language::Swedish);
        assert_eq!(fallback, Language::English);
        assert_eq!(
            serde_json::to_string(&Language::Swedish).expect("serialize"),
            known
        );
    }

    #[test]
    fn spanish_in_settings_round_trips() {
        // Arrange
        let stored = r#""spanish""#;

        // Act
        let spanish: Language = serde_json::from_str(stored).expect("known");

        // Assert
        assert_eq!(spanish, Language::Spanish);
        assert_eq!(
            serde_json::to_string(&Language::Spanish).expect("serialize"),
            stored
        );
    }
}
