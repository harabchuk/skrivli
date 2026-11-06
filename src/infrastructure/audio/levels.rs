//! Atomic meter peaks and the microphone gain the mix thread reads.
//!
//! Peaks are non-negative `f32` bit patterns. Those bits sort in the same
//! order as the values, so `fetch_max` is the hot-path publish. The UI swaps
//! the peak back to zero about 30 times a second. Gain uses the same bit
//! pattern so the slider does not take a lock in the mix loop.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use crate::domain::audio_frame::{AudioFrame, ChannelLayout};
use crate::domain::gain::clamp_slider_gain;
use crate::services::ports::AudioTap;

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
}

impl GainControl {
    pub(crate) fn new(slider: f32) -> Self {
        Self {
            slider: AtomicU32::new(clamp_slider_gain(slider).to_bits()),
        }
    }

    pub(crate) fn slider(&self) -> f32 {
        load_gain(&self.slider)
    }

    pub(crate) fn set_slider(&self, gain: f32) {
        self.slider
            .store(clamp_slider_gain(gain).to_bits(), Ordering::Release);
    }
}

/// Which recording meter a [`PeakTap`] publishes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MeterSlot {
    Mic,
    System,
    Mixed,
}

/// Publishes the peak of the frame's valid samples. Padding past `valid_frames`
/// is ignored, and samples are never logged.
pub(crate) struct PeakTap {
    levels: Arc<LevelSnapshot>,
    slot: MeterSlot,
}

impl PeakTap {
    pub(crate) fn new(levels: Arc<LevelSnapshot>, slot: MeterSlot) -> Self {
        Self { levels, slot }
    }
}

impl AudioTap for PeakTap {
    fn observe(&mut self, frame: &AudioFrame) {
        let peak = peak_of_valid(frame);
        match self.slot {
            MeterSlot::Mic => self.levels.publish(peak, 0.0, 0.0),
            MeterSlot::System => self.levels.publish(0.0, peak, 0.0),
            MeterSlot::Mixed => self.levels.publish(0.0, 0.0, peak),
        }
    }
}

fn peak_of_valid(frame: &AudioFrame) -> f32 {
    let count = match frame.layout() {
        ChannelLayout::Mono => frame.valid_frames(),
        ChannelLayout::Stereo => frame.valid_frames().saturating_mul(2),
    };
    let samples = frame.samples();
    let end = count.min(samples.len());
    let mut loudest = 0.0_f32;
    for sample in &samples[..end] {
        if sample.is_finite() {
            loudest = loudest.max(sample.abs());
        }
    }
    loudest
}

#[cfg(test)]
mod tests {
    use super::{peak_of_valid, GainControl, LevelSnapshot, MeterSlot, PeakTap};
    use crate::domain::audio_frame::{AudioFrame, ChannelLayout, FRAME_LEN};
    use crate::services::ports::AudioTap;
    use std::sync::Arc;

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
        let gain = GainControl::new(1.0);

        // Act
        gain.set_slider(1.5);

        // Assert
        assert!((gain.slider() - 1.5).abs() < 0.0001);
        gain.set_slider(f32::NAN);
        assert!((gain.slider() - 1.0).abs() < 0.0001);
    }

    #[test]
    fn peak_tap_ignores_samples_past_the_valid_length() {
        // Arrange
        let levels = Arc::new(LevelSnapshot::new());
        let mut tap = PeakTap::new(Arc::clone(&levels), MeterSlot::Mic);
        let mut samples = vec![0.0; FRAME_LEN];
        samples[0] = 0.25;
        samples[1] = 0.9;
        let frame = AudioFrame::new(ChannelLayout::Mono, samples, 1, 0).expect("frame");

        // Act
        let measured = peak_of_valid(&frame);
        tap.observe(&frame);
        let published = levels.take_mic();

        // Assert
        assert!((measured - 0.25).abs() < 0.0001);
        assert!((published - 0.25).abs() < 0.0001);
        assert_eq!(levels.take_system(), 0.0);
        assert_eq!(levels.take_mixed(), 0.0);
    }
}
