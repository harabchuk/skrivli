//! Microphone echo cancellation and gain, on two sonora instances.
//!
//! Production code imports `sonora` only in this file. The spike module may
//! import it under `cfg(test)`. The gain instance stays capture-level
//! adjustment only: nothing feeds it a render stream or enables an echo
//! canceller. Echo removal is [`SonoraEchoCanceller`], and only when system
//! audio was requested.

use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use sonora::config::{CaptureLevelAdjustment, EchoCanceller};
use sonora::{AudioProcessing, Config, Error as SonoraError, StreamConfig};

use crate::domain::audio::INTERNAL_SAMPLE_RATE;
use crate::domain::audio_frame::{AudioFrame, ChannelLayout, FRAME_LEN};
use crate::domain::gain::SYSTEM_GAIN;
use crate::infrastructure::audio::levels::{GainControl, MeterSlot, PeakTap};
use crate::infrastructure::audio::resampler::StereoResampler;
use crate::services::audio_pipeline::{
    Aligner, Chain, EchoStats, EchoStatsSlot, FixedGain, Pipeline, PipelineSetup, PipelineStages,
    SoftLimiter, SumMixer,
};
use crate::services::ports::{AudioProcessor, PipelineFactory, ProcessContext, ProcessError};

const STEP_NAME: &str = "sonora-mic-gain";
const AEC_STEP_NAME: &str = "sonora-echo-canceller";
/// Outside the slider range, so the first `process` always sends the slider.
const GAIN_UNSET: f32 = -1.0;
/// D-13: hint 0 covered synthetic echo at 20, 100, and 300 ms. The canceller
/// still adds about 9 ms. This step does not delay the system path to match.
const STREAM_DELAY_MS: i32 = 0;

/// Capped sonora estimates. Not full-band energy ERLE.
///
/// [`SonoraEchoCanceller`] copies these into [`EchoStatsSlot`] after each
/// successful capture. The session logs the slot once at the end. This type
/// does not store samples.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct EchoCancellerStats {
    pub(crate) echo_return_loss_enhancement: Option<f64>,
    pub(crate) delay_ms: Option<i32>,
}

/// Removes speaker echo from the raw microphone.
///
/// Capture is 48 kHz mono and render is 48 kHz stereo, with the default echo
/// canceller and no custom internal rate. Each frame feeds render, then
/// [`AudioProcessing::set_stream_delay_ms`]`(0)`, then capture. Scratch
/// buffers are allocated in [`Self::new`]; `process` does not allocate.
pub(crate) struct SonoraEchoCanceller {
    apm: AudioProcessing,
    render_left: Vec<f32>,
    render_right: Vec<f32>,
    render_left_out: Vec<f32>,
    render_right_out: Vec<f32>,
    capture_out: Vec<f32>,
    stats: Arc<EchoStatsSlot>,
}

impl SonoraEchoCanceller {
    /// Echo canceller only. Gain, `pre_amplifier`, and `gain_controller2` stay off.
    pub(crate) fn new() -> Result<Self, ProcessError> {
        let apm = AudioProcessing::builder()
            .config(Config {
                echo_canceller: Some(EchoCanceller::default()),
                ..Config::default()
            })
            .capture_config(StreamConfig::new(INTERNAL_SAMPLE_RATE, 1))
            .render_config(StreamConfig::new(INTERNAL_SAMPLE_RATE, 2))
            .build();
        let config = apm.config();
        if config.echo_canceller.is_none()
            || config.capture_level_adjustment.is_some()
            || config.pre_amplifier.is_some()
            || config.gain_controller2.is_some()
        {
            return Err(ProcessError::new(
                AEC_STEP_NAME,
                "echo canceller was not configured",
            ));
        }
        Ok(Self {
            apm,
            render_left: vec![0.0; FRAME_LEN],
            render_right: vec![0.0; FRAME_LEN],
            render_left_out: vec![0.0; FRAME_LEN],
            render_right_out: vec![0.0; FRAME_LEN],
            capture_out: vec![0.0; FRAME_LEN],
            stats: Arc::new(EchoStatsSlot::new()),
        })
    }

