//! Atomic meter peaks and the microphone gain the mix thread reads.
//!
//! Peaks are non-negative `f32` bit patterns. Those bits sort in the same
//! order as the values, so `fetch_max` is the hot-path publish. The UI swaps
//! the peak back to zero about 30 times a second. Gain uses the same bit
//! pattern so the slider does not take a lock in the mix loop.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::domain::gain::{clamp_slider_gain, MIC_GAIN_DEFAULT};

fn publish_peak(slot: &AtomicU32, peak: f32) {
    if peak <= 0.0 || !peak.is_finite() {
        return;
    }
    let bits = peak.to_bits();
    let _ = slot.fetch_max(bits, Ordering::Relaxed);
}

fn take_peak(slot: &AtomicU32) -> f32 {
    let bits = slot.swap(0, Ordering::AcqRel);
    let value = f32::from_bits(bits);
    if value.is_finite() && value > 0.0 {
        value
    } else {
        0.0
    }
}

fn load_gain(slot: &AtomicU32) -> f32 {
    clamp_slider_gain(f32::from_bits(slot.load(Ordering::Acquire)))
}

pub(crate) struct LevelSnapshot {
    mic: AtomicU32,
    system: AtomicU32,
    mixed: AtomicU32,
}

impl LevelSnapshot {
    pub(crate) fn new() -> Self {
        Self {
            mic: AtomicU32::new(0),
            system: AtomicU32::new(0),
            mixed: AtomicU32::new(0),
        }
    }

    pub(crate) fn publish(&self, mic: f32, system: f32, mixed: f32) {
        publish_peak(&self.mic, mic);
        publish_peak(&self.system, system);
        publish_peak(&self.mixed, mixed);
    }

    pub(crate) fn take_mic(&self) -> f32 {
        take_peak(&self.mic)
    }

    pub(crate) fn take_system(&self) -> f32 {
        take_peak(&self.system)
    }

    pub(crate) fn take_mixed(&self) -> f32 {
        take_peak(&self.mixed)
    }
}

pub(crate) struct GainControl {
    slider: AtomicU32,
    agc: AtomicBool,
    effective: AtomicU32,
}

impl GainControl {
    pub(crate) fn new(slider: f32, agc: bool) -> Self {
        let bits = clamp_slider_gain(slider).to_bits();
        Self {
            slider: AtomicU32::new(bits),
            agc: AtomicBool::new(agc),
            effective: AtomicU32::new(bits),
        }
    }

    pub(crate) fn slider(&self) -> f32 {
        load_gain(&self.slider)
    }

    pub(crate) fn set_slider(&self, gain: f32) {
        self.slider
            .store(clamp_slider_gain(gain).to_bits(), Ordering::Release);
    }

    pub(crate) fn agc_enabled(&self) -> bool {
        self.agc.load(Ordering::Acquire)
    }

    pub(crate) fn set_agc_enabled(&self, enabled: bool) {
        self.agc.store(enabled, Ordering::Release);
    }

    pub(crate) fn set_effective(&self, gain: f32) {
        let stored = if gain.is_finite() {
            gain.clamp(0.0, 4.0)
        } else {
            MIC_GAIN_DEFAULT
        };
        self.effective.store(stored.to_bits(), Ordering::Release);
    }

    pub(crate) fn effective(&self) -> f32 {
        let value = f32::from_bits(self.effective.load(Ordering::Acquire));
        if value.is_finite() {
            value.clamp(0.0, 4.0)
        } else {
            MIC_GAIN_DEFAULT
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{GainControl, LevelSnapshot};

    #[test]
    fn peaks_keep_the_louder_sample_until_the_ui_takes_them() {
        // Arrange
        let levels = LevelSnapshot::new();

        // Act
        levels.publish(0.1, 0.2, 0.3);
        levels.publish(0.4, 0.05, 0.2);
        let mic = levels.take_mic();
        let system = levels.take_system();
        let mixed = levels.take_mixed();
        let cleared = levels.take_mic();

        // Assert
        assert!((mic - 0.4).abs() < 0.0001);
        assert!((system - 0.2).abs() < 0.0001);
        assert!((mixed - 0.3).abs() < 0.0001);
        assert_eq!(cleared, 0.0);
    }

    #[test]
    fn slider_gain_is_visible_without_a_lock() {
        // Arrange
        let gain = GainControl::new(1.0, false);

        // Act
        gain.set_slider(1.5);
        gain.set_agc_enabled(true);
        gain.set_effective(2.0);

        // Assert
        assert!((gain.slider() - 1.5).abs() < 0.0001);
        assert!(gain.agc_enabled());
        assert!((gain.effective() - 2.0).abs() < 0.0001);
        gain.set_slider(f32::NAN);
        assert!((gain.slider() - 1.0).abs() < 0.0001);
    }
}
