//! Latest echo-canceller estimates, shared without keeping the step type.
//!
//! The canceller is type-erased inside the mic chain. It writes this slot on
//! each successful capture. The session reads it once at the end. The numbers
//! are sonora's capped internal statistic (about 7.8 dB in D-13), not
//! full-band energy ERLE. Samples are never stored here.

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};

/// `echo_return_loss_enhancement` and `delay_ms` from one statistics read.
///
/// `None` means sonora had not produced that estimate. It is not a zero.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct EchoStats {
    pub(crate) echo_return_loss_enhancement: Option<f64>,
    pub(crate) delay_ms: Option<i32>,
}

/// Slot the factory shares between the canceller and the pipeline.
///
/// `latest` stays `None` until a successful capture publishes. Publishing
/// does not allocate.
pub(crate) struct EchoStatsSlot {
    written: AtomicBool,
    enhancement_present: AtomicBool,
    enhancement_bits: AtomicU64,
    delay_present: AtomicBool,
    delay_bits: AtomicI32,
}

impl EchoStatsSlot {
    pub(crate) fn new() -> Self {
        Self {
            written: AtomicBool::new(false),
            enhancement_present: AtomicBool::new(false),
            enhancement_bits: AtomicU64::new(0),
            delay_present: AtomicBool::new(false),
            delay_bits: AtomicI32::new(0),
        }
    }

    pub(crate) fn publish(&self, stats: EchoStats) {
        match stats.echo_return_loss_enhancement {
            Some(value) => {
                self.enhancement_bits
                    .store(value.to_bits(), Ordering::Relaxed);
                self.enhancement_present.store(true, Ordering::Relaxed);
            }
            None => self.enhancement_present.store(false, Ordering::Relaxed),
        }
        match stats.delay_ms {
            Some(value) => {
                self.delay_bits.store(value, Ordering::Relaxed);
                self.delay_present.store(true, Ordering::Relaxed);
            }
            None => self.delay_present.store(false, Ordering::Relaxed),
        }
        self.written.store(true, Ordering::Release);
    }

    /// The last published estimates, or `None` if capture never succeeded.
    pub(crate) fn latest(&self) -> Option<EchoStats> {
        if !self.written.load(Ordering::Acquire) {
            return None;
        }
        let echo_return_loss_enhancement = self
            .enhancement_present
            .load(Ordering::Relaxed)
            .then(|| f64::from_bits(self.enhancement_bits.load(Ordering::Relaxed)));
        let delay_ms = self
            .delay_present
            .load(Ordering::Relaxed)
            .then(|| self.delay_bits.load(Ordering::Relaxed));
        Some(EchoStats {
            echo_return_loss_enhancement,
            delay_ms,
        })
    }
}