    /// Shared with the pipeline so session end can read the latest estimates
    /// without downcasting this step.
    pub(crate) fn stats_slot(&self) -> Arc<EchoStatsSlot> {
        Arc::clone(&self.stats)
    }

    /// `echo_return_loss_enhancement` and `delay_ms` from the current statistics.
    pub(crate) fn echo_statistics(&self) -> EchoCancellerStats {
        let stats = self.apm.statistics();
        EchoCancellerStats {
            echo_return_loss_enhancement: stats.echo_return_loss_enhancement,
            delay_ms: stats.delay_ms,
        }
    }

    fn cancel(&mut self, mic: &[f32], reference: &[f32]) -> Result<(), SonoraError> {
        let SonoraEchoCanceller {
            apm,
            render_left,
            render_right,
            render_left_out,
            render_right_out,
            capture_out,
            ..
        } = self;
        // Full 480-frame buffer, including silence padding. A shorter slice panics.
        // Length was checked: exactly FRAME_LEN stereo pairs, matching the scratch.
        let (pairs, rest) = reference.as_chunks::<2>();
        debug_assert!(rest.is_empty());
        debug_assert_eq!(pairs.len(), render_left.len());
        for (pair, (left, right)) in pairs
            .iter()
            .zip(render_left.iter_mut().zip(render_right.iter_mut()))
        {
            *left = pair[0];
            *right = pair[1];
        }
        apm.process_render_f32(
            &[render_left, render_right],
            &mut [
                render_left_out.as_mut_slice(),
                render_right_out.as_mut_slice(),
            ],
        )?;
        apm.set_stream_delay_ms(STREAM_DELAY_MS)?;
        apm.process_capture_f32(&[mic], &mut [capture_out.as_mut_slice()])?;
        Ok(())
    }
}

impl AudioProcessor for SonoraEchoCanceller {
    fn name(&self) -> &'static str {
        AEC_STEP_NAME
    }

    fn process(
        &mut self,
        frame: &mut AudioFrame,
        context: &ProcessContext<'_>,
    ) -> Result<(), ProcessError> {
        let Some(reference) = context.echo_reference else {
            return Ok(());
        };
        if frame.layout() != ChannelLayout::Mono || frame.samples().len() != FRAME_LEN {
            return Err(ProcessError::new(
                AEC_STEP_NAME,
                "microphone frame is not 480 mono samples",
            ));
        }
        if reference.layout() != ChannelLayout::Stereo || reference.samples().len() != FRAME_LEN * 2
        {
            return Err(ProcessError::new(
                AEC_STEP_NAME,
                "echo reference is not a 480-frame stereo buffer",
            ));
        }
        match self.cancel(frame.samples(), reference.samples()) {
            Ok(()) => {
                frame.samples_mut().copy_from_slice(&self.capture_out);
                self.publish_stats();
                Ok(())
            }
            Err(error) => Err(process_error(AEC_STEP_NAME, error)),
        }
    }
}

impl SonoraEchoCanceller {
    fn publish_stats(&self) {
        let stats = self.echo_statistics();
        self.stats.publish(EchoStats {
            echo_return_loss_enhancement: stats.echo_return_loss_enhancement,
            delay_ms: stats.delay_ms,
        });
    }
}

fn with_echo_canceller(
    chain: Chain,
    step: Result<SonoraEchoCanceller, ProcessError>,
) -> (Chain, Option<Arc<EchoStatsSlot>>) {
    match step {
        Ok(step) => {
            let stats = step.stats_slot();
            (chain.process(step), Some(stats))
        }
        Err(error) => {
            tracing::error!(
                step = AEC_STEP_NAME,
                error = %error,
                "echo canceller step was not created"
            );
            (chain.construction_failed(), None)
        }
    }
}

/// Manual microphone gain. One capture-only sonora instance.
pub(crate) struct SonoraMicGain {
    apm: AudioProcessing,
    gain: Arc<GainControl>,
    /// Last slider value passed to `set_capture_post_gain`.
    applied: f32,
    /// One capture channel. Reused so `process` does not allocate.
    capture_out: Vec<f32>,
}

