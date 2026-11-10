//! Energy-based pause detector for 16 kHz mono audio. Pure math, no OS calls.
//!
//! Frames are 20 ms (320 samples). Loudness is RMS in dBFS. The noise floor is
//! the 10th percentile of frame loudness over the last 10 s, so steady
//! background noise raises the floor instead of counting as speech.

// Wired in by package B (offline piece transcription).
#![allow(dead_code)]

use std::collections::VecDeque;

use crate::services::ports::{PauseDetector, PauseDetectorFactory, VoiceFrame};

const SAMPLE_RATE: u64 = 16_000;
const FRAME_SAMPLES: usize = 320;
const FRAME_MS: u64 = 20;
/// 10 s of 20 ms frames.
const FLOOR_WINDOW_FRAMES: usize = 500;
/// Share of the window that sits below the noise floor.
const FLOOR_PERCENTILE: f32 = 0.10;
/// A frame must be this far above the floor to count as speech.
const SPEECH_MARGIN_DB: f32 = 8.0;
/// Frames below this are silent whatever the floor says.
const ABSOLUTE_SILENCE_DBFS: f32 = -50.0;
/// Distance above the floor that scores 1.
const FULL_SCORE_DB: f32 = 30.0;
/// Loudness of digital silence.
const MIN_DBFS: f32 = -120.0;

#[derive(Debug, Default)]
pub(crate) struct EnergyPauseDetector {
    /// Samples of an incomplete frame carried to the next `push`.
    pending: Vec<f32>,
    /// Loudness of the last `FLOOR_WINDOW_FRAMES` frames.
    loudness: VecDeque<f32>,
    /// Samples already turned into frames, including a partial final frame.
    consumed: u64,
}

impl EnergyPauseDetector {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn analyse(&mut self, frame: &[f32]) -> VoiceFrame {
        let start_ms = self.consumed * 1_000 / SAMPLE_RATE;
        self.consumed += frame.len() as u64;
        let end_ms = self.consumed * 1_000 / SAMPLE_RATE;

        let db = loudness_dbfs(frame);
        if self.loudness.len() == FLOOR_WINDOW_FRAMES {
            self.loudness.pop_front();
        }
        self.loudness.push_back(db);
        let floor = self.noise_floor();

        let silent = db < floor + SPEECH_MARGIN_DB || db < ABSOLUTE_SILENCE_DBFS;
        let score = (db - floor) / FULL_SCORE_DB;
        VoiceFrame::new(start_ms, end_ms, silent, score)
    }

    fn noise_floor(&self) -> f32 {
        let mut sorted: Vec<f32> = self.loudness.iter().copied().collect();
        sorted.sort_by(f32::total_cmp);
        let index = ((sorted.len() - 1) as f32 * FLOOR_PERCENTILE) as usize;
        sorted[index]
    }
}

fn loudness_dbfs(frame: &[f32]) -> f32 {
    if frame.is_empty() {
        return MIN_DBFS;
    }
    let mean_square = frame.iter().map(|s| s * s).sum::<f32>() / frame.len() as f32;
    if mean_square <= 0.0 {
        return MIN_DBFS;
    }
    (10.0 * mean_square.log10()).max(MIN_DBFS)
}

impl PauseDetector for EnergyPauseDetector {
    fn push(&mut self, samples_16k_mono: &[f32]) -> Vec<VoiceFrame> {
        self.pending.extend_from_slice(samples_16k_mono);
        let whole = self.pending.len() / FRAME_SAMPLES * FRAME_SAMPLES;
        let ready: Vec<f32> = self.pending.drain(..whole).collect();
        let (frames, _) = ready.as_chunks::<FRAME_SAMPLES>();
        frames.iter().map(|frame| self.analyse(frame)).collect()
    }

    fn finish(&mut self) -> Vec<VoiceFrame> {
        if self.pending.is_empty() {
            return Vec::new();
        }
        let rest = std::mem::take(&mut self.pending);
        vec![self.analyse(&rest)]
    }
}

/// Builds energy detectors. It holds no state, so every job starts clean.
#[derive(Debug, Default)]
pub(crate) struct EnergyPauseDetectorFactory;

