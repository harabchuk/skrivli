//! Phase S spike for sonora 0.2.0. Not the production pipeline.
//!
//! Synthetic signals only, except the ignored hardware probe. Slow, device,
//! and one-shot measurements are `#[ignore]` so default `cargo test` stays
//! short and device-free.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::time::{Duration, Instant};

use sonora::config::{CaptureLevelAdjustment, DownmixMethod, EchoCanceller, MaxProcessingRate};
use sonora::{AudioProcessing, Config, StreamConfig};

use super::resampler::StereoResampler;

const RATE: u32 = 48_000;
const FRAME: usize = 480;
const TONE_HZ: f32 = 440.0;

#[derive(Clone, Copy)]
enum RenderSide {
    Both,
    Left,
    Right,
}

fn gain_apm(capture_level_adjustment: bool) -> AudioProcessing {
    let config = if capture_level_adjustment {
        Config {
            capture_level_adjustment: Some(CaptureLevelAdjustment::default()),
            ..Config::default()
        }
    } else {
        Config::default()
    };
    AudioProcessing::builder()
        .config(config)
        .capture_config(StreamConfig::new(RATE, 1))
        .render_config(StreamConfig::new(RATE, 1))
        .build()
}

fn aec_apm() -> AudioProcessing {
    AudioProcessing::builder()
        .config(Config {
            echo_canceller: Some(EchoCanceller::default()),
            ..Config::default()
        })
        .capture_config(StreamConfig::new(RATE, 1))
        .render_config(StreamConfig::new(RATE, 2))
        .build()
}

fn sine_frame(frame_index: usize, amplitude: f32) -> Vec<f32> {
    let mut frame = vec![0.0; FRAME];
    for (index, sample) in frame.iter_mut().enumerate() {
        let n = frame_index * FRAME + index;
        let phase = 2.0 * std::f32::consts::PI * TONE_HZ * n as f32 / RATE as f32;
        *sample = amplitude * phase.sin();
    }
    frame
}

fn peak_abs(frame: &[f32]) -> f32 {
    frame
        .iter()
        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()))
}

fn max_adjacent_step(frame: &[f32]) -> f32 {
    frame
        .windows(2)
        .fold(0.0_f32, |step, pair| step.max((pair[1] - pair[0]).abs()))
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "non-string panic".to_string()
    }
}

fn call_result(result: Result<Result<(), sonora::Error>, Box<dyn std::any::Any + Send>>) -> String {
    match result {
        Ok(Ok(())) => "ok".to_string(),
        Ok(Err(error)) => format!("err {error}"),
        Err(payload) => format!("panic {}", panic_message(payload.as_ref())),
    }
}

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

fn correlation(left: &[f32], right: &[f32]) -> f64 {
    let n = left.len().min(right.len());
    if n == 0 {
        return 0.0;
    }
    let dot: f64 = left[..n]
        .iter()
        .zip(right[..n].iter())
        .map(|(a, b)| f64::from(*a) * f64::from(*b))
        .sum();
    let denom = energy(&left[..n]).sqrt() * energy(&right[..n]).sqrt();
    if denom == 0.0 {
        0.0
    } else {
        dot / denom
    }
}

/// Best match of `delayed` against `reference` when `delayed` lags by 0..=max_lag.
fn best_lag(delayed: &[f32], reference: &[f32], max_lag: usize) -> (usize, f64) {
    let mut best_lag = 0_usize;
    let mut best = 0.0_f64;
    for lag in 0..=max_lag {
        if lag >= delayed.len() {
            break;
        }
        let value = correlation(&delayed[lag..], reference);
        if value.abs() > best.abs() {
            best = value;
            best_lag = lag;
        }
    }
    (best_lag, best)
}

fn erle_db(reference: &[f32], residual: &[f32]) -> f64 {
    let input = energy(reference);
    let output = energy(residual).max(1e-20);
    10.0 * (input / output).log10()
}

struct GainStep {
    gain: f32,
    peak: f32,
    join_step: f32,
    transition_step: f32,
    steady_step: f32,
}

fn process_capture(apm: &mut AudioProcessing, input: &[f32]) -> Vec<f32> {
    let mut output = vec![0.0; input.len()];
    apm.process_capture_f32(&[input], &mut [&mut output])
        .expect("capture frame");
    output
}

