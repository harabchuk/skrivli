//! Mix and gain steps that do not know about capture or sonora.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::Arc;

use crate::domain::audio_frame::{AudioFrame, ChannelLayout};
use crate::domain::gain::{soft_limit, LIMIT_KNEE};
use crate::infrastructure::audio::levels::GainControl;
use crate::services::ports::{AudioMixer, AudioProcessor, AudioTap, ProcessContext, ProcessError};

#[derive(Clone, Copy, Debug)]
pub(crate) struct FixedGain {
    gain: f32,
}

impl FixedGain {
    pub(crate) fn new(gain: f32) -> Result<Self, ProcessError> {
        if !gain.is_finite() {
            return Err(ProcessError::new("fixed-gain", "gain is not finite"));
        }
        Ok(Self { gain })
    }
}

impl AudioProcessor for FixedGain {
    fn name(&self) -> &'static str {
        "fixed-gain"
    }

    fn process(
        &mut self,
        frame: &mut AudioFrame,
        _context: &ProcessContext<'_>,
    ) -> Result<(), ProcessError> {
        for sample in frame.samples_mut() {
            *sample *= self.gain;
        }
        Ok(())
    }
}

/// Exact multiply of [`GainControl::slider`] for the recording harness and
/// exact-sample tests. Production uses `SonoraMicGain`.
///
/// This stays in the library, not only `cfg(test)`. `tests/` links
/// `RecordingHarness`, and that build of the library does not set `cfg(test)`.
/// It does not cancel echo.
pub(crate) struct LinearGain {
    gain: Arc<GainControl>,
}

impl LinearGain {
    pub(crate) fn new(gain: Arc<GainControl>) -> Self {
        Self { gain }
    }
}

impl AudioProcessor for LinearGain {
    fn name(&self) -> &'static str {
        "linear-gain"
    }

    fn process(
        &mut self,
        frame: &mut AudioFrame,
        _context: &ProcessContext<'_>,
    ) -> Result<(), ProcessError> {
        let slider = self.gain.slider();
        for sample in frame.samples_mut() {
            *sample *= slider;
        }
        Ok(())
    }
}

#[derive(Debug, Default)]
pub(crate) struct SoftLimiter {
    limited_samples: Option<Arc<AtomicU64>>,
}

impl SoftLimiter {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn new() -> Self {
        Self {
            limited_samples: None,
        }
    }

    /// Counts valid samples whose absolute value is above the limiter knee.
    pub(crate) fn counting(limited_samples: Arc<AtomicU64>) -> Self {
        Self {
            limited_samples: Some(limited_samples),
        }
    }
}

