//! 10 ms stereo mix of microphone and system audio.
//!
//! A block is emitted when both active sources have it, or when the block is
//! about 150 ms behind the session clock. The missing side is silence. A
//! quiet loopback does not stop the clock. Gains are mic (live) and system
//! 0.8. Sums use `f64` and then the soft limiter.

use std::collections::VecDeque;
use std::sync::Arc;

use crate::domain::chunk_plan::{block_is_late, plan_packet, AlignState, MIX_BLOCK_FRAMES};
use crate::domain::gain::{soft_limit, Agc, LIMIT_KNEE, SYSTEM_GAIN};

use super::levels::{GainControl, LevelSnapshot};
use super::queue::AudioBlock;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct MixDiagnostics {
    pub(crate) gaps: u64,
    pub(crate) discontinuities: u64,
    pub(crate) drift_drops: u64,
    pub(crate) drift_duplicates: u64,
    pub(crate) limited_samples: u64,
}

pub(crate) struct MixedBlock {
    pub(crate) samples: Vec<f32>,
    pub(crate) frames: u64,
}

struct Track {
    align: AlignState,
    pending: VecDeque<f32>,
    pending_origin: u64,
    gaps: u64,
    discontinuities: u64,
    drift_drops: u64,
    drift_duplicates: u64,
}

