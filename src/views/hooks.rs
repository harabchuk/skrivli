//! Process hooks injected into the shell. The window never names app workers.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::infrastructure::storage::settings::Settings;
use crate::services::ports::{SharedTrayText, TrayText};

/// Save and exit callbacks owned by the process. Bootstrap fills them.
pub(crate) struct ShellHooks {
    save_settings: Arc<dyn Fn(Settings) + Send + Sync>,
    request_exit: Arc<dyn Fn() + Send + Sync>,
    exit_requested: Arc<AtomicBool>,
    tray_text: SharedTrayText,
}

impl ShellHooks {
    pub(crate) fn new(
        save_settings: Arc<dyn Fn(Settings) + Send + Sync>,
        request_exit: Arc<dyn Fn() + Send + Sync>,
        exit_requested: Arc<AtomicBool>,
        tray_text: SharedTrayText,
    ) -> Self {
        Self {
            save_settings,
            request_exit,
            exit_requested,
            tray_text,
        }
    }

    pub(crate) fn save_settings(&self, settings: Settings) {
        (self.save_settings)(settings);
    }

    /// Replaces the text the tray menu and balloon show. The tray thread reads it on use.
    pub(crate) fn set_tray_text(&self, text: TrayText) {
        let mut shared = self
            .tray_text
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *shared = text;
    }

    pub(crate) fn request_exit(&self) {
        (self.request_exit)();
    }

    pub(crate) fn exit_requested(&self) -> bool {
        self.exit_requested.load(Ordering::Acquire)
    }
}