fn measure_gain_steps(capture_level_adjustment: bool, amplitude: f32) -> Vec<GainStep> {
    let mut apm = gain_apm(capture_level_adjustment);
    let mut frame_index = 0_usize;
    let mut previous = vec![0.0; FRAME];
    for _ in 0..3 {
        previous = process_capture(&mut apm, &sine_frame(frame_index, amplitude));
        frame_index += 1;
    }
    let mut steps = Vec::new();
    for gain in [0.0_f32, 0.5, 1.0, 4.0] {
        apm.set_capture_post_gain(gain);
        let transition = process_capture(&mut apm, &sine_frame(frame_index, amplitude));
        frame_index += 1;
        let mut settled = vec![0.0; FRAME];
        for _ in 0..2 {
            settled = process_capture(&mut apm, &sine_frame(frame_index, amplitude));
            frame_index += 1;
        }
        let join_step = (transition[0] - previous[previous.len() - 1]).abs();
        steps.push(GainStep {
            gain,
            peak: peak_abs(&settled),
            join_step,
            transition_step: max_adjacent_step(&transition),
            steady_step: max_adjacent_step(&settled),
        });
        previous = settled;
    }
    steps
}

struct ImpulseDelay {
    onset_samples: i64,
    peak_samples: i64,
    peak: f32,
    proc_rate_hz: usize,
}

fn measure_impulse_delay(aec: bool) -> ImpulseDelay {
    let mut apm = if aec { aec_apm() } else { gain_apm(true) };
    const LEAD: usize = 4;
    const TAIL: usize = 10;
    let mut collected = Vec::with_capacity(FRAME * (LEAD + 1 + TAIL));
    for frame in 0..(LEAD + 1 + TAIL) {
        let mut capture = vec![0.0; FRAME];
        if frame == LEAD {
            capture[0] = 0.5;
        }
        let mut capture_out = vec![0.0; FRAME];
        if aec {
            let silent = vec![0.0; FRAME];
            let mut render_left = vec![0.0; FRAME];
            let mut render_right = vec![0.0; FRAME];
            apm.process_render_f32(
                &[&silent, &silent],
                &mut [&mut render_left, &mut render_right],
            )
            .expect("silent render");
            apm.set_stream_delay_ms(0).expect("delay 0");
        }
        apm.process_capture_f32(&[&capture], &mut [&mut capture_out])
            .expect("impulse capture");
        collected.extend_from_slice(&capture_out);
    }
    let impulse_at = LEAD * FRAME;
    let mut peak_index = 0_usize;
    let mut peak = 0.0_f32;
    for (index, sample) in collected.iter().enumerate() {
        let magnitude = sample.abs();
        if magnitude > peak {
            peak = magnitude;
            peak_index = index;
        }
    }
    let onset = collected
        .iter()
        .position(|sample| sample.abs() > peak * 0.05 && sample.abs() > 1.0e-4)
        .unwrap_or(peak_index);
    ImpulseDelay {
        onset_samples: onset as i64 - impulse_at as i64,
        peak_samples: peak_index as i64 - impulse_at as i64,
        peak,
        proc_rate_hz: apm.proc_sample_rate_hz(),
    }
}

struct AecWindow {
    delay_ms: u32,
    erle_db: f64,
    speech_correlation: f64,
    speech_lag_samples: usize,
    echo_correlation: f64,
    echo_lag_samples: usize,
    speech_energy_ratio: f64,
    stats_erle_db: Option<f64>,
    stats_delay_ms: Option<i32>,
    render_side: RenderSide,
    silence_render: bool,
}

fn render_sides(side: RenderSide, sample: f32) -> (f32, f32, f32) {
    match side {
        RenderSide::Both => (sample, sample, sample),
        RenderSide::Left => (sample, 0.0, sample),
        RenderSide::Right => (0.0, sample, sample),
    }
}

