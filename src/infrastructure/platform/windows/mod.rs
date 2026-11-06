//! Windows host: the only module that may call Win32 / COM in this crate.

mod device_wake;
mod notify;
mod paths;
mod replace;
mod shell;
mod single_instance;
mod time_and_disk;
mod trash;
mod tray;
mod watcher;

use std::sync::Arc;

use crate::common::error::AppError;
use crate::common::paths::AppPaths;
use crate::infrastructure::audio::devices::DeviceCatalog;
use crate::services::ports::{
    DesktopShell, DirectoryWatch, FreeSpace, PerformanceCounter, RecordingClock, SingleInstance,
    Trash, Tray,
};

use self::shell::WindowsShell;
use self::single_instance::WindowsSingleInstance;
use self::time_and_disk::{QpcCounter, SystemClock, WindowsDisk};
use self::trash::WindowsTrash;
use self::tray::WindowsTray;
use self::watcher::WindowsDirectoryWatch;

pub(crate) struct PlatformHost {
    pub(crate) shell: Arc<dyn DesktopShell>,
    pub(crate) trash: Arc<dyn Trash>,
    pub(crate) watch: Arc<dyn DirectoryWatch>,
    pub(crate) single_instance: Arc<dyn SingleInstance>,
    pub(crate) tray: Arc<dyn Tray>,
}

pub(crate) use notify::completion_notifier;

pub(crate) fn host() -> PlatformHost {
    PlatformHost {
        shell: Arc::new(WindowsShell),
        trash: Arc::new(WindowsTrash),
        watch: Arc::new(WindowsDirectoryWatch),
        single_instance: Arc::new(WindowsSingleInstance),
        tray: Arc::new(WindowsTray),
    }
}

pub(crate) fn replace_file(
    replacement: &std::path::Path,
    replaced: &std::path::Path,
    context: &'static str,
) -> Result<(), AppError> {
    replace::replace_file(replacement, replaced, context)
}

pub(crate) fn user_paths() -> Result<AppPaths, AppError> {
    paths::user_paths()
}

pub(crate) fn spawn_device_watch(
    emit: Box<dyn Fn(DeviceCatalog) + Send>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> std::thread::JoinHandle<()> {
    device_wake::spawn_device_watch(emit, stop)
}

pub(crate) fn clock() -> Box<dyn RecordingClock> {
    Box::new(SystemClock)
}

pub(crate) fn disk() -> Box<dyn FreeSpace> {
    Box::new(WindowsDisk)
}

pub(crate) fn performance_counter() -> Box<dyn PerformanceCounter> {
    Box::new(QpcCounter)
}