impl SonoraMicGain {
    /// Capture is 48 kHz mono with `capture_level_adjustment` on.
    ///
    /// The builder always stores a render stream, so render is set to 48 kHz
    /// stereo and never fed. Echo cancellation, `pre_amplifier`, and
    /// `gain_controller2` stay off.
    pub(crate) fn new(gain: Arc<GainControl>) -> Result<Self, ProcessError> {
        let apm = AudioProcessing::builder()
            .config(Config {
                capture_level_adjustment: Some(CaptureLevelAdjustment::default()),
                ..Config::default()
            })
            .capture_config(StreamConfig::new(INTERNAL_SAMPLE_RATE, 1))
            .render_config(StreamConfig::new(INTERNAL_SAMPLE_RATE, 2))
            .build();
        let config = apm.config();
        if config.capture_level_adjustment.is_none()
            || config.echo_canceller.is_some()
            || config.pre_amplifier.is_some()
            || config.gain_controller2.is_some()
        {
            return Err(ProcessError::new(
                STEP_NAME,
                "microphone gain was not configured",
            ));
        }
        Ok(Self {
            apm,
            gain,
            applied: GAIN_UNSET,
            capture_out: vec![0.0; FRAME_LEN],
        })
    }
}

impl AudioProcessor for SonoraMicGain {
    fn name(&self) -> &'static str {
        STEP_NAME
    }

    fn process(
        &mut self,
        frame: &mut AudioFrame,
        _context: &ProcessContext<'_>,
    ) -> Result<(), ProcessError> {
        let slider = self.gain.slider();
        if slider != self.applied {
            self.apm.set_capture_post_gain(slider);
            self.applied = slider;
        }
        if frame.layout() != ChannelLayout::Mono || frame.samples().len() != FRAME_LEN {
            return Err(ProcessError::new(
                STEP_NAME,
                "microphone frame is not 480 mono samples",
            ));
        }
        // Mono is already one channel. The scratch is that channel's output.
        // A short slice panics inside sonora, so this only runs for 480 samples.
        let result = {
            let input = frame.samples();
            let SonoraMicGain {
                apm, capture_out, ..
            } = self;
            apm.process_capture_f32(&[input], &mut [&mut capture_out[..]])
        };
        match result {
            Ok(()) => {
                frame.samples_mut().copy_from_slice(&self.capture_out);
                Ok(())
            }
            Err(error) => Err(process_error(STEP_NAME, error)),
        }
    }
}

fn process_error(step: &'static str, error: SonoraError) -> ProcessError {
    let message = match error {
        SonoraError::InvalidSampleRate { .. } => "sonora rejected the sample rate",
        SonoraError::InvalidChannelCount { .. } => "sonora rejected the channel count",
        SonoraError::ChannelMismatch { .. } => "sonora rejected the channel layout",
        SonoraError::StreamParameterClamped => "sonora clamped a stream parameter",
    };
    ProcessError::new(step, message)
}

fn with_mic_gain(chain: Chain, gain: Result<SonoraMicGain, ProcessError>) -> Chain {
    match gain {
        Ok(step) => chain.process(step),
        Err(error) => {
            tracing::error!(
                step = STEP_NAME,
                error = %error,
                "microphone gain step was not created"
            );
            chain.construction_failed()
        }
    }
}

/// Production pipeline. Same shape as the harness factory, with sonora gain
/// in place of `LinearGain` and a Rubato resampler on each source.
///
/// Mic chain, when system audio was requested: [`SonoraEchoCanceller`], then
/// [`SonoraMicGain`], then the mic [`PeakTap`]. The canceller sees the raw
/// microphone. Its reference is the system frame before [`FixedGain`].
/// When system audio was not requested, the mic chain starts at
/// [`SonoraMicGain`].
/// System chain: [`FixedGain`] of [`SYSTEM_GAIN`] then the system tap.
/// Mix chain: [`SoftLimiter`] then the mixed tap.
#[derive(Debug, Default)]
pub(crate) struct SonoraPipelineFactory;