fn run_aec_window(
    delay_ms: u32,
    echo_seconds: u32,
    speech_seconds: u32,
    render_side: RenderSide,
    silence_render: bool,
) -> AecWindow {
    let mut apm = aec_apm();
    let mut render_noise = XorShift64::new(0x51_56_4C_49 ^ u64::from(delay_ms));
    let mut speech_noise = XorShift64::new(0xA5A5_F00D ^ u64::from(delay_ms) ^ render_side as u64);
    let mut render_lp = 0.0_f32;
    let mut speech_lp = 0.0_f32;
    let delay_samples = usize::try_from(u64::from(delay_ms) * u64::from(RATE) / 1_000).unwrap_or(0);
    let echo_frames = usize::try_from(echo_seconds)
        .unwrap_or(0)
        .saturating_mul(100);
    let speech_frames = usize::try_from(speech_seconds)
        .unwrap_or(0)
        .saturating_mul(100);
    let total_frames = echo_frames.saturating_add(speech_frames);
    let measure_from = echo_frames.saturating_sub(200);
    let mut played_history = Vec::with_capacity(total_frames.saturating_mul(FRAME));
    let mut echo_only_in = Vec::new();
    let mut echo_only_out = Vec::new();
    let mut speech_ref = Vec::new();
    let mut echo_ref = Vec::new();
    let mut speech_out = Vec::new();
    let mut left = vec![0.0; FRAME];
    let mut right = vec![0.0; FRAME];
    let mut mic = vec![0.0; FRAME];
    let mut render_left_out = vec![0.0; FRAME];
    let mut render_right_out = vec![0.0; FRAME];
    let mut mic_out = vec![0.0; FRAME];

    for frame in 0..total_frames {
        let speech_on = frame >= echo_frames;
        for index in 0..FRAME {
            let white = render_noise.next_unit();
            render_lp = 0.85 * render_lp + 0.15 * white;
            let shaped = if silence_render { 0.0 } else { render_lp * 0.4 };
            let (left_sample, right_sample, played) = render_sides(render_side, shaped);
            left[index] = left_sample;
            right[index] = right_sample;
            played_history.push(played);
            let n = played_history.len() - 1;
            let echo = if n >= delay_samples {
                played_history[n - delay_samples] * 0.3
            } else {
                0.0
            };
            let speech = if speech_on {
                let voice = speech_noise.next_unit();
                speech_lp = 0.90 * speech_lp + 0.10 * voice;
                let envelope = (2.0 * std::f32::consts::PI * 3.0 * n as f32 / RATE as f32)
                    .sin()
                    .abs();
                speech_lp * 0.25 * envelope
            } else {
                0.0
            };
            mic[index] = echo + speech;
            if frame >= measure_from && !speech_on {
                echo_only_in.push(echo);
            }
            if speech_on {
                speech_ref.push(speech);
                echo_ref.push(echo);
            }
        }
        apm.process_render_f32(
            &[&left, &right],
            &mut [&mut render_left_out, &mut render_right_out],
        )
        .expect("aec render");
        apm.set_stream_delay_ms(0).expect("delay 0");
        apm.process_capture_f32(&[&mic], &mut [&mut mic_out])
            .expect("aec capture");
        if frame >= measure_from && !speech_on {
            echo_only_out.extend_from_slice(&mic_out);
        }
        if speech_on {
            speech_out.extend_from_slice(&mic_out);
        }
    }
    let stats = apm.statistics().clone();
    let speech_energy = energy(&speech_ref).max(1e-20);
    let (speech_lag_samples, speech_correlation) = best_lag(&speech_out, &speech_ref, 800);
    let (echo_lag_samples, echo_correlation) = best_lag(&speech_out, &echo_ref, 800);
    AecWindow {
        delay_ms,
        erle_db: erle_db(&echo_only_in, &echo_only_out),
        speech_correlation,
        speech_lag_samples,
        echo_correlation,
        echo_lag_samples,
        speech_energy_ratio: energy(&speech_out) / speech_energy,
        stats_erle_db: stats.echo_return_loss_enhancement,
        stats_delay_ms: stats.delay_ms,
        render_side,
        silence_render,
    }
}

impl RenderSide {
    fn label(self) -> &'static str {
        match self {
            Self::Both => "both",
            Self::Left => "left",
            Self::Right => "right",
        }
    }
}

struct SourceProbe {
    name: &'static str,
    frame_index_advance: u64,
    qpc_frames: u64,
    delta_frames: i64,
    chunks: u64,
    max_abs_drift_error: i64,
}

