//! Rubato asynchronous stereo resampler into 48 kHz interleaved `f32`.
//!
//! The pipeline owns one [`StereoResampler`] per source. Nominal conversion uses
//! `out_rate / in_rate`. Gradual clock drift is corrected by nudging that ratio
//! from the aligner's timeline error (D-06). Sudden holes and overlaps stay in
//! the aligner; this type only stretches or compresses the continuous stream.

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Adjustable, Async, FixedAsync, Indexing, PolynomialDegree, Resampler};

use crate::domain::audio::INTERNAL_CHANNELS;
use crate::services::ports::DriftResampler;

/// Fixed input chunk: about 10 ms at 48 kHz.
const CHUNK_FRAMES: usize = 480;
/// Allow ±1% ratio adjustment for drift feedback.
const MAX_RATIO_RELATIVE: f64 = 1.01;
/// Maps mixer frame error into a relative ratio nudge.
const DRIFT_GAIN: f64 = 1e-5;

pub(crate) struct StereoResampler {
    inner: Async<f32>,
    pending: Vec<f32>,
    out_scratch: Vec<f32>,
    base_ratio: f64,
}

impl StereoResampler {
    /// Build a resampler from device rate to internal rate.
    /// Zero rates are treated as 1 so construction cannot fail on bad hardware
    /// reports; callers still gate unsupported formats earlier.
    pub(crate) fn new(in_rate: u32, out_rate: u32) -> Self {
        let in_rate = in_rate.max(1);
        let out_rate = out_rate.max(1);
        let base_ratio = f64::from(out_rate) / f64::from(in_rate);
        let inner = Async::<f32>::new_poly(
            base_ratio,
            MAX_RATIO_RELATIVE,
            PolynomialDegree::Cubic,
            CHUNK_FRAMES,
            INTERNAL_CHANNELS as usize,
            FixedAsync::Input,
        )
        .expect("positive rates and chunk size are valid for Rubato Async");
        Self {
            inner,
            pending: Vec::new(),
            out_scratch: Vec::new(),
            base_ratio,
        }
    }

    /// Apply mixer timeline error. Positive means the device clock is slow and
    /// needs slightly more output frames per input frame.
    pub(crate) fn set_drift_error(&mut self, frames: i64) {
        let relative =
            (1.0 + DRIFT_GAIN * frames as f64).clamp(1.0 / MAX_RATIO_RELATIVE, MAX_RATIO_RELATIVE);
        let target = self.base_ratio * relative;
        let _ = self.inner.set_resample_ratio(target, true);
    }

    /// Push interleaved stereo frames and return any complete output chunks.
    pub(crate) fn process(&mut self, interleaved: &[f32]) -> Vec<f32> {
        if interleaved.len() >= 2 {
            let frames = interleaved.len() / 2;
            self.pending
                .extend_from_slice(&interleaved[..frames.saturating_mul(2)]);
        }
        self.drain(false)
    }

    /// Consume leftovers and flush Rubato's delay line (end of stream).
    pub(crate) fn flush(&mut self) -> Vec<f32> {
        self.drain(true)
    }

    /// Drop buffered input and the filter delay. The ratio nudge is unchanged:
    /// the next [`set_drift_error`](Self::set_drift_error) applies it again.
    pub(crate) fn reset(&mut self) {
        self.pending.clear();
        self.out_scratch.clear();
        self.inner = Async::<f32>::new_poly(
            self.base_ratio,
            MAX_RATIO_RELATIVE,
            PolynomialDegree::Cubic,
            CHUNK_FRAMES,
            INTERNAL_CHANNELS as usize,
            FixedAsync::Input,
        )
        .expect("positive rates and chunk size are valid for Rubato Async");
    }

    fn drain(&mut self, flush: bool) -> Vec<f32> {
        let channels = INTERNAL_CHANNELS as usize;
        let mut output = Vec::new();
        loop {
            let needed = self.inner.input_frames_next();
            let have = self.pending.len() / channels;
            if have >= needed && needed > 0 {
                let produced = self.process_chunk(needed, None);
                output.extend_from_slice(&produced);
                continue;
            }
            if !flush {
                break;
            }
            if have > 0 {
                let produced = self.process_chunk(needed.max(1), Some(have));
                output.extend_from_slice(&produced);
                continue;
            }
            // Flush filter delay with silent input until a short pass produces
            // nothing useful beyond one delay-sized burst.
            let delay = self.inner.output_delay();
            if delay == 0 {
                break;
            }
            let before = output.len();
            let produced = self.process_chunk(needed.max(1), Some(0));
            output.extend_from_slice(&produced);
            if output.len() == before {
                break;
            }
            // One silent chunk is enough to push out the delay for Cubic poly.
            break;
        }
        output
    }

