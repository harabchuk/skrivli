//! Linear resampler into 48 kHz stereo interleaved `f32`.
//!
//! Output frame `i` reads the source at `i * in_rate / out_rate`. The fraction
//! stays in integers so a long recording does not accumulate binary float error
//! in the index. Each call is independent; the capture thread passes one
//! packet at a time after channel mapping.

use crate::domain::audio::INTERNAL_SAMPLE_RATE;

pub(crate) fn resample_stereo(interleaved: &[f32], in_rate: u32, out_rate: u32) -> Vec<f32> {
    if interleaved.len() < 2 || in_rate == 0 || out_rate == 0 {
        return Vec::new();
    }
    let in_frames = interleaved.len() / 2;
    if in_rate == out_rate {
        let used = in_frames.saturating_mul(2);
        return interleaved[..used].to_vec();
    }
    let out_frames = (in_frames as u128).saturating_mul(u128::from(out_rate)) / u128::from(in_rate);
    let out_frames = usize::try_from(out_frames).unwrap_or(0);
    let mut output = Vec::with_capacity(out_frames.saturating_mul(2));
    let in_rate = u64::from(in_rate);
    let out_rate = u64::from(out_rate);
    for index in 0..out_frames {
        let index = index as u64;
        let position = index.saturating_mul(in_rate) / out_rate;
        let fraction = index.saturating_mul(in_rate) % out_rate;
        let left = frame_at(interleaved, in_frames, position as usize);
        let right = frame_at(interleaved, in_frames, position as usize + 1);
        let (left, right) = interpolate(left, right, fraction, out_rate);
        output.push(left);
        output.push(right);
    }
    let _ = INTERNAL_SAMPLE_RATE;
    output
}

fn frame_at(interleaved: &[f32], in_frames: usize, frame: usize) -> (f32, f32) {
    let frame = frame.min(in_frames.saturating_sub(1));
    let base = frame.saturating_mul(2);
    (
        interleaved.get(base).copied().unwrap_or(0.0),
        interleaved.get(base + 1).copied().unwrap_or(0.0),
    )
}

fn interpolate(a: (f32, f32), b: (f32, f32), fraction: u64, out_rate: u64) -> (f32, f32) {
    if fraction == 0 || out_rate == 0 {
        return a;
    }
    let weight = fraction as f32 / out_rate as f32;
    (a.0 + (b.0 - a.0) * weight, a.1 + (b.1 - a.1) * weight)
}

#[cfg(test)]
mod tests {
    use super::resample_stereo;

    #[test]
    fn identity_rate_keeps_every_frame() {
        // Arrange
        let input = vec![0.1, 0.2, 0.3, 0.4];

        // Act
        let output = resample_stereo(&input, 48_000, 48_000);

        // Assert
        assert_eq!(output, input);
    }

    #[test]
    fn double_rate_inserts_a_midpoint() {
        // Arrange
        let input = vec![0.0, 0.0, 1.0, 1.0];

        // Act
        let output = resample_stereo(&input, 24_000, 48_000);

        // Assert
        assert_eq!(output.len(), 8);
        assert_eq!(output[0], 0.0);
        assert!((output[2] - 0.5).abs() < 0.001);
    }
}
