//! Theme preference cycle used by settings and the shell button.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ThemePreference {
    System,
    Light,
    Dark,
}

impl ThemePreference {
    /// Next preference in the theme-button cycle.
    ///
    /// Order is `[System, Light, Dark]` when Windows is dark, or
    /// `[System, Dark, Light]` when Windows is light (Light and Dark swap).
    pub(crate) fn next(self, system_is_dark: bool) -> Self {
        if system_is_dark {
            match self {
                Self::System => Self::Light,
                Self::Light => Self::Dark,
                Self::Dark => Self::System,
            }
        } else {
            match self {
                Self::System => Self::Dark,
                Self::Dark => Self::Light,
                Self::Light => Self::System,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ThemePreference;

    #[test]
    fn cycles_system_dark_light_when_os_is_light() {
        // Arrange
        let os_dark = false;

        // Act
        let dark = ThemePreference::System.next(os_dark);
        let light = dark.next(os_dark);
        let system = light.next(os_dark);

        // Assert
        assert_eq!(dark, ThemePreference::Dark);
        assert_eq!(light, ThemePreference::Light);
        assert_eq!(system, ThemePreference::System);
    }

    #[test]
    fn cycles_system_light_dark_when_os_is_dark() {
        // Arrange
        let os_dark = true;

        // Act
        let light = ThemePreference::System.next(os_dark);
        let dark = light.next(os_dark);
        let system = dark.next(os_dark);

        // Assert
        assert_eq!(light, ThemePreference::Light);
        assert_eq!(dark, ThemePreference::Dark);
        assert_eq!(system, ThemePreference::System);
    }
}