impl PauseDetectorFactory for EnergyPauseDetectorFactory {
    fn begin(&mut self) -> Box<dyn PauseDetector + '_> {
        Box::new(EnergyPauseDetector::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(samples: usize, amplitude: f32) -> Vec<f32> {
        (0..samples)
            .map(|i| amplitude * (i as f32 * 2.0 * std::f32::consts::PI * 440.0 / 16_000.0).sin())
            .collect()
    }

    /// Uniform noise in -amplitude..amplitude from a fixed seed.
    fn noise(samples: usize, amplitude: f32) -> Vec<f32> {
        let mut state = 0x9e37_79b9_u32;
        (0..samples)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state >> 8) as f32 / (1 << 23) as f32 * amplitude - amplitude
            })
            .collect()
    }

    fn run_all(audio: &[f32]) -> Vec<VoiceFrame> {
        let mut detector = EnergyPauseDetector::new();
        let mut frames = detector.push(audio);
        frames.extend(detector.finish());
        frames
    }

    #[test]
    fn digital_silence_is_silent_with_score_zero() {
        // Arrange
        let audio = vec![0.0; 16_000];

        // Act
        let frames = run_all(&audio);

        // Assert
        assert_eq!(frames.len(), 50);
        assert!(frames.iter().all(|f| f.silent() && f.speech_score() == 0.0));
    }

    #[test]
    fn a_tone_far_above_the_noise_is_speech() {
        // Arrange
        let mut audio = noise(32_000, 0.01);
        audio.extend(tone(16_000, 0.5));

        // Act
        let frames = run_all(&audio);

        // Assert
        assert!(frames[..100].iter().all(|f| f.silent()));
        assert!(frames[100..].iter().all(|f| !f.silent()));
        assert!(frames[100..].iter().all(|f| f.speech_score() > 0.99));
    }

    #[test]
    fn a_quiet_tone_below_minus_fifty_dbfs_stays_silent() {
        // Arrange: -60 dBFS tone after near-digital silence, so the floor is far lower.
        let mut audio = vec![0.0; 16_000];
        audio.extend(tone(16_000, 0.0014));

        // Act
        let frames = run_all(&audio);

        // Assert
        assert!(frames[50..].iter().all(|f| f.silent()));
        assert!(frames[50..].iter().all(|f| f.speech_score() > 0.0));
    }

    #[test]
    fn the_floor_adapts_to_steady_background_noise() {
        // Arrange: noise near -31 dBFS is loud in absolute terms.
        let mut audio = noise(16_000 * 12, 0.05);
        audio.extend(tone(16_000, 0.12));

        // Act
        let frames = run_all(&audio);

        // Assert
        let noisy = &frames[..600];
        assert!(noisy.iter().all(|f| f.silent()));
        let spoken = &frames[600..];
        assert!(spoken.iter().all(|f| !f.silent()));
        assert!(spoken
            .iter()
            .all(|f| (0.2..0.9).contains(&f.speech_score())));
    }

    #[test]
    fn frame_times_run_on_without_gaps_across_odd_push_sizes() {
        // Arrange
        let audio = tone(10_000, 0.3);
        let sizes = [1, 7, 333, 319, 320, 321, 1, 2_000, 5];
        let mut detector = EnergyPauseDetector::new();
        let mut frames = Vec::new();

        // Act
        let mut offset = 0;
        for size in sizes.iter().cycle() {
            if offset >= audio.len() {
                break;
            }
            let end = (offset + size).min(audio.len());
            frames.extend(detector.push(&audio[offset..end]));
            offset = end;
        }
        frames.extend(detector.finish());

        // Assert
        let mut expected_start = 0;
        for frame in &frames[..frames.len() - 1] {
            assert_eq!(frame.start_ms(), expected_start);
            assert_eq!(frame.end_ms(), expected_start + FRAME_MS);
            expected_start += FRAME_MS;
        }
        let last = frames.last().expect("partial frame");
        assert_eq!(last.start_ms(), expected_start);
        assert_eq!(last.end_ms(), 625); // 10 000 samples at 16 kHz
        assert_eq!(frames.len(), 32);
    }

    #[test]
    fn odd_push_sizes_give_the_same_frames_as_one_push() {
        // Arrange
        let mut audio = noise(8_000, 0.01);
        audio.extend(tone(8_000, 0.4));
        let whole = run_all(&audio);
        let mut detector = EnergyPauseDetector::new();

        // Act
        let mut split = Vec::new();
        for piece in audio.chunks(137) {
            split.extend(detector.push(piece));
        }
        split.extend(detector.finish());

        // Assert
        assert_eq!(split, whole);
    }

    #[test]
    fn finish_without_a_partial_frame_returns_nothing() {
        // Arrange
        let mut detector = EnergyPauseDetector::new();
        detector.push(&vec![0.0; FRAME_SAMPLES * 2]);

        // Act
        let rest = detector.finish();

        // Assert
        assert!(rest.is_empty());
    }

    #[test]
    fn the_factory_hands_out_independent_detectors() {
        // Arrange
        let mut factory = EnergyPauseDetectorFactory;
        let audio = vec![0.0; FRAME_SAMPLES];
        let mut first = factory.begin();
        first.push(&audio);
        drop(first);

        // Act
        let mut second = factory.begin();
        let frames = second.push(&audio);

        // Assert
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].start_ms(), 0);
    }
}