enum ProbeReport {
    Opened(Vec<SourceProbe>),
    Unavailable(String),
}

fn probe_devices(duration: Duration) -> ProbeReport {
    use std::sync::Arc;

    use flexaudio::SourceKind;

    use crate::domain::audio::{SessionOrigin, INTERNAL_SAMPLE_RATE};
    use crate::infrastructure::audio::levels::{GainControl, LevelSnapshot};
    use crate::infrastructure::platform::performance_counter;
    use crate::services::audio_pipeline::{AudioSource, PassThroughPipelineFactory, PipelineSetup};
    use crate::services::ports::PipelineFactory;

    let clock = performance_counter();
    let (origin_ticks, ticks_per_second) = clock.sample();
    let Some(origin) = SessionOrigin::new(origin_ticks, ticks_per_second, INTERNAL_SAMPLE_RATE)
    else {
        return ProbeReport::Unavailable("qpc origin rejected".to_string());
    };
    let mic = match open_source(SourceKind::Mic) {
        Ok(stream) => stream,
        Err(error) => return ProbeReport::Unavailable(format!("mic: {error}")),
    };
    let system = match open_source(SourceKind::SystemLoopback) {
        Ok(stream) => stream,
        Err(error) => {
            let mut mic = mic;
            mic.stop();
            return ProbeReport::Unavailable(format!("system: {error}"));
        }
    };
    // Pass-through keeps chunk lengths, so the drift figure stays the aligner's
    // timeline error. The resampler itself is not what this probe measures.
    let mut pipeline = match PassThroughPipelineFactory.build(PipelineSetup {
        system_active: true,
        gain: Arc::new(GainControl::new(1.0)),
        levels: Arc::new(LevelSnapshot::new()),
    }) {
        Ok(pipeline) => pipeline,
        Err(error) => return ProbeReport::Unavailable(format!("pipeline: {error}")),
    };
    let mut mic_track = ProbeTrack::new("mic");
    let mut system_track = ProbeTrack::new("system");
    let deadline = Instant::now() + duration;
    let mut streams = [mic, system];
    while Instant::now() < deadline {
        let mut idle = true;
        if let Some(chunk) = streams[0].poll_chunk() {
            idle = false;
            accept_chunk(
                &chunk,
                &mut mic_track,
                clock.as_ref(),
                origin,
                &mut pipeline,
                AudioSource::Mic,
            );
        }
        if let Some(chunk) = streams[1].poll_chunk() {
            idle = false;
            accept_chunk(
                &chunk,
                &mut system_track,
                clock.as_ref(),
                origin,
                &mut pipeline,
                AudioSource::System,
            );
        }
        if idle {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    streams[0].stop();
    streams[1].stop();
    let _ = pipeline.diagnostics();
    ProbeReport::Opened(vec![mic_track.finish(), system_track.finish()])
}

fn accept_chunk(
    chunk: &flexaudio::AudioChunk,
    track: &mut ProbeTrack,
    clock: &dyn crate::services::ports::PerformanceCounter,
    origin: crate::domain::audio::SessionOrigin,
    pipeline: &mut crate::services::audio_pipeline::Pipeline,
    source: crate::services::audio_pipeline::AudioSource,
) {
    use crate::domain::audio::AudioBlock;
    use crate::infrastructure::audio::flex_capture::{chunk_discontinuity, map_timeline};
    use crate::services::audio_pipeline::AudioSource;

    let timeline = map_timeline(origin, clock, &mut track.base, chunk.frame_index);
    track.observe(chunk, clock, origin);
    let block = AudioBlock {
        samples: chunk.data.clone(),
        timeline_frame: timeline,
        discontinuity: chunk_discontinuity(chunk),
    };
    match source {
        AudioSource::Mic => pipeline.push_mic(block),
        AudioSource::System => pipeline.push_system(block),
    }
    track.note_drift(pipeline.drift_error(source));
}

fn open_source(kind: flexaudio::SourceKind) -> Result<flexaudio::Stream, String> {
    use flexaudio::{open_with_options, OpenOptions, OutputFormat, StreamConfig};

    use crate::domain::audio::{INTERNAL_CHANNELS, INTERNAL_SAMPLE_RATE};

    let config = StreamConfig {
        kind,
        device_id: None,
        output: OutputFormat {
            sample_rate: INTERNAL_SAMPLE_RATE,
            channels: INTERNAL_CHANNELS,
        },
        ..StreamConfig::default()
    };
    let mut stream =
        open_with_options(config, OpenOptions::new()).map_err(|error| error.to_string())?;
    stream.start().map_err(|error| error.to_string())?;
    Ok(stream)
}

struct ProbeTrack {
    name: &'static str,
    base: Option<u64>,
    first_index: Option<u64>,
    last_index: Option<u64>,
    first_qpc: Option<u64>,
    last_qpc: Option<u64>,
    chunks: u64,
    max_abs_drift_error: i64,
}

impl ProbeTrack {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            base: None,
            first_index: None,
            last_index: None,
            first_qpc: None,
            last_qpc: None,
            chunks: 0,
            max_abs_drift_error: 0,
        }
    }

    fn observe(
        &mut self,
        chunk: &flexaudio::AudioChunk,
        clock: &dyn crate::services::ports::PerformanceCounter,
        origin: crate::domain::audio::SessionOrigin,
    ) {
        let (ticks, _) = clock.sample();
        let qpc_frame = origin.sample_index(ticks).unwrap_or(0);
        if self.first_index.is_none() {
            self.first_index = Some(chunk.frame_index);
            self.first_qpc = Some(qpc_frame);
        }
        self.last_index = Some(chunk.frame_index.saturating_add(chunk.frames as u64));
        self.last_qpc = Some(qpc_frame);
        self.chunks = self.chunks.saturating_add(1);
    }

    fn note_drift(&mut self, error: i64) {
        self.max_abs_drift_error = self.max_abs_drift_error.max(error.abs());
    }

    fn finish(self) -> SourceProbe {
        let frame_index_advance = self
            .last_index
            .unwrap_or(0)
            .saturating_sub(self.first_index.unwrap_or(0));
        let qpc_frames = self
            .last_qpc
            .unwrap_or(0)
            .saturating_sub(self.first_qpc.unwrap_or(0));
        SourceProbe {
            name: self.name,
            frame_index_advance,
            qpc_frames,
            delta_frames: i64::try_from(frame_index_advance).unwrap_or(i64::MAX)
                - i64::try_from(qpc_frames).unwrap_or(i64::MAX),
            chunks: self.chunks,
            max_abs_drift_error: self.max_abs_drift_error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s2_wrong_frame_length_panics() {
        // Arrange
        let full = vec![0.1; FRAME];
        let mut full_out = vec![0.0; FRAME];
        let short = vec![0.1; FRAME - 1];
        let mut short_out = vec![0.0; FRAME - 1];

        // Act
        let mut capture_ok = gain_apm(true);
        let valid = capture_ok.process_capture_f32(&[&full], &mut [&mut full_out]);
        let mut capture_short = gain_apm(true);
        let capture = catch_unwind(AssertUnwindSafe(|| {
            capture_short.process_capture_f32(&[&short], &mut [&mut short_out])
        }));
        let mut render_short = aec_apm();
        let render_right = vec![0.0; FRAME - 1];
        let mut render_left_out = vec![0.0; FRAME - 1];
        let mut render_right_out = vec![0.0; FRAME - 1];
        let render = catch_unwind(AssertUnwindSafe(|| {
            render_short.process_render_f32(
                &[&short, &render_right],
                &mut [&mut render_left_out, &mut render_right_out],
            )
        }));
        let capture_text = call_result(capture);
        let render_text = call_result(render);
        eprintln!("S2 valid_480={valid:?} capture_479={capture_text} render_479={render_text}");

        // Assert — 479 is not an Error variant; the float copy indexes 480 samples.
        assert!(valid.is_ok(), "480-frame capture should succeed");
        assert!(
            capture_text.starts_with("panic"),
            "capture 479 should panic, got {capture_text}"
        );
        assert!(
            render_text.starts_with("panic"),
            "render 479 should panic, got {render_text}"
        );
    }

    #[test]
    fn s3_live_gain_matches_when_level_adjustment_is_configured() {
        // Arrange
        let amplitude = 0.2_f32;

        // Act
        let with_adjustment = measure_gain_steps(true, amplitude);
        let without_adjustment = measure_gain_steps(false, amplitude);
        for step in &with_adjustment {
            eprintln!(
                "S3 cla=true gain={:.1} peak={:.5} expected={:.5} join_step={:.5} transition_step={:.5} steady_step={:.5}",
                step.gain,
                step.peak,
                amplitude * step.gain,
                step.join_step,
                step.transition_step,
                step.steady_step
            );
        }
        for step in &without_adjustment {
            eprintln!(
                "S3 cla=false gain={:.1} peak={:.5} expected_if_applied={:.5} join_step={:.5}",
                step.gain,
                step.peak,
                amplitude * step.gain,
                step.join_step
            );
        }

        // Assert
        for step in &with_adjustment {
            let expected = amplitude * step.gain;
            assert!(
                (step.peak - expected).abs() < 0.02,
                "cla gain {} peak {} expected {}",
                step.gain,
                step.peak,
                expected
            );
            assert!(
                step.join_step < 0.05,
                "join step {} at gain {}",
                step.join_step,
                step.gain
            );
            assert!(step.peak.is_finite());
        }
        for step in &without_adjustment {
            assert!(
                (step.peak - amplitude).abs() < 0.01,
                "without cla, gain {} still changed the peak to {}",
                step.gain,
                step.peak
            );
        }
    }

    #[test]
    fn s4_hot_input_hard_clips_above_full_scale() {
        // Arrange
        let amplitude = 0.9_f32;
        let mut apm = gain_apm(true);
        let mut frame_index = 0_usize;
        for _ in 0..3 {
            let _ = process_capture(&mut apm, &sine_frame(frame_index, amplitude));
            frame_index += 1;
        }
        let unity = peak_abs(&process_capture(
            &mut apm,
            &sine_frame(frame_index, amplitude),
        ));
        frame_index += 1;

        // Act
        apm.set_capture_post_gain(4.0);
        let _ramp = process_capture(&mut apm, &sine_frame(frame_index, amplitude));
        frame_index += 1;
        let clipped = process_capture(&mut apm, &sine_frame(frame_index, amplitude));
        let peak = peak_abs(&clipped);
        let rail = clipped
            .iter()
            .filter(|sample| sample.abs() >= 0.999)
            .count();
        let soft = crate::domain::gain::soft_limit(amplitude * 4.0);
        let finite = clipped.iter().all(|sample| sample.is_finite());
        eprintln!(
            "S4 unity_peak={unity:.8} gain4_peak={peak:.8} rail_samples={rail} soft_limit_of_3_6={soft:.8}"
        );

        // Assert — hard clip is a known difference from soft_limit, and unity stays usable.
        assert!(finite, "clipped frame contains non-finite samples");
        assert!(
            (unity - amplitude).abs() < 0.02,
            "unity hot tone peak {unity}"
        );
        assert!(peak > 0.999, "expected a full-scale rail, peak {peak}");
        assert!(peak <= 1.0, "peak exceeded full scale: {peak}");
        assert!(
            peak > soft + 0.02,
            "sonora peak {peak} should sit above soft_limit {soft}"
        );
        assert!(rail > 0, "no samples sat on the rail");
    }

    #[test]
    fn s5_impulse_delay_per_instance() {
        // Arrange — each instance is built inside the measurement.

        // Act
        let gain = measure_impulse_delay(false);
        let aec = measure_impulse_delay(true);
        eprintln!(
            "S5 gain_onset={} gain_peak_delay={} gain_peak={:.5} gain_proc_hz={} aec_onset={} aec_peak_delay={} aec_peak={:.5} aec_proc_hz={}",
            gain.onset_samples,
            gain.peak_samples,
            gain.peak,
            gain.proc_rate_hz,
            aec.onset_samples,
            aec.peak_samples,
            aec.peak,
            aec.proc_rate_hz
        );

        // Assert — a missing impulse fails. 20 ms is the noticeable-lag bound used here.
        assert!(gain.peak > 0.2, "gain impulse peak {}", gain.peak);
        assert!(aec.peak > 0.05, "aec impulse peak {}", aec.peak);
        assert!(
            gain.peak_samples.abs() < 960,
            "gain delay {}",
            gain.peak_samples
        );
        assert!(
            aec.peak_samples.abs() < 960,
            "aec delay {}",
            aec.peak_samples
        );
    }

    #[test]
    fn s7_stereo_render_mono_capture_uses_default_pipeline() {
        // Arrange
        let mut apm = aec_apm();
        let left: Vec<f32> = (0..FRAME)
            .map(|index| (index as f32 / 40.0).sin() * 0.3)
            .collect();
        let right: Vec<f32> = (0..FRAME)
            .map(|index| (index as f32 / 25.0).cos() * 0.15)
            .collect();
        let mic = vec![0.05; FRAME];
        let mut left_out = vec![0.0; FRAME];
        let mut right_out = vec![0.0; FRAME];
        let mut mic_out = vec![0.0; FRAME];

        // Act
        let render = apm.process_render_f32(&[&left, &right], &mut [&mut left_out, &mut right_out]);
        let delay = apm.set_stream_delay_ms(0);
        let capture = apm.process_capture_f32(&[&mic], &mut [&mut mic_out]);
        let pipeline = apm.config().pipeline.clone();
        let max_rate = match pipeline.maximum_internal_processing_rate {
            MaxProcessingRate::Rate32kHz => 32_000,
            MaxProcessingRate::Rate48kHz => 48_000,
        };
        eprintln!(
            "S7 render={render:?} delay={delay:?} capture={capture:?} max_rate={max_rate} multi_render={} multi_capture={} downmix={:?} proc_hz={}",
            pipeline.multi_channel_render,
            pipeline.multi_channel_capture,
            pipeline.capture_downmix_method,
            apm.proc_sample_rate_hz()
        );

        // Assert — `Pipeline::default` in sonora 0.2.0.
        assert!(render.is_ok());
        assert!(delay.is_ok());
        assert!(capture.is_ok());
        assert!(mic_out.iter().all(|sample| sample.is_finite()));
        assert_eq!(
            pipeline.maximum_internal_processing_rate,
            MaxProcessingRate::Rate32kHz
        );
        assert!(pipeline.multi_channel_render);
        assert!(pipeline.multi_channel_capture);
        assert_eq!(
            pipeline.capture_downmix_method,
            DownmixMethod::AverageChannels
        );
    }

    #[test]
    #[ignore = "AEC convergence over several seconds of synthetic audio"]
    fn s6_aec_with_zero_stream_delay() {
        // Arrange
        let delays = [20_u32, 100, 300];

        // Act
        let windows: Vec<AecWindow> = delays
            .into_iter()
            .map(|delay| run_aec_window(delay, 8, 3, RenderSide::Both, false))
            .collect();
        let left = run_aec_window(100, 4, 0, RenderSide::Left, false);
        let right = run_aec_window(100, 4, 0, RenderSide::Right, false);
        let voice = run_aec_window(0, 0, 3, RenderSide::Both, true);
        for window in windows.iter().chain([&left, &right, &voice]) {
            eprintln!(
                "S6 delay_ms={} channel={} silence_render={} erle_db={:.2} speech_corr={:.3} speech_lag={} echo_corr={:.3} echo_lag={} speech_energy_ratio={:.3} stats_erle={:?} stats_delay_ms={:?}",
                window.delay_ms,
                window.render_side.label(),
                window.silence_render,
                window.erle_db,
                window.speech_correlation,
                window.speech_lag_samples,
                window.echo_correlation,
                window.echo_lag_samples,
                window.speech_energy_ratio,
                window.stats_erle_db,
                window.stats_delay_ms
            );
        }

        // Assert — full-band energy ERLE, not the capped statistics() value.
        for window in &windows {
            assert!(
                window.erle_db > 20.0,
                "delay {} erle {}",
                window.delay_ms,
                window.erle_db
            );
            assert!(
                window.speech_energy_ratio > 0.05,
                "speech energy {}",
                window.speech_energy_ratio
            );
            assert!(
                window.echo_correlation.abs() < 0.1,
                "echo still present {}",
                window.echo_correlation
            );
        }
        assert!(left.erle_db > 10.0, "left erle {}", left.erle_db);
        assert!(right.erle_db > 10.0, "right erle {}", right.erle_db);
        assert!(
            voice.speech_energy_ratio > 0.5,
            "silent-render speech energy {}",
            voice.speech_energy_ratio
        );
    }

    #[test]
    #[ignore = "release timing of 60s of 10ms frames"]
    fn s8_release_cost_of_aec_gain_and_resamplers() {
        // Arrange
        let mut aec = aec_apm();
        let mut gain = gain_apm(true);
        gain.set_capture_post_gain(2.0);
        let mut mic_resampler = StereoResampler::new(RATE, RATE);
        let mut system_resampler = StereoResampler::new(RATE, RATE);
        let mut noise = XorShift64::new(0xC0FFEE);
        let mut stereo = vec![0.0; FRAME * 2];
        let mut left = vec![0.0; FRAME];
        let mut right = vec![0.0; FRAME];
        let mut mic = vec![0.0; FRAME];
        for index in 0..FRAME {
            let sample = noise.next_unit() * 0.2;
            stereo[index * 2] = sample;
            stereo[index * 2 + 1] = sample * 0.8;
            left[index] = sample;
            right[index] = sample * 0.8;
            mic[index] = sample * 0.5;
        }
        let mut render_left = vec![0.0; FRAME];
        let mut render_right = vec![0.0; FRAME];
        let mut mic_out = vec![0.0; FRAME];
        let mut gain_out = vec![0.0; FRAME];
        for _ in 0..200 {
            let _ = mic_resampler.process(&stereo);
            let _ = system_resampler.process(&stereo);
            aec.process_render_f32(&[&left, &right], &mut [&mut render_left, &mut render_right])
                .expect("warmup render");
            aec.set_stream_delay_ms(0).expect("warmup delay");
            aec.process_capture_f32(&[&mic], &mut [&mut mic_out])
                .expect("warmup capture");
            gain.process_capture_f32(&[&mic_out], &mut [&mut gain_out])
                .expect("warmup gain");
        }

        // Act
        let frames = 6_000_u32;
        let started = Instant::now();
        for _ in 0..frames {
            let mic_audio = mic_resampler.process(&stereo);
            let system_audio = system_resampler.process(&stereo);
            aec.process_render_f32(&[&left, &right], &mut [&mut render_left, &mut render_right])
                .expect("render");
            aec.set_stream_delay_ms(0).expect("delay");
            aec.process_capture_f32(&[&mic], &mut [&mut mic_out])
                .expect("capture");
            gain.process_capture_f32(&[&mic_out], &mut [&mut gain_out])
                .expect("gain");
            std::hint::black_box(&mic_audio);
            std::hint::black_box(&system_audio);
            std::hint::black_box(&mic_out);
            std::hint::black_box(&gain_out);
        }
        let elapsed = started.elapsed();
        let average_us = elapsed.as_secs_f64() * 1_000_000.0 / f64::from(frames);
        eprintln!(
            "S8 frames={frames} elapsed_ms={:.1} avg_us_per_10ms_frame={average_us:.1} avg_ms={:.4}",
            elapsed.as_secs_f64() * 1_000.0,
            average_us / 1_000.0
        );

        // Assert — record the number even if it misses the 2 ms proposal.
        assert!(average_us.is_finite());
        assert!(average_us < 50_000.0, "frame cost blew up: {average_us} us");
    }

    #[test]
    #[ignore = "short device probe; not the 30 minute S9 measurement"]
    fn s9_short_frame_index_versus_qpc() {
        // Arrange
        let seconds = 10_u64;

        // Act
        let report = probe_devices(Duration::from_secs(seconds));

        // Assert — counts only. The delta is not a ppm measurement.
        match report {
            ProbeReport::Opened(sources) => {
                for source in &sources {
                    eprintln!(
                        "S9 source={} duration_s={seconds} frame_index_advance={} qpc_frames={} delta_frames={} chunks={} max_abs_drift_error={}",
                        source.name,
                        source.frame_index_advance,
                        source.qpc_frames,
                        source.delta_frames,
                        source.chunks,
                        source.max_abs_drift_error
                    );
                }
                assert!(!sources.is_empty());
            }
            ProbeReport::Unavailable(message) => {
                eprintln!("S9 probe_unavailable={message}");
            }
        }
    }
}
