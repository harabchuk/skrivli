//! Per-session single-instance mutex and the show/focus event.
//!
//! The production mutex is [`SINGLE_INSTANCE_MUTEX`]. The loser signals
//! [`ACTIVATE_EVENT_NAME`] and exits without creating a window or touching
//! recordings. The winner creates that auto-reset event before its window
//! exists, so a signal that arrives early stays pending until the shell drains
//! it. `ReleaseMutex` runs on the acquiring thread; the mutex guard is
//! thread-affine. The event handle may be waited on from the listener thread.

use std::marker::PhantomData;
use std::thread;
use std::time::{Duration, Instant};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, WAIT_OBJECT_0,
};
use windows::Win32::System::Threading::{
    CreateEventW, CreateMutexW, OpenEventW, ReleaseMutex, SetEvent, WaitForSingleObject,
    EVENT_MODIFY_STATE,
};

use crate::common::error::AppError;

pub(crate) const SINGLE_INSTANCE_MUTEX: &str = r"Local\Skrivli.SingleInstance";
pub(crate) const ACTIVATE_EVENT_NAME: &str = r"Local\Skrivli.Activate";

#[derive(Debug)]
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

/// Named auto-reset event. The owner creates it; a second process opens and pulses it.
pub(crate) struct ShowSignal {
    handle: HANDLE,
}

impl ShowSignal {
    pub(crate) fn create(name: &str) -> Result<Self, AppError> {
        let wide = wide(name);
        // SAFETY: `wide` is NUL-terminated. The event is auto-reset and starts unsignaled.
        let handle = unsafe { CreateEventW(None, false, false, PCWSTR(wide.as_ptr())) }
            .map_err(|error| AppError::message("activation", error.to_string()))?;
        Ok(Self { handle })
    }

    pub(crate) fn open(name: &str) -> Result<Self, AppError> {
        let wide = wide(name);
        // SAFETY: `wide` is NUL-terminated. EVENT_MODIFY_STATE is enough to pulse.
        let handle = unsafe { OpenEventW(EVENT_MODIFY_STATE, false, PCWSTR(wide.as_ptr())) }
            .map_err(|error| AppError::message("activation", error.to_string()))?;
        Ok(Self { handle })
    }

    pub(crate) fn pulse(&self) -> Result<(), AppError> {
        // SAFETY: the handle came from CreateEventW or OpenEventW.
        unsafe { SetEvent(self.handle) }
            .map_err(|error| AppError::message("activation", error.to_string()))
    }

    pub(crate) fn wait_timeout(&self, timeout: Duration) -> bool {
        let millis = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
        // SAFETY: the handle came from CreateEventW or OpenEventW.
        unsafe { WaitForSingleObject(self.handle, millis) == WAIT_OBJECT_0 }
    }
}

impl Drop for ShowSignal {
    fn drop(&mut self) {
        // SAFETY: this owner is the only closer of this handle value.
        unsafe {
            if let Err(error) = CloseHandle(self.handle) {
                tracing::error!(%error, "CloseHandle failed for the activation event");
            }
        }
    }
}

/// Retry briefly so a loser that starts before the winner creates the event still signals.
pub(crate) fn signal_primary_instance(name: &str) -> Result<(), AppError> {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match ShowSignal::open(name) {
            Ok(signal) => return signal.pulse(),
            Err(error) => {
                if Instant::now() >= deadline {
                    return Err(error);
                }
                thread::sleep(Duration::from_millis(50));
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
    use std::time::Duration;

    use super::{signal_primary_instance, InstanceGuardError, ShowSignal, SingleInstance};

    const SHOW_FOCUS: &str = "show/focus";

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

    #[test]
    fn second_acquire_signals_show_without_a_recordings_path() {
        // Arrange
        let mutex_name = unique_name();
        let event_name = unique_event();
        let owner = SingleInstance::acquire(&mutex_name).expect("owner");
        let signal = ShowSignal::create(&event_name).expect("event");

        // Act
        let second = SingleInstance::acquire(&mutex_name);
        let signaled = matches!(second, Err(InstanceGuardError::AlreadyRunning))
            && signal_primary_instance(&event_name).is_ok();
        let pulsed = signal.wait_timeout(Duration::from_secs(2));

        // Assert
        assert!(signaled);
        assert!(pulsed);
        assert_eq!(SHOW_FOCUS, "show/focus");
        drop(owner);
    }

    #[test]
    fn show_requested_before_the_window_exists_is_applied_when_it_does() {
        // Arrange
        let mut pending = false;
        let mut log = Vec::new();

        // Act
        remember_show(false, &mut pending, &mut log);
        let before = log.clone();
        flush_pending_show(&mut pending, &mut log);

        // Assert
        assert!(before.is_empty());
        assert_eq!(log, vec![SHOW_FOCUS]);
        assert!(!pending);
    }

    fn remember_show(window_ready: bool, pending: &mut bool, log: &mut Vec<&'static str>) {
        if window_ready {
            *pending = false;
            log.push(SHOW_FOCUS);
        } else {
            *pending = true;
        }
    }

    fn flush_pending_show(pending: &mut bool, log: &mut Vec<&'static str>) {
        if *pending {
            *pending = false;
            log.push(SHOW_FOCUS);
        }
    }

    fn unique_event() -> String {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        format!(r"Local\Skrivli.Test.Activate.{}.{}", std::process::id(), id)
    }
}
