//! Process hooks injected into the shell. The window never names app workers.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::infrastructure::storage::settings::Settings;

/// Save and exit callbacks owned by the process. Bootstrap fills them.
pub(crate) struct ShellHooks {
    save_settings: Arc<dyn Fn(Settings) + Send + Sync>,
    request_exit: Arc<dyn Fn() + Send + Sync>,
    exit_requested: Arc<AtomicBool>,
}

impl ShellHooks {
    pub(crate) fn new(
        save_settings: Arc<dyn Fn(Settings) + Send + Sync>,
        request_exit: Arc<dyn Fn() + Send + Sync>,
        exit_requested: Arc<AtomicBool>,
    ) -> Self {
        Self {
            save_settings,
            request_exit,
            exit_requested,
        }
    }

    pub(crate) fn save_settings(&self, settings: Settings) {
        (self.save_settings)(settings);
    }

    pub(crate) fn request_exit(&self) {
        (self.request_exit)();
    }

    pub(crate) fn exit_requested(&self) -> bool {
        self.exit_requested.load(Ordering::Acquire)
    }
}
