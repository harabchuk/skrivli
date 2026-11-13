//! The interface language the user picks in Settings.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Languages the interface is translated into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Language {
    #[default]
    English,
    Spanish,
    Swedish,
    Russian,
}

impl Language {
    /// Every language, in the order Settings lists them.
    pub(crate) const ALL: [Language; 4] = [
        Language::English,
        Language::Spanish,
        Language::Swedish,
        Language::Russian,
    ];

    /// BCP 47 code used for the translation catalog and for ICU4X.
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::Spanish => "es",
            Self::Swedish => "sv",
            Self::Russian => "ru",
        }
    }

    /// The language's own name. It is never translated, so a user who
    /// cannot read the current language can still find theirs.
    pub(crate) fn native_name(self) -> &'static str {
        match self {
            Self::English => "English",
            Self::Spanish => "Español",
            Self::Swedish => "Svenska",
            Self::Russian => "Русский",
        }
    }
}

impl Language {
    /// The name stored in settings. It matches the serde `snake_case` name.
    fn storage_name(self) -> &'static str {
        match self {
            Self::English => "english",
            Self::Spanish => "spanish",
            Self::Swedish => "swedish",
            Self::Russian => "russian",
        }
    }

    /// The language stored under `name`, if this build knows it.
    fn from_storage_name(name: &str) -> Option<Language> {
        Self::ALL
            .into_iter()
            .find(|language| language.storage_name() == name)
    }

    /// The supported language for a locale tag such as `sv-SE`, `sv_FI` or `SV`.
    /// Only the primary subtag counts.
    pub(crate) fn from_tag(tag: &str) -> Option<Language> {
        let primary = tag.split(['-', '_']).next().unwrap_or_default();
        Self::ALL
            .into_iter()
            .find(|language| language.code().eq_ignore_ascii_case(primary))
    }
}

/// What the user chose in Settings: follow Windows, or one pinned language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum LanguagePreference {
    #[default]
    System,
    Fixed(Language),
}

impl LanguagePreference {
    /// The language to show. `system_tags` is the Windows list in preference
    /// order; the first supported one wins, otherwise English.
    pub(crate) fn resolve(self, system_tags: &[String]) -> Language {
        let system = system_tags
            .iter()
            .find_map(|tag| Language::from_tag(tag))
            .unwrap_or_default();
        self.with_system(system)
    }

    /// The language to show once the system language is already known.
    pub(crate) fn with_system(self, system: Language) -> Language {
        match self {
            Self::Fixed(language) => language,
            Self::System => system,
        }
    }
}

impl Serialize for LanguagePreference {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::System => serializer.serialize_str("system"),
            Self::Fixed(language) => language.serialize(serializer),
        }
    }
}

/// `"system"` follows Windows. Any other value reads as a language, so an
/// unknown one is English, as for [`Language`].
impl<'de> Deserialize<'de> for LanguagePreference {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Ok(match text.as_str() {
            "system" => Self::System,
            name => Self::Fixed(Language::from_storage_name(name).unwrap_or_default()),
        })
    }
}

/// A stored value this build does not know reads as English, so a settings file
/// from a build with more languages still opens.
impl<'de> Deserialize<'de> for Language {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Ok(Self::from_storage_name(&text).unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::{Language, LanguagePreference};

    fn tags(list: &[&str]) -> Vec<String> {
        list.iter().map(|t| t.to_string()).collect()
    }

    #[test]
    fn language_from_tag_matches_primary_subtag() {
        // Arrange
        let swedish = ["sv-SE", "sv_FI", "SV"];

        // Act
        let matched: Vec<_> = swedish.iter().map(|t| Language::from_tag(t)).collect();

        // Assert
        assert!(matched.iter().all(|m| *m == Some(Language::Swedish)));
        assert_eq!(Language::from_tag("en-GB"), Some(Language::English));
        assert_eq!(Language::from_tag("pl-PL"), None);
        assert_eq!(Language::from_tag(""), None);
    }

    #[test]
    fn system_preference_picks_first_supported_language() {
        // Arrange
        let system = tags(&["pl-PL", "sv-SE", "en-US"]);

        // Act
        let language = LanguagePreference::System.resolve(&system);

        // Assert
        assert_eq!(language, Language::Swedish);
    }

    #[test]
    fn system_preference_falls_back_to_english() {
        // Arrange
        let unsupported = tags(&["pl-PL"]);

        // Act
        let from_unsupported = LanguagePreference::System.resolve(&unsupported);
        let from_empty = LanguagePreference::System.resolve(&[]);

        // Assert
        assert_eq!(from_unsupported, Language::English);
        assert_eq!(from_empty, Language::English);
    }

    #[test]
    fn fixed_preference_ignores_windows() {
        // Arrange
        let system = tags(&["en-US"]);

        // Act
        let language = LanguagePreference::Fixed(Language::Swedish).resolve(&system);

        // Assert
        assert_eq!(language, Language::Swedish);
    }

    #[test]
    fn with_system_uses_known_system_language_only_for_system() {
        // Arrange
        let system = Language::Swedish;

        // Act
        let followed = LanguagePreference::System.with_system(system);
        let pinned = LanguagePreference::Fixed(Language::English).with_system(system);

        // Assert
        assert_eq!(followed, Language::Swedish);
        assert_eq!(pinned, Language::English);
    }

    #[test]
    fn language_preference_serialises_as_before() {
        // Arrange
        let cases = [
            ("\"system\"", LanguagePreference::System),
            ("\"english\"", LanguagePreference::Fixed(Language::English)),
            ("\"swedish\"", LanguagePreference::Fixed(Language::Swedish)),
        ];

        // Act / Assert
        for (text, preference) in cases {
            let read: LanguagePreference = serde_json::from_str(text).expect("read");
            assert_eq!(read, preference);
            assert_eq!(serde_json::to_string(&preference).expect("write"), text);
        }
        let unknown: LanguagePreference = serde_json::from_str("\"fr\"").expect("unknown");
        assert_eq!(unknown, LanguagePreference::Fixed(Language::English));
    }

    #[test]
    fn language_codes_and_native_names() {
        // Arrange
        let languages = Language::ALL;

        // Act
        let codes: Vec<_> = languages.iter().map(|l| l.code()).collect();
        let names: Vec<_> = languages.iter().map(|l| l.native_name()).collect();

        // Assert
        assert_eq!(codes, ["en", "es", "sv", "ru"]);
        assert_eq!(names, ["English", "Español", "Svenska", "Русский"]);
        assert_eq!(Language::default(), Language::English);
    }

    #[test]
    fn new_languages_round_trip_in_settings() {
        // Arrange
        let cases = [
            ("\"spanish\"", Language::Spanish),
            ("\"russian\"", Language::Russian),
        ];

        // Act / Assert
        for (text, language) in cases {
            let read: Language = serde_json::from_str(text).expect("read language");
            let preference: LanguagePreference = serde_json::from_str(text).expect("read choice");
            assert_eq!(read, language);
            assert_eq!(preference, LanguagePreference::Fixed(language));
            assert_eq!(serde_json::to_string(&language).expect("write"), text);
            assert_eq!(serde_json::to_string(&preference).expect("write"), text);
        }
    }

    #[test]
    fn system_tags_pick_the_new_languages() {
        // Arrange
        let cases = [("es-419", Language::Spanish), ("ru-RU", Language::Russian)];

        // Act / Assert
        for (tag, language) in cases {
            let resolved = LanguagePreference::System.resolve(&tags(&[tag]));
            assert_eq!(resolved, language);
        }
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
}
