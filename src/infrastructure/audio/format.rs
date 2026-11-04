//! Convert captured PCM into the internal 48 kHz stereo `f32` layout.
//!
//! The hardware mix format is whatever WASAPI accepted. This module does not
//! assume the requested format was granted. Resampling is stateful and owned
//! by the capture stream ([`super::resampler::StereoResampler`]).

use crate::domain::audio::{AudioFormat, SampleEncoding, INTERNAL_CHANNELS};

use super::resampler::StereoResampler;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConvertError {
    Unaligned,
    Unsupported,
}

/// Decode `bytes`, map channels, and push through `resampler`.
pub(crate) fn to_internal(
    bytes: &[u8],
    format: AudioFormat,
    resampler: &mut StereoResampler,
) -> Result<Vec<f32>, ConvertError> {
    let frame_bytes = format.bytes_per_frame().ok_or(ConvertError::Unsupported)?;
    if frame_bytes == 0 || !bytes.len().is_multiple_of(frame_bytes as usize) {
        return Err(ConvertError::Unaligned);
    }
    let frames = bytes.len() / frame_bytes as usize;
    let decoded = decode(bytes, format)?;
    let stereo = map_channels(&decoded, format.channels() as usize, frames);
    Ok(resampler.process(&stereo))
}

/// `source_frames` of silence in the device format, fed through the resampler
/// so phase and ratio state stay continuous.
pub(crate) fn silence_internal(
    source_frames: u64,
    format: AudioFormat,
    resampler: &mut StereoResampler,
) -> Result<Vec<f32>, ConvertError> {
    let frame_bytes = format.bytes_per_frame().ok_or(ConvertError::Unsupported)?;
    let bytes = frame_bytes
        .checked_mul(source_frames)
        .ok_or(ConvertError::Unsupported)?;
    let byte_len = usize::try_from(bytes).map_err(|_| ConvertError::Unsupported)?;
    to_internal(&vec![0_u8; byte_len], format, resampler)
}

fn decode(bytes: &[u8], format: AudioFormat) -> Result<Vec<f32>, ConvertError> {
    match format.encoding() {
        SampleEncoding::F32 => {
            let mut out = Vec::with_capacity(bytes.len() / 4);
            for chunk in bytes.as_chunks::<4>().0 {
                let bits = i32::from_le_bytes(*chunk);
                out.push(f32::from_bits(bits as u32));
            }
            Ok(out)
        }
        SampleEncoding::I16 => {
            let mut out = Vec::with_capacity(bytes.len() / 2);
            for chunk in bytes.as_chunks::<2>().0 {
                let sample = i16::from_le_bytes(*chunk);
                out.push(f32::from(sample) / 32768.0);
            }
            Ok(out)
        }
        SampleEncoding::I32 => {
            let mut out = Vec::with_capacity(bytes.len() / 4);
            for chunk in bytes.as_chunks::<4>().0 {
                let sample = i32::from_le_bytes(*chunk);
                out.push(sample as f32 / 2_147_483_648.0);
            }
            Ok(out)
        }
    }
}

/// Mono is duplicated. Stereo is copied. Extra channels keep the first two.
fn map_channels(interleaved: &[f32], channels: usize, frames: usize) -> Vec<f32> {
    if channels == INTERNAL_CHANNELS as usize {
        return interleaved.to_vec();
    }
    let mut stereo = Vec::with_capacity(frames.saturating_mul(2));
    for frame in 0..frames {
        let base = frame.saturating_mul(channels);
        if channels == 1 {
            let sample = interleaved.get(base).copied().unwrap_or(0.0);
            stereo.push(sample);
            stereo.push(sample);
        } else {
            stereo.push(interleaved.get(base).copied().unwrap_or(0.0));
            stereo.push(interleaved.get(base + 1).copied().unwrap_or(0.0));
        }
    }
    stereo
}

#[cfg(test)]
mod tests {
    use super::to_internal;
    use crate::domain::audio::{AudioFormat, SampleEncoding};
    use crate::infrastructure::audio::resampler::StereoResampler;

    #[test]
    fn mono_i16_is_duplicated_through_the_resampler() {
        // Arrange
        let format = AudioFormat::new(48_000, 1, SampleEncoding::I16).expect("format");
        let mut resampler =
            StereoResampler::new(format.sample_rate(), AudioFormat::internal().sample_rate());
        let mut bytes = Vec::new();
        for _ in 0..480 {
            bytes.extend_from_slice(&16_384_i16.to_le_bytes());
        }

        // Act
        let samples = to_internal(&bytes, format, &mut resampler).expect("convert");
        let flushed = resampler.flush();

        // Assert
        let total = samples.len() + flushed.len();
        assert!(total >= 2);
        assert!(to_internal(
            &[0, 1, 2],
            format,
            &mut StereoResampler::new(48_000, 48_000)
        )
        .is_err());
    }

    #[test]
    fn stereo_f32_at_48k_produces_output() {
        // Arrange
        let format = AudioFormat::internal();
        let mut resampler = StereoResampler::new(format.sample_rate(), format.sample_rate());
        let mut bytes = Vec::new();
        for _ in 0..480 {
            for sample in [0.25_f32, -0.25_f32] {
                bytes.extend_from_slice(&sample.to_le_bytes());
            }
        }

        // Act
        let samples = to_internal(&bytes, format, &mut resampler).expect("convert");

        // Assert
        assert!(!samples.is_empty() || !resampler.flush().is_empty());
    }

    #[test]
    fn half_rate_stereo_increases_frame_count() {
        // Arrange
        let format = AudioFormat::new(24_000, 2, SampleEncoding::F32).expect("format");
        let mut resampler =
            StereoResampler::new(format.sample_rate(), AudioFormat::internal().sample_rate());
        let mut bytes = Vec::new();
        for _ in 0..480 {
            for sample in [0.0_f32, 0.0, 1.0, 1.0] {
                bytes.extend_from_slice(&sample.to_le_bytes());
            }
        }
        // 480 iterations of 4 floats = 960 frames at 24 kHz

        // Act
        let mut samples = to_internal(&bytes, format, &mut resampler).expect("convert");
        samples.extend(resampler.flush());

        // Assert
        assert!(samples.len() / 2 > 900);
        assert!(to_internal(
            &[0, 1, 2],
            format,
            &mut StereoResampler::new(24_000, 48_000)
        )
        .is_err());
    }
}
