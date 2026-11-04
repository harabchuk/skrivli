//! Convert captured PCM into the internal 48 kHz stereo `f32` layout.
//!
//! The hardware mix format is whatever WASAPI accepted. This module does not
//! assume the requested format was granted.

use crate::domain::audio::{AudioFormat, SampleEncoding, INTERNAL_CHANNELS};

use super::resampler::resample_stereo;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConvertError {
    Unaligned,
    Unsupported,
}

/// Decode `bytes` and return interleaved stereo `f32` at 48 kHz.
pub(crate) fn to_internal(bytes: &[u8], format: AudioFormat) -> Result<Vec<f32>, ConvertError> {
    let frame_bytes = format.bytes_per_frame().ok_or(ConvertError::Unsupported)?;
    if frame_bytes == 0 || !bytes.len().is_multiple_of(frame_bytes as usize) {
        return Err(ConvertError::Unaligned);
    }
    let frames = bytes.len() / frame_bytes as usize;
    let decoded = decode(bytes, format)?;
    let stereo = map_channels(&decoded, format.channels() as usize, frames);
    Ok(resample_stereo(
        &stereo,
        format.sample_rate(),
        AudioFormat::internal().sample_rate(),
    ))
}

/// `source_frames` of silence in the internal format, before resampling.
pub(crate) fn silence_internal(
    source_frames: u64,
    format: AudioFormat,
) -> Result<Vec<f32>, ConvertError> {
    let frame_bytes = format.bytes_per_frame().ok_or(ConvertError::Unsupported)?;
    let bytes = frame_bytes
        .checked_mul(source_frames)
        .ok_or(ConvertError::Unsupported)?;
    let byte_len = usize::try_from(bytes).map_err(|_| ConvertError::Unsupported)?;
    to_internal(&vec![0_u8; byte_len], format)
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

    #[test]
    fn mono_i16_is_duplicated_at_the_same_rate() {
        // Arrange
        let format = AudioFormat::new(48_000, 1, SampleEncoding::I16).expect("format");
        let bytes = 16_384_i16.to_le_bytes();

        // Act
        let samples = to_internal(&bytes, format).expect("convert");

        // Assert
        assert_eq!(samples.len(), 2);
        assert!((samples[0] - 0.5).abs() < 0.001);
        assert_eq!(samples[0], samples[1]);
    }

    #[test]
    fn stereo_f32_at_48k_is_copied() {
        // Arrange
        let format = AudioFormat::internal();
        let mut bytes = Vec::new();
        for sample in [0.25_f32, -0.25_f32] {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }

        // Act
        let samples = to_internal(&bytes, format).expect("convert");

        // Assert
        assert_eq!(samples, vec![0.25, -0.25]);
    }

    #[test]
    fn half_rate_stereo_doubles_the_frame_count() {
        // Arrange
        let format = AudioFormat::new(24_000, 2, SampleEncoding::F32).expect("format");
        let mut bytes = Vec::new();
        for sample in [0.0_f32, 0.0, 1.0, 1.0] {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }

        // Act
        let samples = to_internal(&bytes, format).expect("convert");

        // Assert
        assert_eq!(samples.len(), 8);
        assert!(to_internal(&[0, 1, 2], format).is_err());
    }
}
