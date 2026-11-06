//! Session mix. Each source is resampled here, then placed on the timeline.
//!
//! `push_mic` / `push_system` read that source's drift error, resample, and
//! forward the block to [`Aligner`]. `finish_input` flushes both resamplers
//! after a normal stop has drained the capture queues. `source_reopened`
//! flushes and resets only the source that came back.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::domain::audio::AudioBlock;
use crate::domain::audio_frame::{AudioFrame, ChannelLayout, FRAME_LEN};
use crate::domain::gain::SYSTEM_GAIN;
use crate::infrastructure::audio::levels::{GainControl, LevelSnapshot, MeterSlot, PeakTap};
use crate::services::ports::{
    AudioMixer, DriftResampler, PipelineFactory, ProcessContext, ProcessError,
};

use super::echo_stats::{EchoStats, EchoStatsSlot};
use super::steps::LinearGain;
use super::{Aligner, AudioSource, Chain, FixedGain, SoftLimiter, SumMixer};

/// Inputs for one recording session. The production factory adds echo
/// cancellation when `system_active` is true. The pass-through factory does not.
pub(crate) struct PipelineSetup {
    pub(crate) system_active: bool,
    pub(crate) gain: Arc<GainControl>,
    pub(crate) levels: Arc<LevelSnapshot>,
}

struct SourceInput {
    resampler: Box<dyn DriftResampler>,
    pending_discontinuity: bool,
    /// `process` has seen audio since the last flush or reset.
    holding: bool,
    /// `process` or `flush` panicked. Later calls pass audio through and do
    /// not touch the resampler again.
    panicked: bool,
}

impl SourceInput {
    fn new(resampler: Box<dyn DriftResampler>) -> Self {
        Self {
            resampler,
            pending_discontinuity: false,
            holding: false,
            panicked: false,
        }
    }
}

pub(crate) struct PipelineParts {
    mic_input: SourceInput,
    system_input: SourceInput,
    aligner: Aligner,
    mic: Chain,
    system: Chain,
    mixer: Box<dyn AudioMixer>,
    mix: Chain,
    limited_samples: Arc<AtomicU64>,
    echo_stats: Option<Arc<EchoStatsSlot>>,
}

/// One session pipeline, with a resampler already chosen for each source.
pub(crate) struct PipelineStages {
    pub(crate) mic_resampler: Box<dyn DriftResampler>,
    pub(crate) system_resampler: Box<dyn DriftResampler>,
    pub(crate) aligner: Aligner,
    pub(crate) mic: Chain,
    pub(crate) system: Chain,
    pub(crate) mixer: Box<dyn AudioMixer>,
    pub(crate) mix: Chain,
    pub(crate) limited_samples: Arc<AtomicU64>,
    /// Present only when the echo canceller was built. System audio being off
    /// leaves this empty and is not a processing failure.
    pub(crate) echo_stats: Option<Arc<EchoStatsSlot>>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct PipelineDiagnostics {
    pub(crate) gaps: u64,
    pub(crate) discontinuities: u64,
    pub(crate) drift_adjustments: u64,
    pub(crate) limited_samples: u64,
    /// A sonora step was skipped after its constructor failed, a step returned
    /// `Err` or panicked, or a resampler panicked. Recorded once on the sidecar.
    pub(crate) audio_processing: bool,
    /// Latest canceller estimates. `None` when the step was not built, or when
    /// it never completed a capture. Missing fields inside the value are also
    /// not zeros.
    pub(crate) echo_stats: Option<EchoStats>,
}

pub(crate) struct Pipeline {
    mic_input: SourceInput,
    system_input: SourceInput,
    aligner: Aligner,
    mic: Chain,
    system: Chain,
    mixer: Box<dyn AudioMixer>,
    mix: Chain,
    mic_frame: AudioFrame,
    system_frame: AudioFrame,
    mix_frame: AudioFrame,
    output: Vec<f32>,
    limited_samples: Arc<AtomicU64>,
    echo_stats: Option<Arc<EchoStatsSlot>>,
}

impl Pipeline {
    pub(crate) fn new(parts: PipelineParts) -> Self {
        Self {
            mic_input: parts.mic_input,
            system_input: parts.system_input,
            aligner: parts.aligner,
            mic: parts.mic,
            system: parts.system,
            mixer: parts.mixer,
            mix: parts.mix,
            mic_frame: AudioFrame::silence(ChannelLayout::Mono, 0),
            system_frame: AudioFrame::silence(ChannelLayout::Stereo, 0),
            mix_frame: AudioFrame::silence(ChannelLayout::Stereo, 0),
            output: Vec::with_capacity(FRAME_LEN.saturating_mul(2)),
            limited_samples: parts.limited_samples,
            echo_stats: parts.echo_stats,
        }
    }

    pub(crate) fn from_stages(stages: PipelineStages) -> Self {
        Self::new(PipelineParts {
            mic_input: SourceInput::new(stages.mic_resampler),
            system_input: SourceInput::new(stages.system_resampler),
            aligner: stages.aligner,
            mic: stages.mic,
            system: stages.system,
            mixer: stages.mixer,
            mix: stages.mix,
            limited_samples: stages.limited_samples,
            echo_stats: stages.echo_stats,
        })
    }

