//! Idempotent process shutdown. `request` wakes workers, `acknowledge` counts
//! clean exits, and `join` waits for tracked threads. A second request or join
//! does not panic.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

pub(crate) struct ShutdownController {
    inner: Arc<Inner>,
}

struct Inner {
    requested: Arc<AtomicBool>,
    acks: AtomicUsize,
    joins: Mutex<Vec<JoinHandle<()>>>,
    hooks: Mutex<Vec<Box<dyn FnOnce() + Send>>>,
}

impl ShutdownController {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                requested: Arc::new(AtomicBool::new(false)),
                acks: AtomicUsize::new(0),
                joins: Mutex::new(Vec::new()),
                hooks: Mutex::new(Vec::new()),
            }),
        }
    }

    pub(crate) fn flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.inner.requested)
    }

    /// Returns whether this call was the first request.
    pub(crate) fn request(&self) -> bool {
        let taken = {
            let mut hooks = lock(&self.inner.hooks);
            if self.inner.requested.swap(true, Ordering::AcqRel) {
                return false;
            }
            std::mem::take(&mut *hooks)
        };
        for hook in taken {
            hook();
        }
        true
    }

    pub(crate) fn is_requested(&self) -> bool {
        self.inner.requested.load(Ordering::Acquire)
    }

    pub(crate) fn on_stop(&self, hook: impl FnOnce() + Send + 'static) {
        let mut hooks = lock(&self.inner.hooks);
        if self.inner.requested.load(Ordering::Acquire) {
            drop(hooks);
            hook();
            return;
        }
        hooks.push(Box::new(hook));
    }

    pub(crate) fn acknowledge(&self) {
        self.inner.acks.fetch_add(1, Ordering::AcqRel);
    }

    pub(crate) fn acknowledgements(&self) -> usize {
        self.inner.acks.load(Ordering::Acquire)
    }

    pub(crate) fn track(&self, handle: JoinHandle<()>) {
        lock(&self.inner.joins).push(handle);
    }

    pub(crate) fn join(&self) {
        let _ = self.request();
        let handles = std::mem::take(&mut *lock(&self.inner.joins));
        for handle in handles {
            if handle.join().is_err() {
                tracing::error!("a worker stopped after a panic");
            }
        }
        tracing::debug!(acks = self.acknowledgements(), "workers joined");
    }
}

impl Clone for ShutdownController {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;
    use std::thread;
    use std::time::Duration;

    use super::ShutdownController;

    #[test]
    fn request_is_idempotent_and_join_can_repeat() {
        // Arrange
        let shutdown = ShutdownController::new();

        // Act
        let first = shutdown.request();
        let second = shutdown.request();
        shutdown.acknowledge();
        shutdown.join();
        shutdown.join();

        // Assert
        assert!(first);
        assert!(!second);
        assert!(shutdown.is_requested());
        assert_eq!(shutdown.acknowledgements(), 1);
    }

    #[test]
    fn join_waits_for_a_worker_that_acknowledges() {
        // Arrange
        let shutdown = ShutdownController::new();
        let worker = shutdown.clone();
        let handle = thread::spawn(move || {
            while !worker.is_requested() {
                thread::sleep(Duration::from_millis(10));
            }
            worker.acknowledge();
        });
        shutdown.track(handle);

        // Act
        assert!(shutdown.request());
        shutdown.join();

        // Assert
        assert_eq!(shutdown.acknowledgements(), 1);
        assert!(shutdown.flag().load(Ordering::Acquire));
        shutdown.join();
    }
}