impl Track {
    fn new() -> Self {
        Self {
            align: AlignState::new(),
            pending: VecDeque::new(),
            pending_origin: 0,
            gaps: 0,
            discontinuities: 0,
            drift_drops: 0,
            drift_duplicates: 0,
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
        self.drift_drops = self.drift_drops.saturating_add(plan.drift_drop);
        self.drift_duplicates = self.drift_duplicates.saturating_add(plan.drift_duplicate);
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
        if plan.duplicate_frames > 0 {
            let (left, right) = if self.pending.len() >= 2 {
                let last = self.pending.len() - 2;
                (self.pending[last], self.pending[last + 1])
            } else {
                (0.0, 0.0)
            };
            for _ in 0..plan.duplicate_frames {
                self.pending.push_back(left);
                self.pending.push_back(right);
            }
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

pub(crate) struct Mixer {
    mic: Track,
    system: Option<Track>,
    next_frame: u64,
    agc: Agc,
    gain: Arc<GainControl>,
    levels: Arc<LevelSnapshot>,
    limited_samples: u64,
}

impl Mixer {
    pub(crate) fn new(
        system_active: bool,
        gain: Arc<GainControl>,
        levels: Arc<LevelSnapshot>,
    ) -> Self {
        let slider = gain.slider();
        Self {
            mic: Track::new(),
            system: system_active.then(Track::new),
            next_frame: 0,
            agc: Agc::new(slider),
            gain,
            levels,
            limited_samples: 0,
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

    pub(crate) fn diagnostics(&self) -> MixDiagnostics {
        let system_gaps = self.system.as_ref().map(|track| track.gaps).unwrap_or(0);
        let system_disc = self
            .system
            .as_ref()
            .map(|track| track.discontinuities)
            .unwrap_or(0);
        let system_drop = self
            .system
            .as_ref()
            .map(|track| track.drift_drops)
            .unwrap_or(0);
        let system_dup = self
            .system
            .as_ref()
            .map(|track| track.drift_duplicates)
            .unwrap_or(0);
        MixDiagnostics {
            gaps: self.mic.gaps.saturating_add(system_gaps),
            discontinuities: self.mic.discontinuities.saturating_add(system_disc),
            drift_drops: self.mic.drift_drops.saturating_add(system_drop),
            drift_duplicates: self.mic.drift_duplicates.saturating_add(system_dup),
            limited_samples: self.limited_samples,
        }
    }

    /// `tail` drains leftover frames after stop and does not extend the
    /// timeline out to the session clock.
    pub(crate) fn pull(&mut self, now_frame: u64, tail: bool) -> Option<MixedBlock> {
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
        let mixed = self.mix(&mic, &system);
        self.next_frame = self.next_frame.saturating_add(want);
        Some(MixedBlock {
            samples: mixed,
            frames: want,
        })
    }

    fn mix(&mut self, mic: &[f32], system: &[f32]) -> Vec<f32> {
        let slider = self.gain.slider();
        let mic_gain = self.agc.process(mic, self.gain.agc_enabled(), slider);
        self.gain.set_effective(mic_gain);
        let mut mic_peak = 0.0_f32;
        let mut system_peak = 0.0_f32;
        let mut mix_peak = 0.0_f32;
        let mut mixed = Vec::with_capacity(mic.len());
        let count = mic.len().min(system.len());
        let mut index = 0;
        while index + 1 < count {
            let left_mic = mic[index] * mic_gain;
            let right_mic = mic[index + 1] * mic_gain;
            let left_system = system[index] * SYSTEM_GAIN;
            let right_system = system[index + 1] * SYSTEM_GAIN;
            mic_peak = mic_peak.max(left_mic.abs()).max(right_mic.abs());
            system_peak = system_peak.max(left_system.abs()).max(right_system.abs());
            let left_sum = f64::from(left_mic) + f64::from(left_system);
            let right_sum = f64::from(right_mic) + f64::from(right_system);
            if left_sum.abs() > f64::from(LIMIT_KNEE) || right_sum.abs() > f64::from(LIMIT_KNEE) {
                self.limited_samples = self.limited_samples.saturating_add(1);
            }
            let left = soft_limit(left_sum as f32);
            let right = soft_limit(right_sum as f32);
            mix_peak = mix_peak.max(left.abs()).max(right.abs());
            mixed.push(left);
            mixed.push(right);
            index += 2;
        }
        self.levels.publish(mic_peak, system_peak, mix_peak);
        mixed
    }
}

#[cfg(test)]
mod tests {
    use super::Mixer;
    use crate::domain::chunk_plan::{
        DRIFT_THRESHOLD_FRAMES, LATENCY_LIMIT_FRAMES, MIX_BLOCK_FRAMES,
    };
    use crate::domain::gain::{soft_limit, MIC_GAIN_DEFAULT, SYSTEM_GAIN};
    use crate::infrastructure::audio::levels::{GainControl, LevelSnapshot};
    use crate::infrastructure::audio::queue::AudioBlock;
    use std::sync::Arc;

    fn mixer(system: bool, gain: f32) -> Mixer {
        Mixer::new(
            system,
            Arc::new(GainControl::new(gain, false)),
            Arc::new(LevelSnapshot::new()),
        )
    }

    fn tone(start: Option<u64>, frames: usize, sample: f32) -> AudioBlock {
        AudioBlock {
            samples: vec![sample; frames.saturating_mul(2)],
            timeline_frame: start,
            discontinuity: false,
        }
    }

    fn collect(mixer: &mut Mixer, now: u64, tail: bool) -> Vec<f32> {
        let mut out = Vec::new();
        while let Some(block) = mixer.pull(now, tail) {
            out.extend(block.samples);
        }
        out
    }

    #[test]
    fn sources_that_start_apart_share_one_timeline() {
        // Arrange
        let mut mixer = mixer(true, 1.0);

        // Act
        mixer.push_mic(tone(Some(0), 2_880, 0.2));
        mixer.push_system(tone(Some(2_400), 480, 0.5));
        let mixed = collect(&mut mixer, 0, false);

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
        let mut mixer = mixer(true, 1.0);
        let frames = 48_000_u64 * 30;

        // Act
        mixer.push_mic(tone(Some(0), frames as usize, 0.2));
        let mixed = collect(&mut mixer, frames + LATENCY_LIMIT_FRAMES, false);

        // Assert
        assert_eq!(mixed.len() as u64, frames * 2);
        assert!((mixed[0] - 0.2).abs() < 0.0001);
        assert!((mixed[1] - 0.2).abs() < 0.0001);
        assert!((mixed[mixed.len() - 2] - 0.2).abs() < 0.0001);
    }

    #[test]
    fn gaps_overlaps_and_discontinuity_follow_the_timeline() {
        // Arrange
        let mut mixer = mixer(false, 1.0);

        // Act
        mixer.push_mic(tone(Some(0), 480, 0.5));
        mixer.push_mic(tone(Some(480 + 240), 480, 0.25));
        mixer.push_mic(AudioBlock {
            samples: vec![0.5; 20],
            timeline_frame: Some(480 + 240 + 480 + 10),
            discontinuity: true,
        });
        let late = {
            let mut other = super::tests::mixer(false, 1.0);
            other.push_mic(tone(Some(0), 480, 0.5));
            other.push_mic(tone(Some(280), 480, 0.25));
            collect(&mut other, 0, true)
        };
        let mixed = collect(&mut mixer, 0, true);

        // Assert
        assert_eq!(mixed.len(), (480 + 240 + 480 + 10 + 10) * 2);
        assert!((mixed[0] - 0.5).abs() < 0.0001);
        assert_eq!(mixed[480 * 2], 0.0);
        assert!((mixed[(480 + 240) * 2] - 0.25).abs() < 0.0001);
        assert!(mixer.diagnostics().gaps >= 1);
        assert!(mixer.diagnostics().discontinuities >= 1);
        assert_eq!(late.len(), (480 + 280) * 2);
        assert!((late[0] - 0.5).abs() < 0.0001);
        assert!((late[480 * 2] - 0.25).abs() < 0.0001);
    }

    #[test]
    fn a_missing_source_is_silence_and_the_tail_is_not_padded() {
        // Arrange
        let mut mixer = mixer(false, 1.0);
        mixer.push_mic(tone(None, 200, 0.3));

        // Act
        let waiting = mixer.pull(0, false);
        let tail = collect(&mut mixer, 0, true);

        // Assert
        assert!(waiting.is_none());
        assert_eq!(tail.len(), 400);
        assert!((tail[0] - 0.3).abs() < 0.0001);
        assert!((tail[1] - 0.3).abs() < 0.0001);
    }

    #[test]
    fn late_blocks_zero_fill_the_source_that_has_not_spoken() {
        // Arrange
        let mut mixer = mixer(true, MIC_GAIN_DEFAULT);
        mixer.push_mic(tone(Some(0), MIX_BLOCK_FRAMES as usize, 0.4));

        // Act
        let early = mixer.pull(0, false);
        let late = mixer
            .pull(LATENCY_LIMIT_FRAMES + 1, false)
            .expect("late block");

        // Assert
        assert!(early.is_none());
        assert_eq!(late.frames, MIX_BLOCK_FRAMES);
        assert!((late.samples[0] - 0.4).abs() < 0.0001);
        assert!((late.samples[1] - 0.4).abs() < 0.0001);
    }

    #[test]
    fn hot_sums_enter_the_limiter() {
        // Arrange
        let mut mixer = mixer(true, 1.0);

        // Act
        mixer.push_mic(tone(Some(0), 480, 1.0));
        mixer.push_system(tone(Some(0), 480, 1.0));
        let mixed = collect(&mut mixer, 0, false);
        let expected = soft_limit(1.0 + SYSTEM_GAIN);

        // Assert
        assert!(!mixed.is_empty());
        assert!(mixed[0].abs() < 1.0);
        assert!((mixed[0] - expected).abs() < 0.0001);
        assert!(mixer.diagnostics().limited_samples > 0);
    }

    #[test]
    fn queue_overrun_gap_is_silence_on_the_shared_timeline() {
        // Arrange
        let mut mixer = mixer(false, 1.0);

        // Act
        mixer.push_mic(tone(Some(0), 480, 0.2));
        mixer.push_mic(tone(Some(960), 480, 0.2));
        let mixed = collect(&mut mixer, 0, false);

        // Assert
        assert_eq!(mixed.len(), (480 + 480 + 480) * 2);
        assert_eq!(mixed[480 * 2], 0.0);
        assert!(mixer.diagnostics().gaps >= 1);
    }

    #[test]
    fn hundred_ppm_clocks_stay_within_about_20ms() {
        // Arrange
        let frames = 4_800_usize;
        let packets = 2_200_u64;
        let mut fast = mixer(false, 1.0);
        let mut slow = mixer(false, 1.0);
        let mut fast_frames = 0_u64;
        let mut slow_frames = 0_u64;

        // Act
        for index in 0..packets {
            let fast_start = index * frames as u64 * 10_000 / 10_001;
            let slow_start = index * frames as u64 * 10_001 / 10_000;
            fast.push_mic(tone(Some(fast_start), frames, 0.1));
            slow.push_mic(tone(Some(slow_start), frames, 0.1));
            fast_frames += collect(&mut fast, 0, false).len() as u64 / 2;
            slow_frames += collect(&mut slow, 0, false).len() as u64 / 2;
        }
        fast_frames += collect(&mut fast, 0, true).len() as u64 / 2;
        slow_frames += collect(&mut slow, 0, true).len() as u64 / 2;

        // Assert
        let produced = packets * frames as u64;
        assert!(fast.diagnostics().drift_drops > 0);
        assert!(slow.diagnostics().drift_duplicates > 0);
        let fast_qpc = produced * 10_000 / 10_001;
        let slow_qpc = produced * 10_001 / 10_000;
        let allowance = DRIFT_THRESHOLD_FRAMES + frames as u64;
        assert!(fast_frames.abs_diff(fast_qpc) <= allowance);
        assert!(slow_frames.abs_diff(slow_qpc) <= allowance);
        assert!(fast_frames < produced);
        assert!(slow_frames > produced);
    }
}
