//! Skrivli application library. The GPUI thread must not block on audio, disk, or inference.

pub(crate) mod app;

/// Process entry used by the binary. Returns a process exit code.
pub fn run() -> i32 {
    app::run()
}
pub(crate) mod common;
pub(crate) mod domain;
pub(crate) mod infrastructure;
pub(crate) mod services;
pub(crate) mod ui;
pub(crate) mod views;