    pub(crate) fn push_mic(&mut self, block: AudioBlock) {
        self.push_source(AudioSource::Mic, block);
    }

    pub(crate) fn push_system(&mut self, block: AudioBlock) {
        self.push_source(AudioSource::System, block);
    }

    /// Timeline error the aligner last published for this source.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn drift_error(&self, source: AudioSource) -> i64 {
        self.aligner.drift_error(source)
    }

    /// Flush this source's resampler into the aligner. A second call is a no-op
    /// until new audio arrives. Used when a capture stream ends mid-session,
    /// which used to flush inside the worker, and by [`Self::finish_input`].
    pub(crate) fn flush_source(&mut self, source: AudioSource) {
        let block = {
            let input = self.input_mut(source);
            if !input.holding {
                return;
            }
            input.holding = false;
            let samples = flush_resampler(input, source_name(source));
            let discontinuity = input.pending_discontinuity;
            input.pending_discontinuity = false;
            if samples.is_empty() {
                return;
            }
            AudioBlock {
                samples,
                timeline_frame: None,
                discontinuity,
            }
        };
        self.push_aligned(source, block);
    }

    /// Clears one source's resampler after its stream is reopened.
    ///
    /// Audio still held is placed on the timeline first. The aligner is left
    /// as it is.
    pub(crate) fn source_reopened(&mut self, source: AudioSource) {
        self.flush_source(source);
        let input = self.input_mut(source);
        // A panicked resampler stays in pass-through for the rest of the session.
        if !input.panicked {
            input.resampler.reset();
        }
        input.pending_discontinuity = false;
        input.holding = false;
    }

    /// Flushes both resamplers into the aligner. Call once on a normal stop,
    /// after both capture queues are empty and before the tail pull.
    pub(crate) fn finish_input(&mut self) {
        self.flush_source(AudioSource::Mic);
        self.flush_source(AudioSource::System);
    }

    /// Mic-chain processor names, in order. Taps are omitted.
    #[cfg(test)]
    pub(crate) fn mic_process_names(&self) -> Vec<&'static str> {
        self.mic.process_names()
    }

    pub(crate) fn diagnostics(&self) -> PipelineDiagnostics {
        let aligned = self.aligner.diagnostics();
        PipelineDiagnostics {
            gaps: aligned.gaps,
            discontinuities: aligned.discontinuities,
            drift_adjustments: aligned.drift_adjustments,
            limited_samples: self.limited_samples.load(Ordering::Relaxed),
            audio_processing: self.mic.processing_problem()
                || self.system.processing_problem()
                || self.mix.processing_problem()
                || self.mic_input.panicked
                || self.system_input.panicked,
            echo_stats: self.echo_stats.as_ref().and_then(|slot| slot.latest()),
        }
    }

    /// The next samples to write, or `None` while the aligner waits.
    ///
    /// A short tail is padded to [`FRAME_LEN`] for the chains. The returned
    /// slice is only the real `valid_frames * 2` interleaved samples, so the
    /// padding does not lengthen the WAV.
    pub(crate) fn pull(&mut self, now_frame: u64, tail: bool) -> Option<&[f32]> {
        let aligned = self.aligner.pull(now_frame, tail)?;
        let valid = usize::try_from(aligned.frames)
            .unwrap_or(FRAME_LEN)
            .min(FRAME_LEN);
        if valid == 0 {
            return None;
        }
        // `valid` is at most one frame, which `AudioFrame::reset` accepts.
        self.load_pair(&aligned.mic, &aligned.system, valid);
        run_mic_chain(&mut self.mic, &mut self.mic_frame, &self.system_frame);
        self.system.run(
            &mut self.system_frame,
            &ProcessContext {
                echo_reference: None,
            },
        );
        self.mixer
            .mix(&self.mic_frame, &self.system_frame, &mut self.mix_frame);
        self.mix.run(
            &mut self.mix_frame,
            &ProcessContext {
                echo_reference: None,
            },
        );
        let count = valid.saturating_mul(2).min(self.mix_frame.samples().len());
        self.output.clear();
        self.output
            .extend_from_slice(&self.mix_frame.samples()[..count]);
        Some(self.output.as_slice())
    }

    fn push_source(&mut self, source: AudioSource, block: AudioBlock) {
        let error = self.aligner.drift_error(source);
        let resampled = {
            let input = self.input_mut(source);
            if !input.panicked {
                input.resampler.set_drift_error(error);
            }
            if !block.samples.is_empty() {
                input.holding = true;
            }
            let samples = resample_or_passthrough(input, source_name(source), &block.samples);
            if samples.is_empty() {
                if block.discontinuity {
                    input.pending_discontinuity = true;
                }
                return;
            }
            let discontinuity = block.discontinuity || input.pending_discontinuity;
            input.pending_discontinuity = false;
            AudioBlock {
                samples,
                timeline_frame: block.timeline_frame,
                discontinuity,
            }
        };
        self.push_aligned(source, resampled);
    }

    fn push_aligned(&mut self, source: AudioSource, block: AudioBlock) {
        match source {
            AudioSource::Mic => self.aligner.push_mic(block),
            AudioSource::System => self.aligner.push_system(block),
        }
    }

    fn input_mut(&mut self, source: AudioSource) -> &mut SourceInput {
        match source {
            AudioSource::Mic => &mut self.mic_input,
            AudioSource::System => &mut self.system_input,
        }
    }

    fn load_pair(&mut self, mic: &[f32], system: &[f32], valid: usize) {
        let valid = valid.min(FRAME_LEN);
        if self.mic_frame.reset(ChannelLayout::Mono, valid, 0).is_err()
            || self
                .system_frame
                .reset(ChannelLayout::Stereo, valid, 0)
                .is_err()
            || self
                .mix_frame
                .reset(ChannelLayout::Stereo, valid, 0)
                .is_err()
        {
            tracing::error!("audio frame rejected the block length");
            return;
        }
        let mic_out = self.mic_frame.samples_mut();
        for (index, sample) in mic_out.iter_mut().enumerate().take(valid) {
            let left = mic.get(index.saturating_mul(2)).copied().unwrap_or(0.0);
            let right = mic
                .get(index.saturating_mul(2).saturating_add(1))
                .copied()
                .unwrap_or(0.0);
            *sample = (left + right) * 0.5;
        }
        let system_out = self.system_frame.samples_mut();
        let copy = valid
            .saturating_mul(2)
            .min(system.len())
            .min(system_out.len());
        system_out[..copy].copy_from_slice(&system[..copy]);
    }
}