impl PipelineFactory for SonoraPipelineFactory {
    fn build(&self, setup: PipelineSetup) -> Result<Pipeline, ProcessError> {
        let limited_samples = Arc::new(AtomicU64::new(0));
        let (mic, echo_stats) = if setup.system_active {
            with_echo_canceller(Chain::new(), SonoraEchoCanceller::new())
        } else {
            (Chain::new(), None)
        };
        let mic = with_mic_gain(mic, SonoraMicGain::new(Arc::clone(&setup.gain)))
            .tap(PeakTap::new(Arc::clone(&setup.levels), MeterSlot::Mic));
        let system = Chain::new()
            .process(FixedGain::new(SYSTEM_GAIN)?)
            .tap(PeakTap::new(Arc::clone(&setup.levels), MeterSlot::System));
        let mix = Chain::new()
            .process(SoftLimiter::counting(Arc::clone(&limited_samples)))
            .tap(PeakTap::new(setup.levels, MeterSlot::Mixed));
        Ok(Pipeline::from_stages(PipelineStages {
            mic_resampler: Box::new(StereoResampler::new(
                INTERNAL_SAMPLE_RATE,
                INTERNAL_SAMPLE_RATE,
            )),
            system_resampler: Box::new(StereoResampler::new(
                INTERNAL_SAMPLE_RATE,
                INTERNAL_SAMPLE_RATE,
            )),
            aligner: Aligner::new(setup.system_active),
            mic,
            system,
            mixer: Box::new(SumMixer),
            mix,
            limited_samples,
            echo_stats,
        }))
    }
}

/// Test seam: the mic-gain constructor is reported as failed, the same way a
/// real `SonoraMicGain::new` error is. System audio being off is not involved.
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct FailedMicGainFactory;

#[cfg(test)]
struct IdentityResampler;

#[cfg(test)]
impl crate::services::ports::DriftResampler for IdentityResampler {
    fn set_drift_error(&mut self, _frames: i64) {}

    fn process(&mut self, interleaved: &[f32]) -> Vec<f32> {
        interleaved.to_vec()
    }

    fn flush(&mut self) -> Vec<f32> {
        Vec::new()
    }

    fn reset(&mut self) {}
}

#[cfg(test)]
impl PipelineFactory for FailedMicGainFactory {
    fn build(&self, setup: PipelineSetup) -> Result<Pipeline, ProcessError> {
        let mic = with_mic_gain(
            Chain::new(),
            Err(ProcessError::new(
                STEP_NAME,
                "microphone gain was not configured",
            )),
        );
        Ok(Pipeline::from_stages(PipelineStages {
            mic_resampler: Box::new(IdentityResampler),
            system_resampler: Box::new(IdentityResampler),
            aligner: Aligner::new(setup.system_active),
            mic,
            system: Chain::new(),
            mixer: Box::new(SumMixer),
            mix: Chain::new(),
            limited_samples: Arc::new(AtomicU64::new(0)),
            echo_stats: None,
        }))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{
        with_echo_canceller, with_mic_gain, EchoCancellerStats, SonoraEchoCanceller, SonoraMicGain,
        SonoraPipelineFactory, AEC_STEP_NAME, STEP_NAME,
    };
    use crate::domain::audio::INTERNAL_SAMPLE_RATE;
    use crate::domain::audio_frame::{AudioFrame, ChannelLayout, FRAME_LEN};
    use crate::infrastructure::audio::levels::{GainControl, LevelSnapshot};
    use crate::services::audio_pipeline::{Chain, Pipeline, PipelineSetup};
    use crate::services::ports::{AudioProcessor, PipelineFactory, ProcessContext, ProcessError};

    const RATE: f32 = 48_000.0;
    const TONE_HZ: f32 = 440.0;
    const AMPLITUDE: f32 = 0.2;

    fn context() -> ProcessContext<'static> {
        ProcessContext {
            echo_reference: None,
        }
    }

    fn sine_frame(frame_index: usize) -> AudioFrame {
        let mut samples = vec![0.0; FRAME_LEN];
        for (index, sample) in samples.iter_mut().enumerate() {
            let n = frame_index * FRAME_LEN + index;
            let phase = 2.0 * std::f32::consts::PI * TONE_HZ * n as f32 / RATE;
            *sample = AMPLITUDE * phase.sin();
        }
        AudioFrame::new(ChannelLayout::Mono, samples, FRAME_LEN, 0).expect("frame")
    }