impl AudioProcessor for SoftLimiter {
    fn name(&self) -> &'static str {
        "soft-limiter"
    }

    fn process(
        &mut self,
        frame: &mut AudioFrame,
        _context: &ProcessContext<'_>,
    ) -> Result<(), ProcessError> {
        let counted = match frame.layout() {
            ChannelLayout::Mono => frame.valid_frames(),
            ChannelLayout::Stereo => frame.valid_frames().saturating_mul(2),
        };
        let samples = frame.samples_mut();
        let end = counted.min(samples.len());
        for (index, sample) in samples.iter_mut().enumerate() {
            if index < end {
                if let Some(counter) = &self.limited_samples {
                    if sample.is_finite() && sample.abs() > LIMIT_KNEE {
                        counter.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
            *sample = soft_limit(*sample);
        }
        Ok(())
    }
}

/// Adds the microphone and system frames into a stereo `output`.
///
/// A mono frame is copied onto both output channels. A stereo frame is added
/// channel by channel. A mono `output` is left unchanged, because it cannot
/// hold the stereo sum.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SumMixer;

impl AudioMixer for SumMixer {
    fn mix(&mut self, mic: &AudioFrame, system: &AudioFrame, output: &mut AudioFrame) {
        if output.layout() != ChannelLayout::Stereo {
            return;
        }
        let (pairs, _) = output.samples_mut().as_chunks_mut::<2>();
        for (index, pair) in pairs.iter_mut().enumerate() {
            pair[0] = channel_sample(mic, index, 0) + channel_sample(system, index, 0);
            pair[1] = channel_sample(mic, index, 1) + channel_sample(system, index, 1);
        }
    }
}

fn channel_sample(frame: &AudioFrame, index: usize, channel: usize) -> f32 {
    let samples = frame.samples();
    match frame.layout() {
        ChannelLayout::Mono => samples.get(index).copied().unwrap_or(0.0),
        ChannelLayout::Stereo => samples.get(index * 2 + channel).copied().unwrap_or(0.0),
    }
}

/// Latest-frame levels published by [`StatsTap`].
///
/// A clipped sample is a finite sample whose absolute value is at least 1.0.
/// That is full scale, not the limiter knee. Non-finite samples are ignored.
/// Samples past `valid_frames` are ignored. Each `observe` replaces these
/// three numbers; it does not accumulate them across frames. The tap does not
/// log samples.
pub(crate) struct AudioLevelStats {
    rms_bits: AtomicU64,
    peak_bits: AtomicU32,
    clipped: AtomicU64,
}

impl AudioLevelStats {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn new() -> Self {
        Self {
            rms_bits: AtomicU64::new(0),
            peak_bits: AtomicU32::new(0),
            clipped: AtomicU64::new(0),
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn rms(&self) -> f64 {
        f64::from_bits(self.rms_bits.load(Ordering::Acquire))
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn peak(&self) -> f32 {
        f32::from_bits(self.peak_bits.load(Ordering::Acquire))
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn clipped_samples(&self) -> u64 {
        self.clipped.load(Ordering::Acquire)
    }

    fn publish(&self, rms: f64, peak: f32, clipped: u64) {
        self.rms_bits.store(rms.to_bits(), Ordering::Relaxed);
        self.peak_bits.store(peak.to_bits(), Ordering::Relaxed);
        self.clipped.store(clipped, Ordering::Release);
    }
}

/// RMS, peak, and clipped-sample count of the frame at this point in the chain.
///
/// `observe` only stores atomics. It does not allocate or block, and it does
/// not log samples. Not installed by the production factory.
pub(crate) struct StatsTap {
    stats: Arc<AudioLevelStats>,
}

impl StatsTap {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn new(stats: Arc<AudioLevelStats>) -> Self {
        Self { stats }
    }
}

impl AudioTap for StatsTap {
    fn observe(&mut self, frame: &AudioFrame) {
        let count = match frame.layout() {
            ChannelLayout::Mono => frame.valid_frames(),
            ChannelLayout::Stereo => frame.valid_frames().saturating_mul(2),
        };
        let samples = frame.samples();
        let end = count.min(samples.len());
        let mut squares = 0.0_f64;
        let mut finite = 0_u64;
        let mut peak = 0.0_f32;
        let mut clipped = 0_u64;
        for sample in &samples[..end] {
            if !sample.is_finite() {
                continue;
            }
            let magnitude = sample.abs();
            peak = peak.max(magnitude);
            if magnitude >= 1.0 {
                clipped = clipped.saturating_add(1);
            }
            let value = f64::from(*sample);
            squares += value * value;
            finite = finite.saturating_add(1);
        }
        let rms = if finite == 0 {
            0.0
        } else {
            (squares / finite as f64).sqrt()
        };
        self.stats.publish(rms, peak, clipped);
    }
}

/// Sends a copy of the frame to another thread without waiting.
///
/// The queue is a bounded `sync_channel`. `observe` calls `try_send`. A full
/// queue, or a receiver that is already gone, drops that frame and increments
/// [`Self::dropped`]. The audio thread never calls `send`. Not installed by
/// the production factory.
pub(crate) struct ChannelTap {
    tx: SyncSender<AudioFrame>,
    dropped: AtomicU64,
}

impl ChannelTap {
    /// `capacity` below 1 is raised to 1. `sync_channel(0)` would panic.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn open(capacity: usize) -> (Self, Receiver<AudioFrame>) {
        let (tx, rx) = sync_channel(capacity.max(1));
        (
            Self {
                tx,
                dropped: AtomicU64::new(0),
            },
            rx,
        )
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

impl AudioTap for ChannelTap {
    fn observe(&mut self, frame: &AudioFrame) {
        if self.tx.try_send(frame.clone()).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use super::{AudioLevelStats, ChannelTap, StatsTap};
    use crate::domain::audio_frame::{AudioFrame, ChannelLayout, FRAME_LEN};
    use crate::domain::gain::{soft_limit, LIMIT_KNEE, SYSTEM_GAIN};
    use crate::infrastructure::audio::levels::GainControl;
    use crate::services::audio_pipeline::{Chain, FixedGain, SoftLimiter, SumMixer};
    use crate::services::ports::{AudioMixer, AudioProcessor, AudioTap, ProcessContext};

    #[test]
    fn linear_gain_reads_the_slider_once_per_frame() {
        // Arrange
        let gain = Arc::new(GainControl::new(1.0));
        let mut step = super::LinearGain::new(Arc::clone(&gain));
        let mut frame = frame(ChannelLayout::Mono, vec![0.25; FRAME_LEN]);
        gain.set_slider(2.0);

        // Act
        step.process(&mut frame, &context()).expect("process");
        gain.set_slider(4.0);

        // Assert
        assert!(frame.samples().iter().all(|sample| *sample == 0.5));
    }

    fn context() -> ProcessContext<'static> {
        ProcessContext {
            echo_reference: None,
        }
    }

    fn frame(layout: ChannelLayout, samples: Vec<f32>) -> AudioFrame {
        AudioFrame::new(layout, samples, FRAME_LEN, 0).expect("frame")
    }

    #[test]
    fn fixed_gain_rejects_a_non_finite_gain() {
        // Arrange
        let nan = f32::NAN;
        let infinite = f32::INFINITY;

        // Act
        let nan_gain = FixedGain::new(nan);
        let infinite_gain = FixedGain::new(infinite);
        let negative_infinite = FixedGain::new(f32::NEG_INFINITY);
        let zero = FixedGain::new(0.0);

        // Assert
        assert!(nan_gain.is_err());
        assert!(infinite_gain.is_err());
        assert!(negative_infinite.is_err());
        assert!(zero.is_ok());
    }

    #[test]
    fn fixed_gain_scales_every_sample() {
        // Arrange
        let mut gain = FixedGain::new(0.5).expect("gain");
        let mut frame = frame(ChannelLayout::Stereo, vec![0.5; FRAME_LEN * 2]);

        // Act
        gain.process(&mut frame, &context()).expect("process");

        // Assert
        assert!(frame.samples().iter().all(|sample| *sample == 0.25));
    }

    #[test]
    fn soft_limiter_matches_soft_limit_on_a_hot_sample() {
        // Arrange
        let hot = 1.0 + SYSTEM_GAIN;
        let mut samples = vec![0.25; FRAME_LEN];
        samples[0] = hot;
        samples[1] = -hot;
        let mut frame = frame(ChannelLayout::Mono, samples);
        let mut limiter = SoftLimiter::new();

        // Act
        limiter.process(&mut frame, &context()).expect("limit");

        // Assert
        assert_eq!(frame.samples()[0], soft_limit(hot));
        assert_eq!(frame.samples()[1], soft_limit(-hot));
        assert_eq!(frame.samples()[2], soft_limit(0.25));
        assert!(soft_limit(hot) < 1.0);
        assert!(soft_limit(hot) > LIMIT_KNEE);
        let counter = Arc::new(AtomicU64::new(0));
        let mut counting = SoftLimiter::counting(Arc::clone(&counter));
        let mut samples = vec![0.25; FRAME_LEN];
        samples[0] = hot;
        samples[2] = hot;
        let mut counted =
            AudioFrame::new(ChannelLayout::Mono, samples, 2, 0).expect("short valid length");
        counting.process(&mut counted, &context()).expect("limit");
        assert_eq!(counter.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn sum_mixer_duplicates_a_mono_mic_onto_both_channels() {
        // Arrange
        let mut mic_samples = vec![0.0; FRAME_LEN];
        mic_samples[0] = 0.25;
        let mic = frame(ChannelLayout::Mono, mic_samples);
        let mut system_samples = vec![0.0; FRAME_LEN * 2];
        system_samples[0] = 0.125;
        system_samples[1] = 0.5;
        let system = frame(ChannelLayout::Stereo, system_samples);
        let mut output = AudioFrame::silence(ChannelLayout::Stereo, 7);
        let mut mixer = SumMixer;

        // Act
        mixer.mix(&mic, &system, &mut output);

        // Assert
        assert_eq!(output.samples()[0], 0.375);
        assert_eq!(output.samples()[1], 0.75);
        assert_eq!(output.samples()[2], 0.0);
        assert_eq!(output.layout(), ChannelLayout::Stereo);
        assert_eq!(output.timeline_frame(), 7);
    }

    #[test]
    fn sum_mixer_handles_unexpected_layouts_without_panicking() {
        // Arrange
        let mono_mic = frame(ChannelLayout::Mono, vec![0.25; FRAME_LEN]);
        let stereo_mic = frame(ChannelLayout::Stereo, {
            let mut samples = vec![0.0; FRAME_LEN * 2];
            samples[0] = 0.25;
            samples[1] = 0.5;
            samples
        });
        let stereo_system = frame(ChannelLayout::Stereo, {
            let mut samples = vec![0.0; FRAME_LEN * 2];
            samples[0] = 0.125;
            samples[1] = 0.25;
            samples
        });
        let mono_system = frame(ChannelLayout::Mono, {
            let mut samples = vec![0.0; FRAME_LEN];
            samples[0] = 0.125;
            samples
        });
        let mut mono_output = frame(ChannelLayout::Mono, vec![0.5; FRAME_LEN]);
        let mut stereo_output = AudioFrame::silence(ChannelLayout::Stereo, 0);
        let mut duplicated_output = AudioFrame::silence(ChannelLayout::Stereo, 0);
        let mut mixer = SumMixer;

        // Act
        mixer.mix(&mono_mic, &stereo_system, &mut mono_output);
        mixer.mix(&stereo_mic, &stereo_system, &mut stereo_output);
        mixer.mix(&mono_mic, &mono_system, &mut duplicated_output);

        // Assert
        // A mono output cannot store the stereo sum, so it is left unchanged.
        assert_eq!(mono_output.samples(), vec![0.5; FRAME_LEN]);
        assert_eq!(mono_output.layout(), ChannelLayout::Mono);
        // A stereo mic is added per channel instead of being duplicated.
        assert_eq!(stereo_output.samples()[0], 0.375);
        assert_eq!(stereo_output.samples()[1], 0.75);
        // A mono system is duplicated onto both channels, then added.
        assert_eq!(duplicated_output.samples()[0], 0.375);
        assert_eq!(duplicated_output.samples()[1], 0.375);
    }

    #[test]
    fn stats_tap_reports_rms_peak_and_clipped_samples() {
        // Arrange
        // Valid region: 0, 0.5, 1, -1.5, and one NaN. The 9.0 sits past
        // valid_frames and must not move the stats. Clipped means |sample| >= 1.
        let mut samples = vec![0.0; FRAME_LEN];
        samples[0] = 0.0;
        samples[1] = 0.5;
        samples[2] = 1.0;
        samples[3] = -1.5;
        samples[4] = f32::NAN;
        samples[5] = 9.0;
        let frame = AudioFrame::new(ChannelLayout::Mono, samples, 5, 0).expect("frame");
        let stats = Arc::new(AudioLevelStats::new());
        let mut tap = StatsTap::new(Arc::clone(&stats));
        let expected_rms = (3.5_f64 / 4.0).sqrt();

        // Act
        tap.observe(&frame);

        // Assert
        assert!((stats.rms() - expected_rms).abs() < 1e-9);
        assert!((stats.peak() - 1.5).abs() < 1e-6);
        assert_eq!(stats.clipped_samples(), 2);
    }

    #[test]
    fn slow_channel_tap_drops_without_blocking() {
        // Arrange
        let (mut tap, rx) = ChannelTap::open(1);
        let first = frame(ChannelLayout::Mono, vec![0.25; FRAME_LEN]);
        let second = frame(ChannelLayout::Mono, vec![0.5; FRAME_LEN]);

        // Act
        let started = Instant::now();
        tap.observe(&first);
        tap.observe(&second);
        let elapsed = started.elapsed();

        // Assert
        // Nothing was taken between the two observes, so the second try_send fails.
        assert_eq!(tap.dropped(), 1);
        assert!(elapsed < Duration::from_millis(250));
        let queued = rx.try_recv().expect("first frame");
        assert_eq!(queued.samples()[0], 0.25);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn stats_and_channel_taps_observe_the_frame_at_their_position() {
        // Arrange
        let stats = Arc::new(AudioLevelStats::new());
        let (channel, rx) = ChannelTap::open(1);
        let mut chain = Chain::new()
            .process(FixedGain::new(2.0).expect("gain"))
            .tap(StatsTap::new(Arc::clone(&stats)))
            .tap(channel)
            .process(FixedGain::new(0.5).expect("gain"));
        let mut frame = frame(ChannelLayout::Mono, vec![0.25; FRAME_LEN]);

        // Act
        chain.run(
            &mut frame,
            &ProcessContext {
                echo_reference: None,
            },
        );

        // Assert
        let seen = rx.try_recv().expect("tapped frame");
        assert!(seen.samples().iter().all(|sample| *sample == 0.5));
        assert!((stats.rms() - 0.5).abs() < 1e-6);
        assert!((stats.peak() - 0.5).abs() < 1e-6);
        assert_eq!(stats.clipped_samples(), 0);
        assert!(frame.samples().iter().all(|sample| *sample == 0.25));
    }
}
