//! Per-session single-instance mutex.
//!
//! The production name is [`SINGLE_INSTANCE_MUTEX`]. A second process that loses
//! the race logs and exits without creating a window or touching recordings.
//!
//! M1 activation (not implemented): the loser should signal the winner with
//! `RegisterWindowMessageW` (or a small authenticated local IPC) asking it to
//! show and focus, then exit. If the winner has not created its window yet,
//! it must remember a pending show and apply it when the window exists.
//! Releasing this guard on `Drop` is the only lifetime behavior implemented now.
//! `ReleaseMutex` runs on the acquiring thread; the guard is thread-affine.

use std::marker::PhantomData;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE};
use windows::Win32::System::Threading::{CreateMutexW, ReleaseMutex};

use crate::common::error::AppError;

pub(crate) const SINGLE_INSTANCE_MUTEX: &str = r"Local\Skrivli.SingleInstance";

pub(crate) enum InstanceGuardError {
    AlreadyRunning,
    Failed(AppError),
}

/// Owns the named mutex until dropped.
pub(crate) struct SingleInstance {
    handle: HANDLE,
    _thread: PhantomData<*const ()>,
}

impl SingleInstance {
    pub(crate) fn acquire(name: &str) -> Result<Self, InstanceGuardError> {
        if name.is_empty() {
            return Err(InstanceGuardError::Failed(AppError::message(
                "single instance",
                "mutex name is empty",
            )));
        }
        let wide = wide(name);
        // SAFETY: `wide` is NUL-terminated. Initial ownership is granted only
        // when this call creates the mutex; an existing mutex sets
        // ERROR_ALREADY_EXISTS and leaves ownership with the other process.
        let handle = unsafe { CreateMutexW(None, true, PCWSTR(wide.as_ptr())) };
        let handle = match handle {
            Ok(handle) => handle,
            Err(error) => {
                return Err(InstanceGuardError::Failed(AppError::message(
                    "single instance",
                    error.to_string(),
                )));
            }
        };
        // SAFETY: read immediately after CreateMutexW, before another Win32 call.
        let already_exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        if already_exists {
            // SAFETY: the handle is from CreateMutexW and this thread does not own it.
            unsafe {
                let _ = CloseHandle(handle);
            }
            return Err(InstanceGuardError::AlreadyRunning);
        }
        Ok(Self {
            handle,
            _thread: PhantomData,
        })
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        // SAFETY: the guard is !Send, so Drop runs on the thread that acquired
        // the mutex. ReleaseMutex requires that thread.
        unsafe {
            if let Err(error) = ReleaseMutex(self.handle) {
                tracing::error!(%error, "ReleaseMutex failed");
            }
            if let Err(error) = CloseHandle(self.handle) {
                tracing::error!(%error, "CloseHandle failed");
            }
        }
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::{InstanceGuardError, SingleInstance};

    fn unique_name() -> String {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        format!(r"Local\Skrivli.Test.{}.{}", std::process::id(), id)
    }

    #[test]
    fn second_guard_fails_until_the_first_drops() {
        // Arrange
        let name = unique_name();

        // Act
        let first = SingleInstance::acquire(&name);
        let second = SingleInstance::acquire(&name);

        // Assert
        assert!(first.is_ok());
        assert!(matches!(second, Err(InstanceGuardError::AlreadyRunning)));

        drop(first);
        let third = SingleInstance::acquire(&name);
        assert!(third.is_ok());
    }

    #[test]
    fn empty_mutex_name_is_rejected() {
        // Arrange
        let name = "";

        // Act
        let acquired = SingleInstance::acquire(name);

        // Assert
        assert!(matches!(acquired, Err(InstanceGuardError::Failed(_))));
    }
}