    fn peak(frame: &AudioFrame) -> f32 {
        frame
            .samples()
            .iter()
            .fold(0.0_f32, |loudest, sample| loudest.max(sample.abs()))
    }

    /// Peak of the frame after the one-frame ramp. `new` does not call the
    /// setter; the first `process` does, and that capture call is the ramp.
    fn settled_peak(gain: f32) -> f32 {
        let control = Arc::new(GainControl::new(gain));
        let mut step = SonoraMicGain::new(Arc::clone(&control)).expect("gain");
        let mut frame = sine_frame(0);
        step.process(&mut frame, &context()).expect("ramp");
        frame = sine_frame(1);
        step.process(&mut frame, &context()).expect("settled");
        peak(&frame)
    }

    #[test]
    fn slider_level_follows_the_gain() {
        // Arrange
        let half = 0.5;
        let unity = 1.0;

        // Act
        let half_peak = settled_peak(half);
        let unity_peak = settled_peak(unity);

        // Assert
        assert!((half_peak - 0.1).abs() < 1e-3, "gain 0.5 peak {half_peak}");
        assert!(
            (unity_peak - 0.2).abs() < 1e-3,
            "gain 1.0 peak {unity_peak}"
        );
    }

    #[test]
    fn slider_change_applies_during_a_stream() {
        // Arrange
        let gain = Arc::new(GainControl::new(1.0));
        let mut step = SonoraMicGain::new(Arc::clone(&gain)).expect("gain");
        let mut frame = sine_frame(0);
        step.process(&mut frame, &context()).expect("first");
        frame = sine_frame(1);
        step.process(&mut frame, &context()).expect("unity");

        // Act
        // The next `process` reads 0.5 and calls `set_capture_post_gain`.
        // That capture call is the ramp D-13 measured. The assertion is on
        // the frame after that ramp, where the level has settled.
        gain.set_slider(0.5);
        frame = sine_frame(2);
        step.process(&mut frame, &context()).expect("ramp");
        frame = sine_frame(3);
        step.process(&mut frame, &context()).expect("settled");

        // Assert
        let settled = peak(&frame);
        assert!((settled - 0.1).abs() < 1e-3, "post-ramp peak {settled}");
    }

    #[test]
    fn gain_zero_is_silent() {
        // Arrange
        let gain = Arc::new(GainControl::new(0.0));
        let mut step = SonoraMicGain::new(gain).expect("gain");
        let mut frame = sine_frame(0);

        // Act
        // The first capture ramps toward silence. The next frame is the one
        // that must be silent.
        step.process(&mut frame, &context()).expect("ramp");
        frame = sine_frame(1);
        step.process(&mut frame, &context()).expect("silent");

        // Assert
        let settled = peak(&frame);
        assert!(settled < 1e-4, "settled peak {settled}");
    }

    #[test]
    fn missing_mic_gain_leaves_unity() {
        // Arrange
        let original = sine_frame(0);
        let mut frame = original.clone();
        let mut chain = with_mic_gain(
            Chain::new(),
            Err(ProcessError::new(
                STEP_NAME,
                "microphone gain was not configured",
            )),
        );

        // Act
        chain.run(&mut frame, &context());

        // Assert
        assert_eq!(frame.samples(), original.samples());
    }

    #[test]
    fn bootstrap_wires_the_sonora_factory() {
        // Arrange
        let source = include_str!("../../app/bootstrap.rs");

        // Act
        let wires_sonora = source.contains("Box::new(SonoraPipelineFactory)");
        let wires_linear = source.contains("LinearPipelineFactory");

        // Assert
        assert!(wires_sonora);
        assert!(!wires_linear);
    }

    /// D-13 full-band energy ERLE at 100 ms was about 40 dB. The spike accepts
    /// anything above 20 dB. This is not `echo_return_loss_enhancement`.
    const ECHO_ERLE_FLOOR_DB: f64 = 20.0;
    /// D-13 silent-render speech energy ratio was 0.845. The spike floor is 0.5.
    const SILENT_RENDER_SPEECH_FLOOR: f64 = 0.5;
    /// D-13 double-talk speech energy ratios were about 0.16, 0.22, and 0.16.
    /// The waveform is distorted, so the floor only checks that energy remains.
    const DOUBLE_TALK_SPEECH_FLOOR: f64 = 0.10;

