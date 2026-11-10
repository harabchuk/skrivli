//! Convert finalized PCM chunks to speech engine input: 16 kHz mono `f32`.
//!
//! Explicit channel downmix plus Rubato FFT resampling with a Blackman–Harris
//! anti-alias window. Leave [`crate::infrastructure::audio::resampler`] alone —
//! that path is the capture-stream Rubato async drift nudge (D-06).
//!
//! Never logs sample values.

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler};

use crate::common::error::AppError;

/// Sample rate the speech engine expects.
pub(crate) const SPEECH_SAMPLE_RATE: u32 = 16_000;
/// FFT chunk size on the fixed input side.
const FFT_CHUNK_FRAMES: usize = 1_024;

/// Downmix interleaved PCM16 to mono `f32` in \[-1, 1\], then resample to 16 kHz.
pub(crate) fn pcm16_to_16k_mono(
    interleaved: &[i16],
    sample_rate: u32,
    channels: u16,
) -> Result<Vec<f32>, AppError> {
    let mono = downmix_i16_to_mono_f32(interleaved, channels)?;
    resample_mono_f32_to_16k(&mono, sample_rate)
}

/// Average channels into mono `f32`. `interleaved.len()` must be a multiple of `channels`.
pub(crate) fn downmix_i16_to_mono_f32(
    interleaved: &[i16],
    channels: u16,
) -> Result<Vec<f32>, AppError> {
    let channels = usize::from(channels);
    if channels == 0 {
        return Err(AppError::message(
            "transcription",
            "cannot downmix zero channels",
        ));
    }
    if !interleaved.len().is_multiple_of(channels) {
        return Err(AppError::message(
            "transcription",
            "interleaved pcm length is not a multiple of channel count",
        ));
    }
    let frames = interleaved.len() / channels;
    let mut mono = Vec::with_capacity(frames);
    if channels == 1 {
        for &sample in interleaved {
            mono.push(i16_to_f32(sample));
        }
        return Ok(mono);
    }
    for frame in interleaved.chunks_exact(channels) {
        let sum: f32 = frame.iter().map(|&sample| i16_to_f32(sample)).sum();
        mono.push(sum / channels as f32);
    }
    Ok(mono)
}

/// Anti-alias resample mono `f32` from `sample_rate` to [`SPEECH_SAMPLE_RATE`].
pub(crate) fn resample_mono_f32_to_16k(
    mono: &[f32],
    sample_rate: u32,
) -> Result<Vec<f32>, AppError> {
    if sample_rate == 0 {
        return Err(AppError::message("transcription", "sample rate is zero"));
    }
    if mono.is_empty() {
        return Ok(Vec::new());
    }
    if sample_rate == SPEECH_SAMPLE_RATE {
        return Ok(mono.to_vec());
    }

    let mut resampler = Fft::<f32>::new(
        sample_rate as usize,
        SPEECH_SAMPLE_RATE as usize,
        FFT_CHUNK_FRAMES,
        1,
        FixedSync::Input,
    )
    .map_err(|error| AppError::message("transcription", error.to_string()))?;

    let input = InterleavedSlice::new(mono, 1, mono.len()).map_err(|error| {
        AppError::message("transcription", format!("resample input adapter: {error}"))
    })?;
    let output = resampler
        .process_all(&input, mono.len(), None)
        .map_err(|error| AppError::message("transcription", error.to_string()))?;
    Ok(output.take_data())
}

fn i16_to_f32(sample: i16) -> f32 {
    f32::from(sample) / 32768.0
}

#[cfg(test)]
mod tests {
    use super::{
        downmix_i16_to_mono_f32, pcm16_to_16k_mono, resample_mono_f32_to_16k, SPEECH_SAMPLE_RATE,
    };
    use std::f32::consts::PI;

    fn sine_i16(frames: usize, sample_rate: u32, freq_hz: f32, amplitude: f32) -> Vec<i16> {
        let mut out = Vec::with_capacity(frames);
        for index in 0..frames {
            let t = index as f32 / sample_rate as f32;
            let sample = (amplitude * (2.0 * PI * freq_hz * t).sin()).clamp(-1.0, 1.0);
            out.push((sample * 32767.0).round() as i16);
        }
        out
    }

