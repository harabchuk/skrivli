//! OS host selection. One binary contains one host; `target_os` chooses it.

#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub(crate) use windows::{
    clock, completion_notifier, disk, host, performance_counter, replace_file, spawn_device_watch,
    user_paths, PlatformHost,
};

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
pub(crate) use macos::{
    clock, completion_notifier, disk, host, performance_counter, replace_file, spawn_device_watch,
    user_paths, PlatformHost,
};

#[cfg(not(any(windows, target_os = "macos")))]
compile_error!("add infrastructure/platform/<os> and select it in platform/mod.rs");
