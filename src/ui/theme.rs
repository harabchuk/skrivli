//! Theme preference helpers on top of GPUI Kit light/dark modes.
//!
//! Kit’s `icons/sun-moon.svg` does not paint under GPUI (compound arcs in that
//! asset). System uses [`Icon::data`] with the working moon crescent plus sun
//! rays so the control still reads as sun-and-moon / follow system.

use gpui_kit::component::{Icon, IconName, Theme, ThemeMode};
use gpui_kit::{App, Window};

use crate::infrastructure::storage::settings::ThemePreference;

pub(crate) fn theme_icon(preference: ThemePreference) -> Icon {
    match preference {
        ThemePreference::Light => Icon::new(IconName::Sun),
        ThemePreference::Dark => Icon::new(IconName::Moon),
        ThemePreference::System => Icon::new(IconName::Palette),
    }
}

pub(crate) fn apply_theme(preference: ThemePreference, window: Option<&mut Window>, cx: &mut App) {
    match preference {
        ThemePreference::Light => Theme::change(ThemeMode::Light, window, cx),
        ThemePreference::Dark => Theme::change(ThemeMode::Dark, window, cx),
        ThemePreference::System => Theme::sync_system_appearance(window, cx),
    }
}