fn source_name(source: AudioSource) -> &'static str {
    match source {
        AudioSource::Mic => "microphone",
        AudioSource::System => "system",
    }
}

/// Returns the resampler output. A panic switches that source to identity for
/// the rest of the session: this call and later `process` calls return the
/// input unchanged, and the resampler is not called again.
fn resample_or_passthrough(
    input: &mut SourceInput,
    source: &'static str,
    samples: &[f32],
) -> Vec<f32> {
    if input.panicked {
        return samples.to_vec();
    }
    let processed = {
        let resampler = &mut input.resampler;
        catch_unwind(AssertUnwindSafe(|| resampler.process(samples)))
    };
    match processed {
        Ok(output) => output,
        Err(_) => {
            input.panicked = true;
            tracing::error!(
                source,
                "audio resampler panicked; passing this source through"
            );
            samples.to_vec()
        }
    }
}

/// Returns held audio. A panic drops whatever the resampler still held — that
/// buffer is inside the object we will not call again — and later flushes
/// return empty. An empty `Ok` is not a failure; the aligner fills that hole.
fn flush_resampler(input: &mut SourceInput, source: &'static str) -> Vec<f32> {
    if input.panicked {
        return Vec::new();
    }
    let flushed = {
        let resampler = &mut input.resampler;
        catch_unwind(AssertUnwindSafe(|| resampler.flush()))
    };
    match flushed {
        Ok(output) => output,
        Err(_) => {
            input.panicked = true;
            tracing::error!(
                source,
                "audio resampler panicked; passing this source through"
            );
            Vec::new()
        }
    }
}

fn run_mic_chain(chain: &mut Chain, mic: &mut AudioFrame, system: &AudioFrame) {
    let context = ProcessContext {
        echo_reference: Some(system),
    };
    chain.run(mic, &context);
}

/// Returns its input unchanged. `flush` is empty, so exact-sample tests keep
/// the block the caller pushed.
struct PassThroughResampler;

impl DriftResampler for PassThroughResampler {
    fn set_drift_error(&mut self, _frames: i64) {}

    fn process(&mut self, interleaved: &[f32]) -> Vec<f32> {
        interleaved.to_vec()
    }

    fn flush(&mut self) -> Vec<f32> {
        Vec::new()
    }

    fn reset(&mut self) {}
}

fn build_harness(setup: PipelineSetup) -> Result<Pipeline, ProcessError> {
    let limited_samples = Arc::new(AtomicU64::new(0));
    let mic = Chain::new()
        .process(LinearGain::new(Arc::clone(&setup.gain)))
        .tap(PeakTap::new(Arc::clone(&setup.levels), MeterSlot::Mic));
    let system = Chain::new()
        .process(FixedGain::new(SYSTEM_GAIN)?)
        .tap(PeakTap::new(Arc::clone(&setup.levels), MeterSlot::System));
    let mix = Chain::new()
        .process(SoftLimiter::counting(Arc::clone(&limited_samples)))
        .tap(PeakTap::new(setup.levels, MeterSlot::Mixed));
    Ok(Pipeline::new(PipelineParts {
        mic_input: SourceInput::new(Box::new(PassThroughResampler)),
        system_input: SourceInput::new(Box::new(PassThroughResampler)),
        aligner: Aligner::new(setup.system_active),
        mic,
        system,
        mixer: Box::new(SumMixer),
        mix,
        limited_samples,
        echo_stats: None,
    }))
}

/// Recording harness and exact-sample tests. Each resampler returns its input
/// and flushes nothing. Microphone gain is [`LinearGain`], not sonora.
#[derive(Debug, Default)]
pub(crate) struct PassThroughPipelineFactory;