    struct XorShift64 {
        state: u64,
    }

    impl XorShift64 {
        fn new(seed: u64) -> Self {
            Self { state: seed.max(1) }
        }

        fn next_unit(&mut self) -> f32 {
            let mut x = self.state;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.state = x;
            let bits = (x >> 40) as u32;
            (bits as f32 / 16_777_216.0) * 2.0 - 1.0
        }
    }

    fn energy(samples: &[f32]) -> f64 {
        samples
            .iter()
            .map(|sample| {
                let value = f64::from(*sample);
                value * value
            })
            .sum()
    }

    fn erle_db(reference: &[f32], residual: &[f32]) -> f64 {
        let input = energy(reference);
        let output = energy(residual).max(1e-20);
        10.0 * (input / output).log10()
    }

    struct Measured {
        erle_db: Option<f64>,
        speech_energy_ratio: Option<f64>,
        stats: EchoCancellerStats,
    }

    /// Same noise shape as the S6 harness: low-passed render, 0.3× delayed echo,
    /// and a slower speech-like noise with a 3 Hz envelope.
    fn measure_window(
        delay_ms: u32,
        echo_seconds: u32,
        speech_seconds: u32,
        silence_render: bool,
    ) -> Measured {
        let mut step = SonoraEchoCanceller::new().expect("echo canceller");
        let mut render_noise = XorShift64::new(0x51_56_4C_49 ^ u64::from(delay_ms));
        let mut speech_noise = XorShift64::new(0xA5A5_F00D ^ u64::from(delay_ms));
        let mut render_lp = 0.0_f32;
        let mut speech_lp = 0.0_f32;
        let delay_samples =
            usize::try_from(u64::from(delay_ms) * u64::from(INTERNAL_SAMPLE_RATE) / 1_000)
                .unwrap_or(0);
        let echo_frames = usize::try_from(echo_seconds)
            .unwrap_or(0)
            .saturating_mul(100);
        let speech_frames = usize::try_from(speech_seconds)
            .unwrap_or(0)
            .saturating_mul(100);
        let total_frames = echo_frames.saturating_add(speech_frames);
        let measure_from = echo_frames.saturating_sub(200);
        let mut played_history = Vec::with_capacity(total_frames.saturating_mul(FRAME_LEN));
        let mut echo_only_in = Vec::new();
        let mut echo_only_out = Vec::new();
        let mut speech_ref = Vec::new();
        let mut speech_out = Vec::new();
        let mut interleaved = vec![0.0; FRAME_LEN * 2];
        let mut mic_samples = vec![0.0; FRAME_LEN];
        let mut mic = AudioFrame::silence(ChannelLayout::Mono, 0);
        let mut reference = AudioFrame::silence(ChannelLayout::Stereo, 0);

        for frame in 0..total_frames {
            let speech_on = frame >= echo_frames;
            for index in 0..FRAME_LEN {
                let white = render_noise.next_unit();
                render_lp = 0.85 * render_lp + 0.15 * white;
                let shaped = if silence_render { 0.0 } else { render_lp * 0.4 };
                interleaved[index * 2] = shaped;
                interleaved[index * 2 + 1] = shaped;
                played_history.push(shaped);
                let n = played_history.len() - 1;
                let echo = if n >= delay_samples {
                    played_history[n - delay_samples] * 0.3
                } else {
                    0.0
                };
                let speech = if speech_on {
                    let voice = speech_noise.next_unit();
                    speech_lp = 0.90 * speech_lp + 0.10 * voice;
                    let envelope = (2.0 * std::f32::consts::PI * 3.0 * n as f32
                        / INTERNAL_SAMPLE_RATE as f32)
                        .sin()
                        .abs();
                    speech_lp * 0.25 * envelope
                } else {
                    0.0
                };
                mic_samples[index] = echo + speech;
                if frame >= measure_from && !speech_on {
                    echo_only_in.push(echo);
                }
                if speech_on {
                    speech_ref.push(speech);
                }
            }
            mic.fill_samples(&mic_samples).expect("mic frame");
            reference
                .fill_samples(&interleaved)
                .expect("reference frame");
            let context = ProcessContext {
                echo_reference: Some(&reference),
            };
            step.process(&mut mic, &context).expect("echo canceller");
            if frame >= measure_from && !speech_on {
                echo_only_out.extend_from_slice(mic.samples());
            }
            if speech_on {
                speech_out.extend_from_slice(mic.samples());
            }
        }

        let erle_db = if echo_only_in.is_empty() {
            None
        } else {
            Some(erle_db(&echo_only_in, &echo_only_out))
        };
        let speech_energy_ratio = if speech_ref.is_empty() {
            None
        } else {
            Some(energy(&speech_out) / energy(&speech_ref).max(1e-20))
        };
        Measured {
            erle_db,
            speech_energy_ratio,
            stats: step.echo_statistics(),
        }
    }