    fn process_chunk(&mut self, needed: usize, partial: Option<usize>) -> Vec<f32> {
        let channels = INTERNAL_CHANNELS as usize;
        let out_frames = self.inner.output_frames_next().max(1);
        self.out_scratch
            .resize(out_frames.saturating_mul(channels), 0.0);

        let take = partial.unwrap_or(needed).min(self.pending.len() / channels);
        let input_samples = take.saturating_mul(channels);
        // Adapter needs space for `needed` frames even on a partial/silent call.
        let adapter_frames = needed.max(take).max(1);
        let mut input_storage = vec![0.0_f32; adapter_frames.saturating_mul(channels)];
        if input_samples > 0 {
            input_storage[..input_samples].copy_from_slice(&self.pending[..input_samples]);
            self.pending.drain(..input_samples);
        }

        let Ok(input) = InterleavedSlice::new(&input_storage, channels, adapter_frames) else {
            return Vec::new();
        };
        let Ok(mut output) = InterleavedSlice::new_mut(&mut self.out_scratch, channels, out_frames)
        else {
            return Vec::new();
        };

        let indexing = partial.map(|frames| Indexing::new().partial_len(frames));
        let Ok((_consumed, produced)) =
            self.inner
                .process_into_buffer(&input, &mut output, indexing.as_ref())
        else {
            return Vec::new();
        };
        self.out_scratch[..produced.saturating_mul(channels)].to_vec()
    }
}

impl DriftResampler for StereoResampler {
    fn set_drift_error(&mut self, frames: i64) {
        StereoResampler::set_drift_error(self, frames);
    }

    fn process(&mut self, interleaved: &[f32]) -> Vec<f32> {
        StereoResampler::process(self, interleaved)
    }

    fn flush(&mut self) -> Vec<f32> {
        StereoResampler::flush(self)
    }

    fn reset(&mut self) {
        StereoResampler::reset(self);
    }
}

#[cfg(test)]
mod tests {
    use super::{StereoResampler, CHUNK_FRAMES};

    fn tone(frames: usize, left: f32, right: f32) -> Vec<f32> {
        let mut samples = Vec::with_capacity(frames.saturating_mul(2));
        for _ in 0..frames {
            samples.push(left);
            samples.push(right);
        }
        samples
    }

    #[test]
    fn equal_rate_preserves_frame_count_after_flush() {
        // Arrange
        let mut resampler = StereoResampler::new(48_000, 48_000);
        let input = tone(CHUNK_FRAMES * 4, 0.25, -0.25);

        // Act
        let mut output = resampler.process(&input);
        output.extend(resampler.flush());

        // Assert — flush emits one padded chunk for the delay line, so allow
        // about one extra chunk beyond the nominal input length.
        let out_frames = output.len() / 2;
        let expected = CHUNK_FRAMES * 4;
        assert!(
            out_frames >= expected.saturating_sub(16),
            "out_frames={out_frames} expected≥{expected}-16"
        );
        assert!(
            out_frames <= expected + CHUNK_FRAMES + 16,
            "out_frames={out_frames} expected≤{expected}+chunk"
        );
    }

    #[test]
    fn half_rate_roughly_doubles_frames() {
        // Arrange
        let mut resampler = StereoResampler::new(24_000, 48_000);
        let input = tone(CHUNK_FRAMES * 2, 0.1, 0.1);

        // Act
        let mut output = resampler.process(&input);
        output.extend(resampler.flush());

        // Assert
        let out_frames = output.len() / 2;
        let expected = CHUNK_FRAMES * 4;
        assert!(
            out_frames >= expected.saturating_sub(expected / 10),
            "out_frames={out_frames} expected≥{expected}"
        );
        assert!(
            out_frames <= expected + CHUNK_FRAMES * 2 + 16,
            "out_frames={out_frames} expected≤{expected}+2chunks"
        );
    }

    #[test]
    fn positive_drift_error_increases_output_frames() {
        // Arrange
        let input = tone(CHUNK_FRAMES * 8, 0.05, 0.05);
        let mut slow = StereoResampler::new(48_000, 48_000);
        let mut fast = StereoResampler::new(48_000, 48_000);
        slow.set_drift_error(2_000);
        fast.set_drift_error(-2_000);

        // Act
        let mut slow_out = slow.process(&input);
        slow_out.extend(slow.flush());
        let mut fast_out = fast.process(&input);
        fast_out.extend(fast.flush());

        // Assert
        assert!(
            slow_out.len() > fast_out.len(),
            "slow={} fast={}",
            slow_out.len(),
            fast_out.len()
        );
    }

    #[test]
    fn reset_restores_a_fresh_resampler() {
        // Arrange
        let input = tone(CHUNK_FRAMES * 4, 0.2, 0.2);
        let mut drifted = StereoResampler::new(48_000, 48_000);
        drifted.set_drift_error(2_000);
        let _ = drifted.process(&input);

        // Act
        drifted.reset();
        let mut after = drifted.process(&input);
        after.extend(drifted.flush());
        let mut fresh = StereoResampler::new(48_000, 48_000);
        let mut expected = fresh.process(&input);
        expected.extend(fresh.flush());

        // Assert
        assert_eq!(after.len(), expected.len());
    }
}