impl PipelineFactory for PassThroughPipelineFactory {
    fn build(&self, setup: PipelineSetup) -> Result<Pipeline, ProcessError> {
        build_harness(setup)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicI64, AtomicU32, AtomicU64, AtomicUsize, Ordering};
    use std::sync::Arc;

    use super::{
        PassThroughPipelineFactory, PassThroughResampler, Pipeline, PipelineParts, PipelineSetup,
        SourceInput,
    };
    use crate::domain::audio::AudioBlock;
    use crate::domain::audio_frame::{AudioFrame, ChannelLayout, FRAME_LEN};
    use crate::domain::chunk_plan::DRIFT_THRESHOLD_FRAMES;
    use crate::domain::gain::{soft_limit, SYSTEM_GAIN};
    use crate::infrastructure::audio::levels::{GainControl, LevelSnapshot};
    use crate::infrastructure::audio::resampler::StereoResampler;
    use crate::services::audio_pipeline::{Aligner, AudioSource, Chain, FixedGain, SumMixer};
    use crate::services::ports::{
        AudioProcessor, DriftResampler, PipelineFactory, ProcessContext, ProcessError,
    };

    struct RecordReferencePeak {
        seen: Arc<AtomicU32>,
    }

    impl RecordReferencePeak {
        fn new(seen: Arc<AtomicU32>) -> Self {
            Self { seen }
        }
    }

    impl AudioProcessor for RecordReferencePeak {
        fn name(&self) -> &'static str {
            "record-reference"
        }