    fn production_pipeline(system_active: bool) -> Pipeline {
        SonoraPipelineFactory
            .build(PipelineSetup {
                system_active,
                gain: Arc::new(GainControl::new(1.0)),
                levels: Arc::new(LevelSnapshot::new()),
            })
            .expect("pipeline")
    }

    #[test]
    fn delayed_system_audio_is_reduced_in_the_mic() {
        // Arrange
        let delay_ms = 100;
        let echo_seconds = 4;

        // Act
        let measured = measure_window(delay_ms, echo_seconds, 0, false);

        // Assert
        let erle = measured.erle_db.expect("echo window");
        assert!(
            erle > ECHO_ERLE_FLOOR_DB,
            "full-band energy ERLE {erle:.2} dB at {delay_ms} ms"
        );
        if let Some(internal) = measured.stats.echo_return_loss_enhancement {
            assert!(internal.is_finite());
            assert!(
                erle > internal,
                "energy ERLE {erle:.2} dB is the figure under test, not the capped statistic {internal:.2}"
            );
        }
    }

    #[test]
    fn speech_without_render_passes_nearly_unchanged() {
        // Arrange
        let speech_seconds = 3;

        // Act
        let measured = measure_window(0, 0, speech_seconds, true);

        // Assert
        let ratio = measured.speech_energy_ratio.expect("speech window");
        assert!(
            ratio > SILENT_RENDER_SPEECH_FLOOR,
            "silent-render speech energy ratio {ratio:.3}"
        );
        // A skipped silent reference would leave the ratio at 1. D-13 measured 0.845.
        assert!(
            ratio < 0.98,
            "silent render was not processed; speech energy ratio {ratio:.3}"
        );
    }

    #[test]
    fn near_speech_survives_during_echo() {
        // Arrange
        let delay_ms = 100;
        let echo_seconds = 4;
        let speech_seconds = 3;

        // Act
        let measured = measure_window(delay_ms, echo_seconds, speech_seconds, false);

        // Assert
        let ratio = measured.speech_energy_ratio.expect("double-talk window");
        assert!(
            ratio > DOUBLE_TALK_SPEECH_FLOOR,
            "double-talk speech energy ratio {ratio:.3}"
        );
    }

    #[test]
    fn missing_echo_reference_leaves_the_microphone() {
        // Arrange
        let mut step = SonoraEchoCanceller::new().expect("echo canceller");
        let original = sine_frame(0);
        let mut frame = original.clone();

        // Act
        let result = step.process(&mut frame, &context());

        // Assert
        assert!(result.is_ok());
        assert_eq!(frame.samples(), original.samples());
    }

    #[test]
    fn echo_canceller_creation_failure_skips_the_step() {
        // Arrange
        let original = sine_frame(0);
        let mut frame = original.clone();
        let (mut chain, stats) = with_echo_canceller(
            Chain::new(),
            Err(ProcessError::new(
                AEC_STEP_NAME,
                "echo canceller was not configured",
            )),
        );

        // Act
        chain.run(&mut frame, &context());

        // Assert
        assert_eq!(frame.samples(), original.samples());
        assert!(chain.process_names().is_empty());
        assert!(stats.is_none());
        assert!(chain.processing_problem());
    }

