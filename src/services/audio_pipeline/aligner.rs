//! Session timeline for microphone and system audio.
//!
//! A block is emitted when both active sources have it, or when the block is
//! about 150 ms behind the session clock. The missing side is silence. A
//! quiet loopback does not stop the clock.
//!
//! Gradual clock drift stays on each track as [`AlignState::last_error`].
//! The pipeline reads it on the session thread and nudges that source's
//! resampler (D-06). Sudden holes and overlaps still use silence and skip
//! from [`plan_packet`].

use std::collections::VecDeque;

use crate::domain::audio::AudioBlock;
use crate::domain::chunk_plan::{
    block_is_late, plan_packet, AlignState, DRIFT_THRESHOLD_FRAMES, MIX_BLOCK_FRAMES,
};

/// One aligned block. `frames` is `MIX_BLOCK_FRAMES` while recording and may
/// be shorter on the stop tail. Both sides are stereo interleaved and the
/// same length.
pub(crate) struct AlignedPair {
    pub(crate) mic: Vec<f32>,
    pub(crate) system: Vec<f32>,
    pub(crate) frames: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct AlignDiagnostics {
    pub(crate) gaps: u64,
    pub(crate) discontinuities: u64,
    pub(crate) drift_adjustments: u64,
}

/// Which capture source an aligner track and its resampler belong to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AudioSource {
    Mic,
    System,
}

struct Track {
    align: AlignState,
    pending: VecDeque<f32>,
    pending_origin: u64,
    gaps: u64,
    discontinuities: u64,
    drift_adjustments: u64,
}

impl Track {
    fn new() -> Self {
        Self {
            align: AlignState::new(),
            pending: VecDeque::new(),
            pending_origin: 0,
            gaps: 0,
            discontinuities: 0,
            drift_adjustments: 0,
        }
    }

    fn pending_frames(&self) -> u64 {
        (self.pending.len() / 2) as u64
    }

    fn push(&mut self, block: AudioBlock) {
        if block.discontinuity {
            self.discontinuities = self.discontinuities.saturating_add(1);
        }
        let frames = (block.samples.len() / 2) as u64;
        let plan = plan_packet(
            &mut self.align,
            block.timeline_frame,
            frames,
            block.discontinuity,
        );
        if plan.gap {
            self.gaps = self.gaps.saturating_add(1);
        }
        if self.align.last_error.unsigned_abs() > DRIFT_THRESHOLD_FRAMES {
            self.drift_adjustments = self.drift_adjustments.saturating_add(1);
        }
        for _ in 0..plan.silence_frames {
            self.pending.push_back(0.0);
            self.pending.push_back(0.0);
        }
        let skip = (plan.skip_frames as usize).saturating_mul(2);
        let take = (plan.take_frames as usize).saturating_mul(2);
        let end = skip.saturating_add(take).min(block.samples.len());
        if skip < end {
            self.pending
                .extend(block.samples[skip..end].iter().copied());
        }
    }

    fn take(&mut self, frames: u64, pad: bool) -> Option<Vec<f32>> {
        let have = self.pending_frames();
        if have < frames && !pad {
            return None;
        }
        let take = have.min(frames) as usize;
        let mut out = Vec::with_capacity((frames as usize).saturating_mul(2));
        for _ in 0..take.saturating_mul(2) {
            out.push(self.pending.pop_front().unwrap_or(0.0));
        }
        let missing = (frames as usize).saturating_sub(take);
        out.extend(std::iter::repeat_n(0.0, missing.saturating_mul(2)));
        self.pending_origin = self.pending_origin.saturating_add(frames);
        if self.pending_origin > self.align.cursor {
            self.align.cursor = self.pending_origin;
            self.align.last_error = 0;
        }
        Some(out)
    }
}

pub(crate) struct Aligner {
    mic: Track,
    system: Option<Track>,
    next_frame: u64,
}

impl Aligner {
    pub(crate) fn new(system_active: bool) -> Self {
        Self {
            mic: Track::new(),
            system: system_active.then(Track::new),
            next_frame: 0,
        }
    }

    /// Timeline error in frames for this source. Positive means it is running slow.
    pub(crate) fn drift_error(&self, source: AudioSource) -> i64 {
        match source {
            AudioSource::Mic => self.mic.align.last_error,
            AudioSource::System => self
                .system
                .as_ref()
                .map(|track| track.align.last_error)
                .unwrap_or(0),
        }
    }

    pub(crate) fn push_mic(&mut self, block: AudioBlock) {
        self.mic.push(block);
    }