        fn process(
            &mut self,
            _frame: &mut AudioFrame,
            context: &ProcessContext<'_>,
        ) -> Result<(), ProcessError> {
            let peak = context.echo_reference.map(reference_peak).unwrap_or(0.0);
            self.seen.store(peak.to_bits(), Ordering::Relaxed);
            Ok(())
        }
    }

    fn reference_peak(frame: &AudioFrame) -> f32 {
        let count = match frame.layout() {
            ChannelLayout::Mono => frame.valid_frames(),
            ChannelLayout::Stereo => frame.valid_frames().saturating_mul(2),
        };
        let mut loudest = 0.0_f32;
        for sample in frame.samples().iter().take(count) {
            if sample.is_finite() {
                loudest = loudest.max(sample.abs());
            }
        }
        loudest
    }

    /// Test double for the input stage. `hold_calls` drops that many outputs.
    /// `retain` keeps samples for `flush` instead of passing them through.
    struct ScriptedResampler {
        last_error: Arc<AtomicI64>,
        resets: Arc<AtomicUsize>,
        flush_calls: Arc<AtomicUsize>,
        hold_calls: u32,
        seen_calls: u32,
        held: Vec<f32>,
        retain: bool,
    }

    impl ScriptedResampler {
        fn observe(last_error: Arc<AtomicI64>) -> Self {
            Self {
                last_error,
                resets: Arc::new(AtomicUsize::new(0)),
                flush_calls: Arc::new(AtomicUsize::new(0)),
                hold_calls: 0,
                seen_calls: 0,
                held: Vec::new(),
                retain: false,
            }
        }

        fn hold_first(calls: u32) -> Self {
            Self {
                hold_calls: calls,
                ..Self::observe(Arc::new(AtomicI64::new(0)))
            }
        }

        fn retain_until_flush(flush_calls: Arc<AtomicUsize>) -> Self {
            Self {
                flush_calls,
                retain: true,
                ..Self::observe(Arc::new(AtomicI64::new(0)))
            }
        }

        fn count_resets(resets: Arc<AtomicUsize>) -> Self {
            Self {
                resets,
                ..Self::observe(Arc::new(AtomicI64::new(0)))
            }
        }
    }

    impl DriftResampler for ScriptedResampler {
        fn set_drift_error(&mut self, frames: i64) {
            self.last_error.store(frames, Ordering::Relaxed);
        }

        fn process(&mut self, interleaved: &[f32]) -> Vec<f32> {
            self.seen_calls = self.seen_calls.saturating_add(1);
            if self.seen_calls <= self.hold_calls {
                return Vec::new();
            }
            if self.retain {
                self.held.extend_from_slice(interleaved);
                return Vec::new();
            }
            interleaved.to_vec()
        }

        fn flush(&mut self) -> Vec<f32> {
            self.flush_calls.fetch_add(1, Ordering::Relaxed);
            std::mem::take(&mut self.held)
        }

        fn reset(&mut self) {
            self.resets.fetch_add(1, Ordering::Relaxed);
            self.held.clear();
            self.seen_calls = 0;
        }
    }

    fn pipeline_with(mic: Chain, system: Chain) -> Pipeline {
        pipeline_with_resamplers(
            Box::new(PassThroughResampler),
            Box::new(PassThroughResampler),
            true,
            mic,
            system,
        )
    }

    fn pipeline_with_resamplers(
        mic_resampler: Box<dyn DriftResampler>,
        system_resampler: Box<dyn DriftResampler>,
        system_active: bool,
        mic: Chain,
        system: Chain,
    ) -> Pipeline {
        Pipeline::new(PipelineParts {
            mic_input: SourceInput::new(mic_resampler),
            system_input: SourceInput::new(system_resampler),
            aligner: Aligner::new(system_active),
            mic,
            system,
            mixer: Box::new(SumMixer),
            mix: Chain::new(),
            limited_samples: Arc::new(AtomicU64::new(0)),
            echo_stats: None,
        })
    }

    fn test_pipeline(system_active: bool, gain: f32) -> (Pipeline, Arc<LevelSnapshot>) {
        let levels = Arc::new(LevelSnapshot::new());
        let pipeline = PassThroughPipelineFactory
            .build(PipelineSetup {
                system_active,
                gain: Arc::new(GainControl::new(gain)),
                levels: Arc::clone(&levels),
            })
            .expect("pipeline");
        (pipeline, levels)
    }

    fn tone(start: Option<u64>, frames: usize, sample: f32) -> AudioBlock {
        AudioBlock {
            samples: vec![sample; frames.saturating_mul(2)],
            timeline_frame: start,
            discontinuity: false,
        }
    }

    fn drain(pipeline: &mut Pipeline) {
        while pipeline.pull(0, false).is_some() {}
    }

    #[test]
    fn echo_reference_is_system_audio_before_gain() {
        // Arrange
        let seen = Arc::new(AtomicU32::new(0));
        let mut pipeline = pipeline_with(
            Chain::new().process(RecordReferencePeak::new(Arc::clone(&seen))),
            Chain::new().process(FixedGain::new(0.5).expect("gain")),
        );
        pipeline.push_mic(tone(Some(0), FRAME_LEN, 0.1));
        pipeline.push_system(tone(Some(0), FRAME_LEN, 0.4));

        // Act
        let output = pipeline.pull(0, false).expect("frame");

        // Assert
        let peak = f32::from_bits(seen.load(Ordering::Relaxed));
        assert!((peak - 0.4).abs() < 0.0001);
        assert!((output[0] - 0.3).abs() < 0.0001);
        assert!((output[1] - 0.3).abs() < 0.0001);
    }

    #[test]
    fn mono_mic_is_duplicated_to_both_channels() {
        // Arrange
        let mut samples = vec![0.0; FRAME_LEN * 2];
        samples[0] = 0.2;
        samples[1] = 0.6;
        let (mut pipeline, _) = test_pipeline(false, 1.0);
        pipeline.push_mic(AudioBlock {
            samples,
            timeline_frame: Some(0),
            discontinuity: false,
        });

        // Act
        let mixed = pipeline.pull(0, false).expect("frame");

        // Assert
        assert_eq!(mixed.len(), FRAME_LEN * 2);
        assert!((mixed[0] - 0.4).abs() < 0.0001);
        assert!((mixed[1] - 0.4).abs() < 0.0001);
        assert_eq!(mixed[2], 0.0);
        assert_eq!(mixed[3], 0.0);
    }

    #[test]
    fn tail_shorter_than_a_frame_keeps_its_length() {
        // Arrange
        let (mut pipeline, _) = test_pipeline(false, 1.0);
        pipeline.push_mic(tone(Some(0), 100, 0.25));

        // Act
        let mixed = pipeline.pull(0, true).expect("tail").to_vec();
        let again = pipeline.pull(0, true);

        // Assert
        assert_eq!(mixed.len(), 200);
        assert!((mixed[0] - 0.25).abs() < 0.0001);
        assert!((mixed[1] - 0.25).abs() < 0.0001);
        assert!(again.is_none());
    }

    #[test]
    fn hot_sums_enter_the_limiter() {
        // Arrange
        let (mut pipeline, levels) = test_pipeline(true, 1.0);

        // Act
        pipeline.push_mic(tone(Some(0), FRAME_LEN, 1.0));
        pipeline.push_system(tone(Some(0), FRAME_LEN, 1.0));
        let mixed = pipeline.pull(0, false).expect("frame").to_vec();
        let expected = soft_limit(1.0 + SYSTEM_GAIN);

        // Assert
        assert!(!mixed.is_empty());
        assert!(mixed[0].abs() < 1.0);
        assert!((mixed[0] - expected).abs() < 0.0001);
        assert!((mixed[1] - expected).abs() < 0.0001);
        assert!(pipeline.diagnostics().limited_samples > 0);
        assert!((levels.take_mic() - 1.0).abs() < 0.0001);
        assert!((levels.take_system() - SYSTEM_GAIN).abs() < 0.0001);
        assert!((levels.take_mixed() - expected).abs() < 0.0001);
    }

    #[test]
    fn mic_meter_reads_audio_after_the_slider() {
        // Arrange
        let (mut pipeline, levels) = test_pipeline(false, 2.0);

        // Act
        pipeline.push_mic(tone(Some(0), FRAME_LEN, 0.2));
        let mixed = pipeline.pull(0, false).expect("frame");

        // Assert
        assert!((levels.take_mic() - 0.4).abs() < 0.0001);
        assert!((mixed[0] - 0.4).abs() < 0.0001);
        assert!((mixed[1] - 0.4).abs() < 0.0001);
    }

    #[test]
    fn drift_error_from_aligner_reaches_the_resampler() {
        // Arrange
        let seen = Arc::new(AtomicI64::new(i64::MIN));
        let mut pipeline = pipeline_with_resamplers(
            Box::new(ScriptedResampler::observe(Arc::clone(&seen))),
            Box::new(PassThroughResampler),
            false,
            Chain::new(),
            Chain::new(),
        );

        // Act
        pipeline.push_mic(tone(Some(0), 480, 0.1));
        pipeline.push_mic(tone(Some(500), 480, 0.1));
        pipeline.push_mic(tone(Some(980), 480, 0.1));

        // Assert
        assert_eq!(seen.load(Ordering::Relaxed), 20);
        assert_eq!(pipeline.drift_error(AudioSource::Mic), 20);
    }

    #[test]
    fn reopened_source_resets_its_resampler_only() {
        // Arrange
        let mic_resets = Arc::new(AtomicUsize::new(0));
        let system_resets = Arc::new(AtomicUsize::new(0));
        let mut pipeline = pipeline_with_resamplers(
            Box::new(ScriptedResampler::count_resets(Arc::clone(&mic_resets))),
            Box::new(ScriptedResampler::count_resets(Arc::clone(&system_resets))),
            true,
            Chain::new(),
            Chain::new(),
        );
        pipeline.push_mic(tone(Some(0), 480, 0.2));
        pipeline.push_system(tone(Some(0), 480, 0.2));

        // Act
        pipeline.source_reopened(AudioSource::Mic);
        pipeline.push_mic(tone(Some(480), 480, 0.2));

        // Assert
        assert_eq!(mic_resets.load(Ordering::Relaxed), 1);
        assert_eq!(system_resets.load(Ordering::Relaxed), 0);
        assert_eq!(pipeline.diagnostics().gaps, 0);
    }

    #[test]
    fn discontinuity_survives_a_resampler_call_with_no_output() {
        // Arrange
        let mut pipeline = pipeline_with_resamplers(
            Box::new(ScriptedResampler::hold_first(1)),
            Box::new(PassThroughResampler),
            false,
            Chain::new(),
            Chain::new(),
        );
        let mut first = tone(Some(0), FRAME_LEN, 0.2);
        first.discontinuity = true;

        // Act
        pipeline.push_mic(first);
        let after_empty = pipeline.diagnostics().discontinuities;
        pipeline.push_mic(AudioBlock {
            samples: vec![0.4; FRAME_LEN * 2],
            timeline_frame: None,
            discontinuity: false,
        });
        let carried = pipeline.diagnostics().discontinuities;
        let mixed = pipeline.pull(0, false).expect("frame").to_vec();

        // Assert
        assert_eq!(after_empty, 0);
        assert_eq!(carried, 1);
        assert!((mixed[0] - 0.4).abs() < 0.0001);
        assert!((mixed[1] - 0.4).abs() < 0.0001);
        assert!(!pipeline.diagnostics().audio_processing);
    }

    struct PanicResampler {
        process_calls: Arc<AtomicUsize>,
        flush_calls: Arc<AtomicUsize>,
        panic_on_process: bool,
        panic_on_flush: bool,
    }

    impl PanicResampler {
        fn on_process(process_calls: Arc<AtomicUsize>) -> Self {
            Self {
                process_calls,
                flush_calls: Arc::new(AtomicUsize::new(0)),
                panic_on_process: true,
                panic_on_flush: true,
            }
        }

        fn on_flush(process_calls: Arc<AtomicUsize>, flush_calls: Arc<AtomicUsize>) -> Self {
            Self {
                process_calls,
                flush_calls,
                panic_on_process: false,
                panic_on_flush: true,
            }
        }
    }

    impl DriftResampler for PanicResampler {
        fn set_drift_error(&mut self, _frames: i64) {}

        fn process(&mut self, interleaved: &[f32]) -> Vec<f32> {
            self.process_calls.fetch_add(1, Ordering::SeqCst);
            if self.panic_on_process {
                panic!("resampler process panicked");
            }
            interleaved.to_vec()
        }

        fn flush(&mut self) -> Vec<f32> {
            self.flush_calls.fetch_add(1, Ordering::SeqCst);
            if self.panic_on_flush {
                panic!("resampler flush panicked");
            }
            Vec::new()
        }

        fn reset(&mut self) {}
    }

    struct ScalingResampler {
        gain: f32,
        calls: Arc<AtomicUsize>,
        drift: Arc<AtomicI64>,
        flushes: Arc<AtomicUsize>,
    }

    impl ScalingResampler {
        fn new(gain: f32, calls: Arc<AtomicUsize>, drift: Arc<AtomicI64>) -> Self {
            Self {
                gain,
                calls,
                drift,
                flushes: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    impl DriftResampler for ScalingResampler {
        fn set_drift_error(&mut self, frames: i64) {
            self.drift.store(frames, Ordering::Relaxed);
        }

        fn process(&mut self, interleaved: &[f32]) -> Vec<f32> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            interleaved
                .iter()
                .map(|sample| sample * self.gain)
                .collect()
        }

        fn flush(&mut self) -> Vec<f32> {
            self.flushes.fetch_add(1, Ordering::Relaxed);
            Vec::new()
        }

        fn reset(&mut self) {}
    }

    #[test]
    fn resampler_panic_falls_back_to_pass_through_and_the_other_source_still_resamples() {
        // Arrange
        let mic_calls = Arc::new(AtomicUsize::new(0));
        let system_calls = Arc::new(AtomicUsize::new(0));
        let system_drift = Arc::new(AtomicI64::new(i64::MIN));
        let mut pipeline = pipeline_with_resamplers(
            Box::new(PanicResampler::on_process(Arc::clone(&mic_calls))),
            Box::new(ScalingResampler::new(
                0.5,
                Arc::clone(&system_calls),
                Arc::clone(&system_drift),
            )),
            true,
            Chain::new(),
            Chain::new(),
        );

        // Act
        pipeline.push_mic(tone(Some(0), FRAME_LEN, 0.2));
        pipeline.push_system(tone(Some(0), FRAME_LEN, 0.6));
        let mixed = pipeline.pull(0, false).expect("frame").to_vec();
        pipeline.push_mic(tone(Some(FRAME_LEN as u64), FRAME_LEN, 0.2));
        pipeline.push_system(tone(Some(FRAME_LEN as u64), FRAME_LEN, 0.6));

        // Assert
        // Mic stays 0.2. System is scaled from 0.6 to 0.3. Sum is 0.5.
        assert!((mixed[0] - 0.5).abs() < 0.0001);
        assert!((mixed[1] - 0.5).abs() < 0.0001);
        assert_eq!(mic_calls.load(Ordering::SeqCst), 1);
        assert_eq!(system_calls.load(Ordering::SeqCst), 2);
        assert_ne!(system_drift.load(Ordering::Relaxed), i64::MIN);
        assert!(pipeline.diagnostics().audio_processing);
    }

    #[test]
    fn resampler_flush_panic_passes_later_audio_through() {
        // Arrange
        let mic_process = Arc::new(AtomicUsize::new(0));
        let mic_flush = Arc::new(AtomicUsize::new(0));
        let system_calls = Arc::new(AtomicUsize::new(0));
        let system_drift = Arc::new(AtomicI64::new(i64::MIN));
        let mut pipeline = pipeline_with_resamplers(
            Box::new(PanicResampler::on_flush(
                Arc::clone(&mic_process),
                Arc::clone(&mic_flush),
            )),
            Box::new(ScalingResampler::new(
                0.5,
                Arc::clone(&system_calls),
                Arc::clone(&system_drift),
            )),
            true,
            Chain::new(),
            Chain::new(),
        );
        pipeline.push_mic(tone(Some(0), FRAME_LEN, 0.2));
        pipeline.push_system(tone(Some(0), FRAME_LEN, 0.6));
        let _ = pipeline.pull(0, false).expect("first frame");

        // Act
        pipeline.finish_input();
        let during_flush = pipeline.diagnostics().audio_processing;
        pipeline.push_mic(tone(Some(FRAME_LEN as u64), FRAME_LEN, 0.2));
        pipeline.push_system(tone(Some(FRAME_LEN as u64), FRAME_LEN, 0.6));
        let mixed = pipeline
            .pull(FRAME_LEN as u64, false)
            .expect("later frame")
            .to_vec();

        // Assert
        assert!(during_flush);
        assert_eq!(mic_flush.load(Ordering::SeqCst), 1);
        assert_eq!(mic_process.load(Ordering::SeqCst), 1);
        assert_eq!(system_calls.load(Ordering::SeqCst), 2);
        assert_ne!(system_drift.load(Ordering::Relaxed), i64::MIN);
        assert!((mixed[0] - 0.5).abs() < 0.0001);
        assert!((mixed[1] - 0.5).abs() < 0.0001);
    }

    struct FailingStep;

    impl AudioProcessor for FailingStep {
        fn name(&self) -> &'static str {
            "failing-step"
        }

        fn process(
            &mut self,
            _frame: &mut AudioFrame,
            _context: &ProcessContext<'_>,
        ) -> Result<(), ProcessError> {
            Err(ProcessError::new(self.name(), "step failed"))
        }
    }

    #[test]
    fn failed_step_marks_audio_processing() {
        // Arrange
        let mut pipeline = pipeline_with(Chain::new().process(FailingStep), Chain::new());
        pipeline.push_mic(tone(Some(0), FRAME_LEN, 0.2));
        pipeline.push_system(tone(Some(0), FRAME_LEN, 0.1));

        // Act
        let mixed = pipeline.pull(0, false).expect("frame").to_vec();
        pipeline.pull(0, false);

        // Assert
        assert!((mixed[0] - 0.3).abs() < 0.0001);
        assert!(pipeline.diagnostics().audio_processing);
        assert!(pipeline.diagnostics().echo_stats.is_none());
    }

    #[test]
    fn finish_input_flushes_held_audio_into_the_aligner() {
        // Arrange
        let flush_calls = Arc::new(AtomicUsize::new(0));
        let mut pipeline = pipeline_with_resamplers(
            Box::new(ScriptedResampler::retain_until_flush(Arc::clone(
                &flush_calls,
            ))),
            Box::new(PassThroughResampler),
            false,
            Chain::new(),
            Chain::new(),
        );
        pipeline.push_mic(tone(Some(0), 100, 0.25));
        assert!(pipeline.pull(0, true).is_none());

        // Act
        pipeline.finish_input();
        let mixed = pipeline.pull(0, true).expect("flushed tail").to_vec();
        pipeline.finish_input();
        let again = pipeline.pull(0, true);

        // Assert
        assert_eq!(mixed.len(), 200);
        assert!((mixed[0] - 0.25).abs() < 0.0001);
        assert!((mixed[1] - 0.25).abs() < 0.0001);
        assert!(again.is_none());
        assert_eq!(flush_calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn hundred_ppm_publishes_drift_error_past_threshold() {
        // Arrange
        let frames = 4_800_usize;
        let packets = 2_200_u64;
        let fast_seen = Arc::new(AtomicI64::new(0));
        let slow_seen = Arc::new(AtomicI64::new(0));
        let mut fast = pipeline_with_resamplers(
            Box::new(ScriptedResampler::observe(Arc::clone(&fast_seen))),
            Box::new(PassThroughResampler),
            false,
            Chain::new(),
            Chain::new(),
        );
        let mut slow = pipeline_with_resamplers(
            Box::new(ScriptedResampler::observe(Arc::clone(&slow_seen))),
            Box::new(PassThroughResampler),
            false,
            Chain::new(),
            Chain::new(),
        );

        // Act
        for index in 0..packets {
            let fast_start = index * frames as u64 * 10_000 / 10_001;
            let slow_start = index * frames as u64 * 10_001 / 10_000;
            fast.push_mic(tone(Some(fast_start), frames, 0.1));
            slow.push_mic(tone(Some(slow_start), frames, 0.1));
            drain(&mut fast);
            drain(&mut slow);
        }

        // Assert — the resampler observed the error, and the aligner still publishes it.
        let fast_published = fast.drift_error(AudioSource::Mic);
        let slow_published = slow.drift_error(AudioSource::Mic);
        let fast_observed = fast_seen.load(Ordering::Relaxed);
        let slow_observed = slow_seen.load(Ordering::Relaxed);
        assert!(
            fast_published < -(DRIFT_THRESHOLD_FRAMES as i64),
            "fast published={fast_published}"
        );
        assert!(
            slow_published > DRIFT_THRESHOLD_FRAMES as i64,
            "slow published={slow_published}"
        );
        assert!(
            fast_observed < -(DRIFT_THRESHOLD_FRAMES as i64),
            "fast observed={fast_observed}"
        );
        assert!(
            slow_observed > DRIFT_THRESHOLD_FRAMES as i64,
            "slow observed={slow_observed}"
        );
        assert!(fast.diagnostics().drift_adjustments > 0);
        assert!(slow.diagnostics().drift_adjustments > 0);
    }

    #[test]
    fn drift_loop_settles_under_steady_timeline_error() {
        // Arrange
        let frames = 480_usize;
        let packets = 3_000_u64;
        let warmup = 8_u64;
        let chunk = vec![0.05_f32; frames * 2];
        let mut open = Aligner::new(false);
        let mut closed = Aligner::new(false);
        let mut resampler = StereoResampler::new(48_000, 48_000);
        let steady = vec![0.05_f32; frames * 8];
        let mut stretched = StereoResampler::new(48_000, 48_000);
        let mut compressed = StereoResampler::new(48_000, 48_000);
        stretched.set_drift_error(2_000);
        compressed.set_drift_error(-2_000);

        // Act
        let mut slow_out = stretched.process(&steady);
        slow_out.extend(stretched.flush());
        let mut fast_out = compressed.process(&steady);
        fast_out.extend(compressed.flush());
        for _ in 0..warmup {
            let output = resampler.process(&chunk);
            if !output.is_empty() {
                closed.push_mic(AudioBlock {
                    samples: output,
                    timeline_frame: None,
                    discontinuity: false,
                });
            }
        }
        let base = warmup * frames as u64;
        for index in 0..packets {
            let start = (base + index * frames as u64) * 1_001 / 1_000;
            let open_start = index * frames as u64 * 1_001 / 1_000;
            open.push_mic(tone(Some(open_start), frames, 0.05));
            let error = closed.drift_error(AudioSource::Mic);
            resampler.set_drift_error(error);
            let output = resampler.process(&chunk);
            if !output.is_empty() {
                closed.push_mic(AudioBlock {
                    samples: output,
                    timeline_frame: Some(start),
                    discontinuity: false,
                });
            }
            while open.pull(0, false).is_some() {}
            while closed.pull(0, false).is_some() {}
        }
        let open_error = open.drift_error(AudioSource::Mic);
        let closed_error = closed.drift_error(AudioSource::Mic);

        // Assert
        assert!(
            slow_out.len() > fast_out.len(),
            "steady error did not change length: slow={} fast={}",
            slow_out.len(),
            fast_out.len()
        );
        assert!(
            open_error > DRIFT_THRESHOLD_FRAMES as i64,
            "open={open_error}"
        );
        assert_eq!(closed.diagnostics().gaps, 0, "correction opened a hole");
        assert!(
            closed_error.abs() < open_error.abs() / 2,
            "open={open_error} closed={closed_error}"
        );
    }
}