    fn stereo_sine(frames: usize, sample_rate: u32, freq_hz: f32, amplitude: f32) -> Vec<i16> {
        let mono = sine_i16(frames, sample_rate, freq_hz, amplitude);
        let mut stereo = Vec::with_capacity(frames * 2);
        for sample in mono {
            stereo.push(sample);
            stereo.push(sample);
        }
        stereo
    }

    fn rms(samples: &[f32]) -> f32 {
        if samples.is_empty() {
            return 0.0;
        }
        let sum: f32 = samples.iter().map(|sample| sample * sample).sum();
        (sum / samples.len() as f32).sqrt()
    }

    fn estimate_peak_freq_hz(samples: &[f32], sample_rate: u32) -> f32 {
        // Zero-crossing estimate on a steady tone (skip edges).
        let start = samples.len() / 10;
        let end = samples
            .len()
            .saturating_sub(samples.len() / 10)
            .max(start + 2);
        let slice = &samples[start..end];
        let mut crossings = 0_usize;
        for pair in slice.windows(2) {
            if pair[0] <= 0.0 && pair[1] > 0.0 {
                crossings += 1;
            }
        }
        let duration_s = slice.len() as f32 / sample_rate as f32;
        if duration_s <= 0.0 {
            return 0.0;
        }
        crossings as f32 / duration_s
    }

    #[test]
    fn duration_scales_with_rate_ratio() {
        // Arrange — one second of 48 kHz mono.
        let input = sine_i16(48_000, 48_000, 1_000.0, 0.5);

        // Act
        let output = pcm16_to_16k_mono(&input, 48_000, 1).expect("resample");

        // Assert — about one second at 16 kHz (±2% for filter delay trim).
        let expected = SPEECH_SAMPLE_RATE as usize;
        let lo = expected * 98 / 100;
        let hi = expected * 102 / 100;
        assert!(
            output.len() >= lo && output.len() <= hi,
            "len={} expected≈{expected}",
            output.len()
        );
    }

    #[test]
    fn one_khz_sine_keeps_pitch_and_amplitude() {
        // Arrange
        let input = stereo_sine(48_000, 48_000, 1_000.0, 0.5);

        // Act
        let output = pcm16_to_16k_mono(&input, 48_000, 2).expect("resample");
        let freq = estimate_peak_freq_hz(&output, SPEECH_SAMPLE_RATE);
        let level = rms(&output);

        // Assert
        assert!((freq - 1_000.0).abs() < 40.0, "freq={freq} expected≈1000");
        assert!(
            (level - 0.5 / std::f32::consts::SQRT_2).abs() < 0.08,
            "rms={level}"
        );
    }

    #[test]
    fn energy_above_nyquist_is_attenuated() {
        // Arrange — 12 kHz tone at 48 kHz (above 8 kHz Nyquist for 16 kHz).
        let high = sine_i16(48_000, 48_000, 12_000.0, 0.8);
        let low = sine_i16(48_000, 48_000, 1_000.0, 0.8);

        // Act
        let high_out =
            resample_mono_f32_to_16k(&downmix_i16_to_mono_f32(&high, 1).expect("downmix"), 48_000)
                .expect("high");
        let low_out =
            resample_mono_f32_to_16k(&downmix_i16_to_mono_f32(&low, 1).expect("downmix"), 48_000)
                .expect("low");

        // Assert — anti-alias filter must crush energy above 8 kHz.
        let high_rms = rms(&high_out);
        let low_rms = rms(&low_out);
        assert!(
            high_rms < low_rms * 0.15,
            "high_rms={high_rms} low_rms={low_rms}"
        );
    }

    #[test]
    fn downmix_averages_channels() {
        // Arrange
        let interleaved = [10_000_i16, -10_000_i16, 20_000, 0];

        // Act
        let mono = downmix_i16_to_mono_f32(&interleaved, 2).expect("downmix");

        // Assert
        assert_eq!(mono.len(), 2);
        assert!(mono[0].abs() < 1e-5);
        assert!((mono[1] - (20_000.0 / 32768.0) / 2.0).abs() < 1e-5);
    }
}