    #[test]
    fn echo_canceller_is_omitted_when_system_audio_is_off() {
        // Arrange
        let pipeline = production_pipeline(false);

        // Act
        let names = pipeline.mic_process_names();

        // Assert
        assert_eq!(names, ["sonora-mic-gain"]);
    }

    #[test]
    fn echo_canceller_precedes_gain_when_system_audio_is_on() {
        // Arrange
        let pipeline = production_pipeline(true);

        // Act
        let names = pipeline.mic_process_names();

        // Assert
        assert_eq!(names, ["sonora-echo-canceller", "sonora-mic-gain"]);
    }

    fn identity_pipeline(
        mic: Chain,
        echo_stats: Option<std::sync::Arc<crate::services::audio_pipeline::EchoStatsSlot>>,
    ) -> Pipeline {
        use std::sync::atomic::AtomicU64;

        use crate::services::audio_pipeline::{Aligner, PipelineStages, SumMixer};

        Pipeline::from_stages(PipelineStages {
            mic_resampler: Box::new(super::IdentityResampler),
            system_resampler: Box::new(super::IdentityResampler),
            aligner: Aligner::new(false),
            mic,
            system: Chain::new(),
            mixer: Box::new(SumMixer),
            mix: Chain::new(),
            limited_samples: Arc::new(AtomicU64::new(0)),
            echo_stats,
        })
    }

    #[test]
    fn skipped_sonora_constructor_marks_audio_processing() {
        // Arrange
        let (mic, stats) = with_echo_canceller(
            Chain::new(),
            Err(ProcessError::new(
                AEC_STEP_NAME,
                "echo canceller was not configured",
            )),
        );
        let gain = with_mic_gain(
            Chain::new(),
            Err(ProcessError::new(
                STEP_NAME,
                "microphone gain was not configured",
            )),
        );

        // Act
        let echo_pipeline = identity_pipeline(mic, stats);
        let gain_pipeline = identity_pipeline(gain, None);

        // Assert
        assert!(echo_pipeline.diagnostics().audio_processing);
        assert!(echo_pipeline.diagnostics().echo_stats.is_none());
        assert!(gain_pipeline.diagnostics().audio_processing);
        assert!(gain_pipeline.diagnostics().echo_stats.is_none());
    }

    #[test]
    fn system_audio_off_does_not_mark_audio_processing() {
        // Arrange
        let pipeline = production_pipeline(false);

        // Act
        let diagnostics = pipeline.diagnostics();

        // Assert
        assert!(!diagnostics.audio_processing);
        assert!(diagnostics.echo_stats.is_none());
        assert_eq!(pipeline.mic_process_names(), ["sonora-mic-gain"]);
    }

    #[test]
    fn diagnostics_returns_the_stats_the_canceller_wrote() {
        // Arrange
        let mut step = SonoraEchoCanceller::new().expect("canceller");
        let slot = step.stats_slot();
        let mut mic = sine_frame(0);
        let mut reference = AudioFrame::silence(ChannelLayout::Stereo, 0);
        reference
            .fill_samples(&vec![0.1; FRAME_LEN * 2])
            .expect("reference");

        // Act
        step.process(&mut mic, &context()).expect("no reference");
        let before = slot.latest();
        mic.fill_samples(&vec![0.2; FRAME_LEN]).expect("mic");
        step.process(
            &mut mic,
            &ProcessContext {
                echo_reference: Some(&reference),
            },
        )
        .expect("cancel");
        let written = step.echo_statistics();
        let pipeline = identity_pipeline(Chain::new().process(step), Some(Arc::clone(&slot)));
        let reported = pipeline.diagnostics().echo_stats;

        // Assert
        assert!(before.is_none());
        let published = slot.latest().expect("published");
        assert_eq!(
            published.echo_return_loss_enhancement,
            written.echo_return_loss_enhancement
        );
        assert_eq!(published.delay_ms, written.delay_ms);
        let reported = reported.expect("diagnostics");
        assert_eq!(
            reported.echo_return_loss_enhancement,
            written.echo_return_loss_enhancement
        );
        assert_eq!(reported.delay_ms, written.delay_ms);
    }
}