    pub(crate) fn push_system(&mut self, block: AudioBlock) {
        if let Some(system) = self.system.as_mut() {
            system.push(block);
        }
    }

    /// `tail` drains leftover frames after stop and does not extend the
    /// timeline out to the session clock. A tail block may be shorter than
    /// 10 ms.
    pub(crate) fn pull(&mut self, now_frame: u64, tail: bool) -> Option<AlignedPair> {
        let want = if tail {
            let mic = self.mic.pending_frames();
            let system = self.system.as_ref().map(Track::pending_frames).unwrap_or(0);
            let pending = mic.max(system);
            if pending == 0 {
                return None;
            }
            pending.min(MIX_BLOCK_FRAMES)
        } else {
            MIX_BLOCK_FRAMES
        };
        let mic_ready = self.mic.pending_frames() >= want;
        let system_ready = self
            .system
            .as_ref()
            .map(|track| track.pending_frames() >= want)
            .unwrap_or(true);
        let late = !tail && block_is_late(self.next_frame, now_frame);
        if !tail && !(mic_ready && system_ready) && !late {
            return None;
        }
        let pad = tail || late || !mic_ready || !system_ready;
        let mic = self.mic.take(want, pad || mic_ready)?;
        let system = if let Some(system) = self.system.as_mut() {
            system.take(want, pad || system_ready)?
        } else {
            vec![0.0; (want as usize).saturating_mul(2)]
        };
        self.next_frame = self.next_frame.saturating_add(want);
        Some(AlignedPair {
            mic,
            system,
            frames: want,
        })
    }

