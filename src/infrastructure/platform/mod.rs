//! OS host selection. One binary contains one host; `target_os` chooses it.

#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub(crate) use windows::{
    clock, disk, host, performance_counter, replace_file, spawn_device_watch, user_paths,
    PlatformHost,
};

#[cfg(not(windows))]
compile_error!("add infrastructure/platform/<os> and select it in platform/mod.rs");
