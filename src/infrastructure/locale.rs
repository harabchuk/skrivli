//! Reads the display-language list through `sys-locale`.

use crate::services::ports::SystemLocale;

/// The live machine locale. Not unit-tested: it reads the real OS state.
pub(crate) struct SysLocale;

impl SystemLocale for SysLocale {
    fn preferred_tags(&self) -> Vec<String> {
        let tags: Vec<String> = sys_locale::get_locales().collect();
        tracing::debug!(?tags, "system locales");
        tags
    }
}