    pub(crate) fn diagnostics(&self) -> AlignDiagnostics {
        let system_gaps = self.system.as_ref().map(|track| track.gaps).unwrap_or(0);
        let system_disc = self
            .system
            .as_ref()
            .map(|track| track.discontinuities)
            .unwrap_or(0);
        let system_drift = self
            .system
            .as_ref()
            .map(|track| track.drift_adjustments)
            .unwrap_or(0);
        AlignDiagnostics {
            gaps: self.mic.gaps.saturating_add(system_gaps),
            discontinuities: self.mic.discontinuities.saturating_add(system_disc),
            drift_adjustments: self.mic.drift_adjustments.saturating_add(system_drift),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Aligner;
    use crate::domain::audio::AudioBlock;
    use crate::domain::chunk_plan::{LATENCY_LIMIT_FRAMES, MIX_BLOCK_FRAMES};
    use crate::domain::gain::{soft_limit, SYSTEM_GAIN};

    fn aligner(system: bool) -> Aligner {
        Aligner::new(system)
    }

    fn tone(start: Option<u64>, frames: usize, sample: f32) -> AudioBlock {
        AudioBlock {
            samples: vec![sample; frames.saturating_mul(2)],
            timeline_frame: start,
            discontinuity: false,
        }
    }

    fn collect(aligner: &mut Aligner, now: u64, tail: bool) -> (Vec<f32>, Vec<f32>) {
        let mut mic = Vec::new();
        let mut system = Vec::new();
        while let Some(block) = aligner.pull(now, tail) {
            mic.extend(block.mic);
            system.extend(block.system);
        }
        (mic, system)
    }

    /// Mic gain is 1.0 and AGC is off in these tests, so this is the mix the
    /// old `Mixer` assertions observed.
    fn unity_mix(mic: &[f32], system: &[f32]) -> Vec<f32> {
        let mut mixed = Vec::with_capacity(mic.len());
        let count = mic.len().min(system.len());
        let mut index = 0;
        while index + 1 < count {
            let left_system = system[index] * SYSTEM_GAIN;
            let right_system = system[index + 1] * SYSTEM_GAIN;
            let left_sum = f64::from(mic[index]) + f64::from(left_system);
            let right_sum = f64::from(mic[index + 1]) + f64::from(right_system);
            mixed.push(soft_limit(left_sum as f32));
            mixed.push(soft_limit(right_sum as f32));
            index += 2;
        }
        mixed
    }

    #[test]
    fn sources_that_start_apart_share_one_timeline() {
        // Arrange
        let mut aligner = aligner(true);

        // Act
        aligner.push_mic(tone(Some(0), 2_880, 0.2));
        aligner.push_system(tone(Some(2_400), 480, 0.5));
        let (mic, system) = collect(&mut aligner, 0, false);
        let mixed = unity_mix(&mic, &system);

        // Assert
        assert_eq!(mixed.len(), 2_880 * 2);
        assert!((mixed[0] - 0.2).abs() < 0.0001);
        assert!((mixed[2] - 0.2).abs() < 0.0001);
        let at_lead = 2_400 * 2;
        let expected = soft_limit(0.2 + 0.5 * SYSTEM_GAIN);
        assert!((mixed[at_lead] - expected).abs() < 0.0001);
    }

    #[test]
    fn thirty_seconds_of_silent_system_audio_does_not_contract() {
        // Arrange
        let mut aligner = aligner(true);
        let frames = 48_000_u64 * 30;

        // Act
        aligner.push_mic(tone(Some(0), frames as usize, 0.2));
        let (mic, system) = collect(&mut aligner, frames + LATENCY_LIMIT_FRAMES, false);
        let mixed = unity_mix(&mic, &system);

        // Assert
        assert_eq!(mixed.len() as u64, frames * 2);
        assert!((mixed[0] - 0.2).abs() < 0.0001);
        assert!((mixed[1] - 0.2).abs() < 0.0001);
        assert!((mixed[mixed.len() - 2] - 0.2).abs() < 0.0001);
    }

    #[test]
    fn gaps_overlaps_and_discontinuity_follow_the_timeline() {
        // Arrange
        let mut aligner = aligner(false);

        // Act
        aligner.push_mic(tone(Some(0), 480, 0.5));
        aligner.push_mic(tone(Some(480 + 240), 480, 0.25));
        aligner.push_mic(AudioBlock {
            samples: vec![0.5; 20],
            timeline_frame: Some(480 + 240 + 480 + 10),
            discontinuity: true,
        });
        let late = {
            let mut other = super::tests::aligner(false);
            other.push_mic(tone(Some(0), 480, 0.5));
            other.push_mic(tone(Some(280), 480, 0.25));
            let (mic, system) = collect(&mut other, 0, true);
            unity_mix(&mic, &system)
        };
        let (mic, system) = collect(&mut aligner, 0, true);
        let mixed = unity_mix(&mic, &system);

        // Assert
        assert_eq!(mixed.len(), (480 + 240 + 480 + 10 + 10) * 2);
        assert!((mixed[0] - 0.5).abs() < 0.0001);
        assert_eq!(mixed[480 * 2], 0.0);
        assert!((mixed[(480 + 240) * 2] - 0.25).abs() < 0.0001);
        assert!(aligner.diagnostics().gaps >= 1);
        assert!(aligner.diagnostics().discontinuities >= 1);
        assert_eq!(late.len(), (480 + 280) * 2);
        assert!((late[0] - 0.5).abs() < 0.0001);
        assert!((late[480 * 2] - 0.25).abs() < 0.0001);
    }

    #[test]
    fn a_missing_source_is_silence_and_the_tail_is_not_padded() {
        // Arrange
        let mut aligner = aligner(false);
        aligner.push_mic(tone(None, 200, 0.3));

        // Act
        let waiting = aligner.pull(0, false);
        let (mic, system) = collect(&mut aligner, 0, true);
        let tail = unity_mix(&mic, &system);

        // Assert
        assert!(waiting.is_none());
        assert_eq!(tail.len(), 400);
        assert!((tail[0] - 0.3).abs() < 0.0001);
        assert!((tail[1] - 0.3).abs() < 0.0001);
    }

    #[test]
    fn late_blocks_zero_fill_the_source_that_has_not_spoken() {
        // Arrange
        let mut aligner = aligner(true);
        aligner.push_mic(tone(Some(0), MIX_BLOCK_FRAMES as usize, 0.4));

        // Act
        let early = aligner.pull(0, false);
        let late = aligner
            .pull(LATENCY_LIMIT_FRAMES + 1, false)
            .expect("late block");
        let samples = unity_mix(&late.mic, &late.system);

        // Assert
        assert!(early.is_none());
        assert_eq!(late.frames, MIX_BLOCK_FRAMES);
        assert!((samples[0] - 0.4).abs() < 0.0001);
        assert!((samples[1] - 0.4).abs() < 0.0001);
    }

    #[test]
    fn queue_overrun_gap_is_silence_on_the_shared_timeline() {
        // Arrange
        let mut aligner = aligner(false);

        // Act
        aligner.push_mic(tone(Some(0), 480, 0.2));
        aligner.push_mic(tone(Some(960), 480, 0.2));
        let (mic, system) = collect(&mut aligner, 0, false);
        let mixed = unity_mix(&mic, &system);

        // Assert
        assert_eq!(mixed.len(), (480 + 480 + 480) * 2);
        assert_eq!(mixed[480 * 2], 0.0);
        assert!(aligner.diagnostics().gaps >= 1);
    }
}
