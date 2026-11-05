//! Windows directory change notifications with debounce and full refresh.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::Storage::FileSystem::{
    FindCloseChangeNotification, FindFirstChangeNotificationW, FindNextChangeNotification,
    FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE,
    FILE_NOTIFY_CHANGE_SIZE,
};
use windows::Win32::System::Threading::WaitForSingleObject;

use crate::common::error::AppError;
use crate::services::ports::{DirectoryWatch, WatchEvent};

const WAIT_SLICE: Duration = Duration::from_millis(200);
const FULL_REFRESH_FALLBACK: Duration = Duration::from_secs(30);

pub(super) struct WindowsDirectoryWatch;

impl DirectoryWatch for WindowsDirectoryWatch {
    fn spawn(
        &self,
        directory: PathBuf,
        stop: Arc<AtomicBool>,
        debounce: Duration,
    ) -> (Receiver<WatchEvent>, JoinHandle<()>) {
        let (tx, rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            watch_loop(directory, stop, debounce, tx);
        });
        (rx, handle)
    }
}

fn watch_loop(
    directory: PathBuf,
    stop: Arc<AtomicBool>,
    debounce: Duration,
    tx: Sender<WatchEvent>,
) {
    let mut pending: Option<Instant> = None;
    let mut last_poll = Instant::now();
    let mut watch = open_watch(&directory);

    while !stop.load(Ordering::Acquire) {
        if let Some(at) = pending {
            if at.elapsed() >= debounce {
                pending = None;
                let _ = tx.send(WatchEvent::Changed);
            }
        }

        if last_poll.elapsed() >= FULL_REFRESH_FALLBACK {
            last_poll = Instant::now();
            let _ = tx.send(WatchEvent::Poll);
            if watch.is_none() {
                watch = open_watch(&directory);
            }
        }

        match watch.as_mut() {
            Some(handle) => match handle.wait(WAIT_SLICE) {
                WaitOutcome::Signaled => {
                    pending = Some(Instant::now());
                    if !handle.arm_next() {
                        tracing::warn!("directory watch was lost; falling back to polling");
                        watch = None;
                    }
                }
                WaitOutcome::Timeout => {}
                WaitOutcome::Failed => {
                    tracing::warn!("directory watch wait failed; falling back to polling");
                    watch = None;
                }
            },
            None => thread::sleep(WAIT_SLICE),
        }
    }
}

enum WaitOutcome {
    Signaled,
    Timeout,
    Failed,
}

struct WatchHandle {
    handle: HANDLE,
}

impl WatchHandle {
    fn open(directory: &Path) -> Result<Self, AppError> {
        if !directory.is_dir() {
            return Err(AppError::message("library watch", "folder is missing"));
        }
        let wide = wide_path(directory);
        // SAFETY: path is NUL-terminated for the duration of the call.
        let handle = unsafe {
            FindFirstChangeNotificationW(
                PCWSTR(wide.as_ptr()),
                false,
                FILE_NOTIFY_CHANGE_FILE_NAME
                    | FILE_NOTIFY_CHANGE_DIR_NAME
                    | FILE_NOTIFY_CHANGE_SIZE
                    | FILE_NOTIFY_CHANGE_LAST_WRITE,
            )
        }
        .map_err(|error| AppError::message("library watch", error.to_string()))?;
        Ok(Self { handle })
    }

    fn wait(&self, timeout: Duration) -> WaitOutcome {
        let ms = timeout.as_millis().min(u32::MAX as u128) as u32;
        // SAFETY: handle is owned by this watcher for its lifetime.
        let result = unsafe { WaitForSingleObject(self.handle, ms) };
        if result == WAIT_OBJECT_0 {
            WaitOutcome::Signaled
        } else if result == WAIT_TIMEOUT {
            WaitOutcome::Timeout
        } else {
            WaitOutcome::Failed
        }
    }

    fn arm_next(&self) -> bool {
        // SAFETY: handle is valid until Drop.
        unsafe { FindNextChangeNotification(self.handle) }.is_ok()
    }
}

impl Drop for WatchHandle {
    fn drop(&mut self) {
        // SAFETY: FindCloseChangeNotification closes the owned handle once.
        unsafe {
            let _ = FindCloseChangeNotification(self.handle);
        }
    }
}

fn open_watch(directory: &Path) -> Option<WatchHandle> {
    match WatchHandle::open(directory) {
        Ok(handle) => Some(handle),
        Err(error) => {
            tracing::debug!(error = %error, "directory watch unavailable");
            None
        }
    }
}

fn wide_path(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use super::WindowsDirectoryWatch;
    use crate::services::ports::{DirectoryWatch, WatchEvent};

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("skrivli-watch-{tag}-{id}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("dir");
        dir
    }

    #[test]
    fn watcher_emits_change_after_create() {
        // Arrange
        let dir = temp_dir("chg");
        let stop = Arc::new(AtomicBool::new(false));
        let watch = WindowsDirectoryWatch;
        let (rx, handle) = watch.spawn(dir.clone(), Arc::clone(&stop), Duration::from_millis(50));
        // Let FindFirstChangeNotification arm before the create.
        std::thread::sleep(Duration::from_millis(100));

        // Act
        fs::write(dir.join("note.txt"), "hi").expect("write");
        let mut saw_change = false;
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if let Ok(event) = rx.recv_timeout(Duration::from_millis(100)) {
                if matches!(event, WatchEvent::Changed | WatchEvent::Poll) {
                    saw_change = true;
                    break;
                }
            }
        }
        stop.store(true, Ordering::Release);
        let _ = handle.join();

        // Assert
        assert!(saw_change, "expected a watch event after creating a file");
        let _ = fs::remove_dir_all(dir);
    }
}
