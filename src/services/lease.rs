//! One open session per endpoint.
//!
//! The idle monitor and the recording workers must not capture the same
//! device together. Recording sets the flag on its own thread and waits until
//! the monitor has dropped its clients.

use std::sync::{Condvar, Mutex};
use std::time::Duration;

struct LeaseState {
    recording: bool,
    monitors: usize,
}

pub(crate) struct EndpointLease {
    state: Mutex<LeaseState>,
    cv: Condvar,
}

impl EndpointLease {
    pub(crate) fn new() -> Self {
        Self {
            state: Mutex::new(LeaseState {
                recording: false,
                monitors: 0,
            }),
            cv: Condvar::new(),
        }
    }

    pub(crate) fn acquire_recording(&self) {
        let mut state = lock(&self.state);
        state.recording = true;
        while state.monitors > 0 {
            state = self
                .cv
                .wait(state)
                .unwrap_or_else(|poison| poison.into_inner());
        }
    }

    pub(crate) fn release_recording(&self) {
        let mut state = lock(&self.state);
        state.recording = false;
        self.cv.notify_all();
    }

    pub(crate) fn recording(&self) -> bool {
        lock(&self.state).recording
    }

    /// `false` when a recording owns the endpoints.
    pub(crate) fn try_monitor(&self) -> bool {
        let mut state = lock(&self.state);
        if state.recording {
            return false;
        }
        state.monitors = state.monitors.saturating_add(1);
        true
    }

    pub(crate) fn end_monitor(&self) {
        let mut state = lock(&self.state);
        state.monitors = state.monitors.saturating_sub(1);
        self.cv.notify_all();
    }

    pub(crate) fn wait_until_idle(&self, stop: &std::sync::atomic::AtomicBool) {
        use std::sync::atomic::Ordering;
        let mut state = lock(&self.state);
        while state.recording && !stop.load(Ordering::Acquire) {
            let (next, _) = self
                .cv
                .wait_timeout(state, Duration::from_millis(50))
                .unwrap_or_else(|poison| poison.into_inner());
            state = next;
        }
    }
}

fn lock(mutex: &Mutex<LeaseState>) -> std::sync::MutexGuard<'_, LeaseState> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}
