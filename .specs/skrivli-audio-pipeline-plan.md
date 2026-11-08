# Skrivli audio pipeline refactor plan

> **2026-11-08:** recordings are FLAC-only now (D-29, `skrivli-flac-migration-plan.md`). WAV, `.wav.partial`, `hound` and RIFF mentions below describe the earlier design; read `.flac` / `.flac.partial` / `flac-codec` where they appear.

Status: **Phases S (S1–S8) and 1–7 are done. Phase 8 updated the specs and added the sonora license notice. S9's 30-minute hardware drift measurement was not run, and the [section 11](#hardware-checks) hardware checks were not run.** Section 2 describes the pipeline this refactor replaced. `mixer.rs` is deleted. The phase checklists in [section 10](#10-migration-phases) track progress.

**Goal in one sentence:** replace the all-in-one `Mixer` and the resampler hidden in the capture workers with one pipeline of swappable steps, use [sonora](https://github.com/dignifiedquire/sonora) for echo cancellation and microphone gain, and make audio readable at any processing point for meters and statistics.

---

## 1. Why we are doing this

### The user problem

When someone records a meeting on speakers, the microphone picks up the other participants from the speakers. The recording already contains those voices from system audio, so they appear twice: once clean and once delayed and roomy. This hurts both listening and transcription. Today our only answer is "recommend headphones".

Acoustic echo cancellation (AEC) fixes this. It uses system audio as a *reference* of what is playing, finds that signal in the microphone, and removes it. Our own voice stays.

### The code problem

[`mixer.rs`](../src/infrastructure/audio/mixer.rs) does five jobs in one type:

1. Places packets on the session timeline (`Track`).
2. Applies microphone gain and AGC.
3. Applies system gain.
4. Sums and limits.
5. Measures peaks for the meters.

Adding AEC there would add a sixth job. Peak measurement is written directly into the mix loop, so measuring anything new means editing that loop too.

The other part of the audio path is split across threads. The resampler runs in the capture worker, the mixer calculates the drift error on the session thread, and an `AtomicI64` carries the error back. Every `CapturePort` has to pass that atomic through, including the idle meter, which ignores it.

### Decisions already made

| Topic | Decision | Consequence |
| --- | --- | --- |
| AEC control | **Always on**, no setting | No new UI. There is no user off switch, see risk R2. |
| Auto gain | **Removed** | Delete `Agc`, the Auto checkbox, and the `agc_enabled` setting. |
| Sonora usage | **Two separate steps, two instances** | Audio is readable between AEC and gain, and each step can be replaced alone. |
| Microphone layout | **Mono inside the pipeline** | The WAV stays stereo. A true stereo mic loses its stereo image. |
| Mixer | **In the pipeline, split in two** | Timeline work becomes `Aligner`; summing becomes an `AudioMixer` step. |
| Resampler | **In the pipeline, as a per-source input stage** | It moves from the capture workers to the pipeline thread. It is not an `AudioProcessor` ([section 4.5](#45-driftresampler-the-input-stage)). |

### Non-goals

- No noise suppression, high-pass filter step, or analog mic control. The design leaves room to add them later.
- No general audio graph. The shape is fixed: two inputs, three chains, one output.
- No adding or removing steps during a recording. A pipeline is built once per session.
- No change to the drift correction *algorithm* during this refactor. Moving the resampler keeps its behavior. Spike S9 checks whether the algorithm sees real drift, and any fix is separate work.
- No change to device opening, the WAV format, the sidecar schema version, or transcription.
- No processing of existing recordings.

---

## 2. Current pipeline

```text
Mic worker:    flexaudio ─ Rubato ─ queue ──┐
                              ▲             ├─ session loop ─ Mixer ─ WAV writer
System worker: flexaudio ─ Rubato ─ queue ──┘                  │  │
                              ▲                                │  └─ LevelSnapshot ─ UI at ~30 Hz
                              └────── drift error (AtomicI64) ─┘
```

These facts drive the design. Each one was checked in the code:

- **Everything after the queues runs on one thread.** `session_body` in [`recording.rs`](../src/services/recording.rs) pulls packets, calls `Mixer::pull`, and writes the WAV in one loop. No locks are needed around sonora.
- **flexaudio already delivers 48 kHz stereo `f32`.** It does the device-rate conversion, and mono mics are already duplicated onto both channels. Chunks are 20 ms.
- **Our resampler only corrects drift.** [`flex_capture.rs`](../src/infrastructure/audio/flex_capture.rs) builds `StereoResampler::new(48_000, 48_000)`, so the only thing it changes is the ratio nudge. D-06 still describes it as converting the device rate.
- **The mixer already produces aligned 10 ms blocks.** Both sources are on one timeline, which is exactly what AEC needs. Alignment must therefore stay *before* AEC.
- **Tail blocks can be shorter than 10 ms.** During stop, `pull` may return fewer than 480 frames. Sonora works on 10 ms chunks, so this needs handling ([section 7](#7-one-10-ms-frame-step-by-step)).
- **The idle meter is separate.** [`level_monitor.rs`](../src/services/level_monitor.rs) opens devices when not recording and calculates `peak × slider` itself.
- **Stop drains every layer (D-04).** The worker polls flexaudio's remaining chunks and flushes the resampler, then the session pulls the queue empty before the tail mix. Moving the resampler must keep this, so the pipeline flushes it when the queues are empty ([section 4.5](#45-driftresampler-the-input-stage)).
- **The drift loop may not see real clock drift.** `map_timeline` computes `base + frame_index`, and flexaudio documents `frame_index` as a count of frames produced, while `pts_ns` is "Not a media clock". So the error may only measure our own resampler against that count, not the device clock against QPC. This comes from reading the code and flexaudio's docs, and it has not been measured. Spike S9 settles it.

---

## 3. Target pipeline

```text
 INPUT STAGE: variable-size blocks, one resampler per source
 ───────────────────────────────────────────────────────────
 Mic worker ──queue──▶ DriftResampler ─┐        ┌──────────┐
                            ▲          ├──────▶ │ Aligner  │
 Sys worker ──queue──▶ DriftResampler ─┘        └────┬─────┘
                            ▲                        │
                            └── drift error ─────────┤
                                                     │
 PROCESSING STAGE: fixed 10 ms frames                │
 ───────────────────────────────────                 │
        ┌── mic stereo ── downmix ───────────────────┤
        ▼                                            │ system stereo
   MIC CHAIN (mono)                                  │
   ├ SonoraEchoCanceller ◀──── echo reference ───────┤
   ├ ◉ optional tap: clean, pre-gain                 │
   ├ SonoraMicGain                                   ▼
   └ ◉ PeakTap → mic meter                    SYSTEM CHAIN (stereo)
        │                                     ├ FixedGain(0.8)
        │                                     └ ◉ PeakTap → system meter
        │                                            │
        └──────────▶ AudioMixer (SumMixer) ◀─────────┘
                            │
                     MIX CHAIN (stereo)
                     ├ SoftLimiter
                     └ ◉ PeakTap → mixed meter
                            │
                            ▼
                        WAV writer
```

`◉` marks a tap. A tap can go between any two steps in any chain.

The two stages exist because the audio has different shapes. Before the aligner, blocks vary in size and can overlap or have holes. After it, every frame is exactly 10 ms and both sources line up. **Taps and processors live only in the second stage**, so that shape is guaranteed for them. The earliest point you can read is therefore right after alignment.

Capture workers become simple: flexaudio chunk, timeline label, queue. They need no drift atomic, no Rubato, and less CPU, which leaves them less likely to overrun.

---

## 4. The concepts

An engineer needs only these few ideas to understand the pipeline.

Code here is illustrative and has not been compiled. Sonora signatures come from the 0.2.0 docs.

### 4.1 `AudioFrame`: 10 ms of audio

Every step receives the same type. Size is fixed, so steps do not handle partial buffers.

```rust
/// 10 ms at 48 kHz.
pub(crate) const FRAME_LEN: usize = 480;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChannelLayout { Mono, Stereo }

pub(crate) struct AudioFrame {
    layout: ChannelLayout,
    samples: Vec<f32>,   // always FRAME_LEN × channels; stereo is interleaved
    valid_frames: usize, // < FRAME_LEN only for the padded tail at stop
    timeline_frame: u64, // position of the first frame in the session
}
```

| Rule | Reason |
| --- | --- |
| Always exactly `FRAME_LEN` frames | Sonora needs 10 ms chunks, and processors stay simple. |
| `valid_frames` tracks real audio | A short tail is padded with silence for processing, then trimmed at output, so the recording length does not change. |
| Frames are reused | The pipeline owns its buffers, so normal processing does not allocate per frame. |
| Fields are private | Constructors validate, as `AGENTS.md` requires. |

### 4.2 `AudioProcessor`: a step that changes audio

```rust
pub(crate) struct ProcessContext<'a> {
    /// Raw system audio for this same 10 ms. `None` outside the mic chain.
    pub(crate) echo_reference: Option<&'a AudioFrame>,
}

pub(crate) trait AudioProcessor: Send {
    /// Used in logs and diagnostics. Never includes audio data.
    fn name(&self) -> &'static str;

    /// On `Err`, the frame must be left unchanged.
    fn process(
        &mut self,
        frame: &mut AudioFrame,
        context: &ProcessContext<'_>,
    ) -> Result<(), ProcessError>;
}
```

Implementations: `SonoraEchoCanceller`, `SonoraMicGain`, `FixedGain`, `SoftLimiter`.

### 4.3 `AudioTap`: a point where audio can be read

```rust
pub(crate) trait AudioTap: Send {
    /// Runs on the pipeline thread, once per frame. Must not block.
    fn observe(&mut self, frame: &AudioFrame);
}
```

A tap receives `&AudioFrame`, so the compiler guarantees it cannot change the audio. Adding a meter can never alter a recording.

| Tap | Purpose | How data leaves |
| --- | --- | --- |
| `PeakTap` | Level meters | The existing `LevelSnapshot` atomics, which the UI already polls |
| `StatsTap` | RMS, peak, clipped-sample count | Shared atomics read when needed |
| `ChannelTap` | Consumers that need the samples, such as future mic/system stems | Bounded `sync_channel` with `try_send`; a full queue drops the frame and counts it |

`ChannelTap` follows the same rule as the capture queue (D-04): the audio thread never waits for a consumer.

The privacy rule applies: taps may log counts and levels, but never samples.

### 4.4 `Chain`: an ordered list of steps

```rust
enum Step {
    Process(Box<dyn AudioProcessor>),
    Tap(Box<dyn AudioTap>),
}

pub(crate) struct Chain { steps: Vec<Step> }

impl Chain {
    pub(crate) fn new() -> Self;
    pub(crate) fn process(self, processor: impl AudioProcessor + 'static) -> Self;
    pub(crate) fn tap(self, tap: impl AudioTap + 'static) -> Self;
    fn run(&mut self, frame: &mut AudioFrame, context: &ProcessContext<'_>);
}
```

Because taps and processors share one list, **the order in the code is the order the audio takes**. No tap registry or tap-point enum is needed.

`run` returns nothing on purpose. A failing step must not end a recording ([section 8](#8-failure-behavior)).

### 4.5 `DriftResampler`: the input stage

```rust
pub(crate) trait DriftResampler: Send {
    /// Timeline error in frames. Positive means the source is running slow.
    fn set_drift_error(&mut self, frames: i64);
    /// Takes any block size and returns whatever output is ready, possibly none.
    fn process(&mut self, interleaved: &[f32]) -> Vec<f32>;
    /// Returns the audio still held. Called once at a normal stop, after the queues are empty.
    fn flush(&mut self) -> Vec<f32>;
    /// Drops all state. Called when a source is reopened.
    fn reset(&mut self);
}
```

`StereoResampler` (Rubato) is the real implementation and only needs `reset` added. Tests use a pass-through, so existing tests that check exact samples keep working; those fake sources bypass Rubato today as well.

**Why it is not an `AudioProcessor`:**

- **Its output size varies.** A processor gets exactly one frame and returns exactly one frame. Drift correction changes the frame count; that is its whole job.
- **It has to run before alignment.** The aligner places blocks by counting the frames it receives, so it needs the corrected count.
- **It works per source, and its error comes from the aligner.** That loop needs a place outside the chains.

**Why move it from the capture workers:**

- The whole audio path can be read in one place.
- The drift loop becomes a function argument on one thread instead of an atomic crossing threads. It can be tested end to end, without threads.
- `CapturePort::open` loses its `drift_error` parameter, and the idle meter stops running a resampler it doesn't need.
- If S9 finds that the drift error needs to compare against the session clock, the pipeline already has that clock in `pull`.

### 4.6 `AudioMixer`: combining two sources

```rust
pub(crate) trait AudioMixer: Send {
    fn mix(&mut self, mic: &AudioFrame, system: &AudioFrame, output: &mut AudioFrame);
}
```

`SumMixer` is the only implementation at first: `output = mic + system` per channel, with the mono mic going to both. It has two inputs, so it can't be a chain step. The trait keeps summing a named, separately tested operation, and alternatives such as mic-left/system-right can be added without touching `Pipeline`.

The limiter stays out of the mixer, as the first step of the mix chain.

### 4.7 `Pipeline`: the fixed shape

```rust
pub(crate) struct Pipeline {
    mic_resampler: Box<dyn DriftResampler>,
    system_resampler: Box<dyn DriftResampler>,
    aligner: Aligner,
    mic: Chain,
    system: Chain,
    mixer: Box<dyn AudioMixer>,
    mix: Chain,
    // Reused frames: mic_frame (mono), system_frame, mix_frame (stereo).
}

impl Pipeline {
    pub(crate) fn push_mic(&mut self, block: AudioBlock);
    pub(crate) fn push_system(&mut self, block: AudioBlock);
    /// Clears the source's resampler state after its stream is reopened.
    pub(crate) fn source_reopened(&mut self, source: AudioSource);
    /// Flushes both resamplers into the aligner at a normal stop.
    pub(crate) fn finish_input(&mut self);
    /// The next samples to write, or `None` if no frame is ready.
    pub(crate) fn pull(&mut self, now_frame: u64, tail: bool) -> Option<&[f32]>;
    pub(crate) fn diagnostics(&self) -> PipelineDiagnostics;
}
```

Apart from `source_reopened` and `finish_input`, these match today's `Mixer` methods, so the session loop barely changes.

### 4.8 How it reads when assembled

This one function is the live description of the pipeline. Reading it should be enough to know what happens to the audio.

```rust
impl PipelineFactory for SonoraPipelineFactory {
    fn build(&self, setup: PipelineSetup) -> Result<Pipeline, PipelineError> {
        let mut mic = Chain::new();
        if setup.system_active {
            mic = mic.process(SonoraEchoCanceller::new()?);
        }
        let mic = mic
            .process(SonoraMicGain::new(setup.gain)?)
            .tap(PeakTap::new(Arc::clone(&setup.levels), MeterSlot::Mic));

        let system = Chain::new()
            .process(FixedGain::new(SYSTEM_GAIN))
            .tap(PeakTap::new(Arc::clone(&setup.levels), MeterSlot::System));

        let mix = Chain::new()
            .process(SoftLimiter::new(setup.limited_samples))
            .tap(PeakTap::new(setup.levels, MeterSlot::Mixed));

        Ok(Pipeline::new(PipelineParts {
            mic_resampler: Box::new(StereoResampler::new(48_000, 48_000)),
            system_resampler: Box::new(StereoResampler::new(48_000, 48_000)),
            aligner: Aligner::new(setup.system_active),
            mic,
            system,
            mixer: Box::new(SumMixer),
            mix,
        }))
    }
}
```

`PipelineFactory` is a port, injected through `RecordingEngine` like `CapturePort`. The real app builds Rubato and sonora steps. Tests use a factory with a pass-through resampler and simple gain steps, so the many existing tests that check exact sample values keep working.

---

## 5. Sonora: what an engineer needs to know

Sonora is a pure Rust port of the WebRTC audio processing module. Version 0.2.0, BSD-3-Clause, needs Rust 1.91 or newer. Our toolchain is `stable`, currently 1.99.1.

### Concepts

**Capture and render.** WebRTC names the two directions from a call's point of view:

- *Capture* is the microphone, which we want to clean.
- *Render* is what is being played. For us, that is loopback system audio.

**The call order matters.** For each 10 ms, give it render first, then capture. AEC needs the reference before it can remove that signal:

```rust
apm.process_render_f32(&[&left, &right], &mut [&mut out_left, &mut out_right])?;
apm.set_stream_delay_ms(STREAM_DELAY_MS)?; // docs: required when AEC is enabled
apm.process_capture_f32(&[&mic], &mut [&mut mic_out])?;
```

**f32 buffers are split per channel.** Sonora's float API takes one slice per channel, while our stereo is interleaved. The adapter converts using reusable scratch buffers.

**Stream delay is a hint.** AEC3 estimates the real delay itself. Our timeline already aligns the two sources, so we start from a constant `0` and let the estimator find the room and output-device delay. Spike S6 validates this.

**AEC takes time to adapt.** Expect some audible echo in the first moments of a session and after the echo path changes. The AEC needs system audio playing in order to learn.

### Why two instances

One instance runs AEC and gain within a single `process_capture_f32` call, so nothing can be read between them. With two instances:

| Instance | Config | Render input |
| --- | --- | --- |
| `SonoraEchoCanceller` | `echo_canceller: Some(default)`, capture 48 kHz mono, render 48 kHz stereo | Yes, every frame |
| `SonoraMicGain` | `capture_level_adjustment: Some(..)`, capture 48 kHz mono, no other components | No |

```rust
let aec = AudioProcessing::builder()
    .config(Config {
        echo_canceller: Some(EchoCanceller::default()),
        ..Default::default()
    })
    .capture_config(StreamConfig::new(48_000, 1))
    .render_config(StreamConfig::new(48_000, 2))
    .build();
```

Order is AEC, then gain. AEC models how the speaker signal reaches the mic, and that works best on the raw mic signal. Moving the gain slider should not disturb it.

### Gain slider

`SonoraMicGain` reads the existing lock-free `GainControl` once per frame and calls sonora only when the value changes:

```rust
let slider = self.gain.slider();
if slider != self.applied {
    self.apm.set_capture_post_gain(slider);
    self.applied = slider;
}
```

The setter docs say it applies at the next call. Live slider changes during recording remain supported, as the product spec promises.

### What we do not use

- **`set_capture_fixed_post_gain`:** its range is 0 to 90 dB, so it cannot lower the volume. Our slider runs from `0.0×` to `4.0×`.
- **`pre_amplifier`:** the docs mark it as legacy and say it should not be combined with `capture_level_adjustment`.
- **`gain_controller2`:** this is sonora's AGC and limiter. We removed Auto, and the mix keeps its own limiter because sonora only sees the mic.

### Not verified yet

Phase S measured these. The numbers are in [D-13](decisions.md). S9 (30 minutes of real clock drift) is not measured. Production code still must not assume anything beyond D-13.

- `set_capture_post_gain` applies on the next capture frame only when `capture_level_adjustment` was configured. Otherwise it is a no-op.
- Gain `0.0` is silence after the one-frame ramp. Gain above full scale hard-clips at ±1. `soft_limit` does not.
- A 479-frame buffer panics. It is not an `Error`. Padding to 480 is mandatory.
- Default `Pipeline` is 32 kHz max, stereo render allowed, capture downmix averages. Stereo render with mono capture is accepted. The AEC instance therefore runs at 32 kHz.
- Gain adds no delay. The AEC instance adds about 9 ms at the default 32 kHz cap.

---

## 6. Reading audio at any point

The requirement is a single line at the place you want to measure. The recipe is the same for every tap.

**Example: a meter for the clean mic before gain.**

```rust
let mic = Chain::new()
    .process(SonoraEchoCanceller::new()?)
    .tap(PeakTap::new(levels, MeterSlot::MicClean)) // ← new line
    .process(SonoraMicGain::new(gain)?);
```

**Example: statistics on raw system audio.**

```rust
let system = Chain::new()
    .tap(StatsTap::new(system_input_stats)) // before any processing
    .process(FixedGain::new(SYSTEM_GAIN));
```

**Example: add a new operation, such as noise suppression.**

1. Write a failing test for the behavior.
2. Add `struct SonoraNoiseSuppressor` implementing `AudioProcessor`.
3. Add `.process(SonoraNoiseSuppressor::new()?)` to the chain.

`Pipeline`, `Chain`, and the session loop do not change.

### Meter meaning

| Meter | Reads from | Change from today |
| --- | --- | --- |
| Mic | After AEC and gain | Speaker bleed no longer moves the mic meter during a recording |
| System | After `FixedGain(0.8)` | None |
| Mixed | After the limiter | None |

**Idle meters stay outside the pipeline.** `level_monitor.rs` still calculates `peak × slider`, because AEC is pointless before a recording and the monitor does not align sources. So the mic meter can read slightly differently once a recording starts on speakers.

---

## 7. One packet and one 10 ms frame, step by step

`Pipeline::push_mic(block)` and `push_system(block)` do this for each packet from a queue:

1. **Read the error.** Get this source's current drift error from `Aligner`, and give it to the resampler. This replaces the `AtomicI64`.
2. **Resample.** If there is no output yet, keep the block's `discontinuity` flag for the next output. Today's worker loses that flag when output is empty, unless the gap came from dropped chunks, so this is a small fix.
3. **Place on the timeline.** `Aligner` applies `plan_packet` exactly as `Track::push` does today.

`Pipeline::pull(now, tail)` then does this for each frame:

1. **Align.** `Aligner` returns 480 frames per source, or `None` if it should wait. Waiting, the 150 ms lateness rule, and silence filling are **moved unchanged** from `Mixer`.
2. **Pad the tail.** During stop, a short tail is padded with silence and `valid_frames` records the real length.
3. **Downmix the mic.** `mono = (L + R) / 2`. A duplicated mono mic is unchanged: `(x + x) / 2 = x`.
4. **Run the mic chain.** `echo_reference` is the raw system frame.
5. **Run the system chain.** It runs *after* the mic chain, because it applies `0.8` in place. **AEC must see system audio before our gain changes it.** This order avoids a per-frame copy, and a test locks it in.
6. **Mix.** `AudioMixer` combines the two frames into the stereo mix frame.
7. **Run the mix chain.** Limiter, then the mixed meter.
8. **Return.** The first `valid_frames × 2` samples go to `wav.write_samples`, so a padded tail never lengthens the recording.

**Reopening a source.** A new flexaudio stream starts its own `frame_index` and today gets a fresh worker and resampler. With the resampler in the pipeline, `try_reopen_sources` must call `pipeline.source_reopened(..)` so the same reset still happens. The aligner state is untouched, as it is today.

If the system source was never requested (`system_active == false`), the AEC step is not built. If it was requested but drops mid-session, the aligner supplies silence, AEC gets a silent reference and has nothing to remove, and reopening works as it does today.

---

## 8. Failure behavior

**A processing failure must never lose a recording.** A slightly worse recording is always better than none.

| Situation | Behavior |
| --- | --- |
| `process` returns `Err` | The frame keeps its previous audio. Count it, log once with the step name, and continue. |
| `process` panics | `catch_unwind`, as `pull_alive` already does for capture. **Disable that step for the rest of the session**, and continue. |
| Sonora fails at session start | Log it, skip that step, record the problem, and record anyway. |
| Resampler returns nothing because of an internal Rubato error | Same as today: no output for that call. The aligner fills the hole with silence and counts a gap. |
| Resampler panics | Today a worker panic ends only that source. Now `catch_unwind` around the call, and the source falls back to passing audio through uncorrected for the session. |
| Slow tap consumer | `ChannelTap` drops and counts. Audio is unaffected. |

Add a sidecar problem code `audio_processing` through the existing `note(&mut problems, ..)` path. This uses the existing string list and does not change the schema.

The mic gain step needs one extra consideration. If it fails, audio passes at unity gain rather than at the slider value. That is acceptable for a rare failure, and the sidecar code records it.

At session end, `SonoraEchoCanceller` logs `echo_return_loss_enhancement` and `delay_ms` from `statistics()`. These are numbers only, and are useful for diagnosing field reports.

---

## 9. User-visible changes and file map

### Changes

| Change | Detail |
| --- | --- |
| Echo removal | Always on when system audio is being captured. |
| Auto checkbox removed | The gain slider is always enabled during capture, except while saving. |
| Old settings | A saved `agc_enabled` is ignored on load. The stored `mic_gain` applies, including for users who had Auto on. |
| Mono mic | The two WAV channels carry the same mic signal. For a true stereo mic, a signal on only one side drops by 6 dB. |

### Files

| Path | Change |
| --- | --- |
| `Cargo.toml`, `Cargo.lock` | Add `sonora = "=0.2.0"`. Pin exactly, because it is pre-1.0. |
| `src/domain/audio_frame.rs` | **New.** `AudioFrame`, `ChannelLayout`, `FRAME_LEN` |
| `src/domain/gain.rs` | Remove `Agc`. Keep clamp, meter helpers, and `soft_limit`. |
| `src/services/ports.rs` | Add `AudioProcessor`, `AudioTap`, `ProcessContext`, `DriftResampler`, `AudioMixer`, `PipelineFactory`. Remove `drift_error` from `CapturePort::open`. |
| `src/services/audio_pipeline/mod.rs` | **New.** `Pipeline` |
| `src/services/audio_pipeline/chain.rs` | **New.** `Chain` and failure handling |
| `src/services/audio_pipeline/aligner.rs` | **New.** `Track` logic moved from `mixer.rs`; exposes the drift error instead of an atomic |
| `src/services/audio_pipeline/steps.rs` | **New.** `SumMixer`, `FixedGain`, `SoftLimiter`, `StatsTap`, `ChannelTap` |
| `src/infrastructure/audio/resampler.rs` | Implement `DriftResampler`; add `reset`. |
| `src/infrastructure/audio/flex_capture.rs` | Remove Rubato, the drift atomic, and the flush from the worker. Keep polling flexaudio's remaining chunks at stop. |
| `src/infrastructure/audio/sonora_steps.rs` | **New.** `SonoraEchoCanceller`, `SonoraMicGain`, `SonoraPipelineFactory`. Production code imports `sonora` only in this file. `sonora_spike.rs` is `#[cfg(test)]` measurement code and also imports it. |
| `src/infrastructure/audio/levels.rs` | Add `PeakTap`, `MeterSlot`. Reduce `GainControl` to the slider. |
| `src/infrastructure/audio/mixer.rs` | **Deleted** once its tests have moved. |
| `src/services/recording.rs` | `Mixer` becomes `Pipeline`; the factory joins `RecordingEngine`; drift atomics go away; reopen calls `source_reopened`. |
| `src/services/recording_harness.rs` | Inject a test factory; update the `open` signature on fake ports. |
| `src/services/level_monitor.rs` | Adjust to the meter API and the `open` signature, including its fake ports. |
| `src/app/bootstrap.rs` | Wire `SonoraPipelineFactory`. |
| `src/infrastructure/storage/settings.rs` | Remove `agc_enabled`. |
| `src/views/recording.rs`, `src/views/main_window.rs` | Remove the checkbox, `set_agc`, and the `effective()` display. |

Traits live in `services/ports.rs` because services own use cases and depend on ports. `Pipeline` knows nothing about sonora, which keeps the dependency pointing the right way.

---

## 10. Migration phases

Each phase keeps the app working and is one reviewable change. Follow red, green, refactor, and mark Arrange, Act, and Assert in every test. `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` must stay clean after every phase.

### Phase S: sonora spike (blocking)

Answer unknowns before refactoring. Use a scratch example or ignored tests with synthetic signals. Record the results in `.specs/decisions.md` as **D-13**.

| # | Question | How | Needed result |
| --- | --- | --- | --- |
| S1 | Does it build on our target? | Build for `x86_64-pc-windows-msvc` | Builds; note binary size growth |
| S2 | Wrong frame length? | Feed 479 frames | Known behavior. If it panics, the padding rule in `Pipeline` is mandatory (planned anyway). |
| S3 | Does the gain setter work live? | Tone through a gain-only instance at `0.0`, `0.5`, `1.0`, `4.0`, changing mid-stream | Measured level matches, with no click |
| S4 | Does hot input hard-clip? | `0.9` tone at `4.0×` | Known. See the decision below. |
| S5 | Added delay per instance? | Impulse through each instance | Known and small. Mic must not noticeably lag system audio in the mix. |
| S6 | Does AEC work with delay `0`? | Mic = system delayed 20 to 300 ms and attenuated, plus separate speech | Clear echo reduction via ERLE; speech survives |
| S7 | Is stereo render with mono capture accepted? | Run it; read `Pipeline` defaults | Works; stereo mix-down policy is documented |
| S8 | CPU cost? | Time both instances and two resamplers for 60 s of frames in a release build | Record it. **Proposed target:** under 2 ms average per 10 ms frame on the dev machine; this is a proposal and not a measured number. |
| S9 | Does the drift loop see real clock drift? | On real hardware, for at least 30 minutes, log each source's `frame_index` advance against QPC, plus the current drift error | Known. Record ppm per device and whether the drift error follows it. |

**Decision gate.** If S3, S4, or S5 fails, sonora is unsuitable for the *gain* step. The fallback would be a small `LinearGain` processor behind the same trait, which is one line in the factory. Because the brief is to use sonora for gain, **raise this for a decision rather than switching silently**. If S6 fails, stop and reassess the whole AEC plan.

**If S9 shows real drift the loop can't see,** the two sources slowly separate during long recordings: at 100 ppm, that is 360 ms per hour. It would be an existing bug rather than something this refactor causes, but it matters more with AEC. Raise it for a decision, and fix it as its own change after Phase 4, when the resampler and the session clock sit together.

- [x] S1–S8 ran, with results in D-13. S9 was not measured.
- [ ] S9's 30-minute hardware drift measurement was not run. A 10-second probe during Phase S is not S9. The drift algorithm was not changed. If a later 30-minute run shows the loop cannot see real clock drift, that fix is a separate change. It is not part of this refactor.
- [x] Decision gates reviewed. Gain passes. AEC passes. S9 is not measured, so any drift fix stays a separate change.

### Phase 1: frame, traits, chain

No app behavior change.

- [x] Add `AudioFrame`, `AudioProcessor`, `AudioTap`, `Chain`
- [x] Add `AudioMixer`, `SumMixer`, `FixedGain`, `SoftLimiter`
- [x] Tests: step order, taps see audio at their position, `Err` leaves the frame unchanged and continues, a panicking step is disabled

### Phase 2: extract `Aligner`

A pure move with no behavior change. Keep `Mixer` as a thin wrapper for now.

- [x] Move `Track` and timing logic from `Mixer` to `Aligner`
- [x] Move timeline tests: start offset, 30 s of silent system audio, gaps and overlaps, late blocks, tail, overrun, 100 ppm drift

### Phase 3: `Pipeline` replaces `Mixer`, and Auto is removed

These go together because `Agc` lives in `Mixer::mix`. Use a simple test gain step for now, since sonora arrives in the next phases.

- [x] Add `Pipeline`, `PipelineFactory`, `PeakTap`, and a temporary `LinearGain`
- [x] Mono mic downmix and stereo output
- [x] Update `session_body` and the harness
- [x] Remove `Agc`, the setting, the checkbox, and `effective`
- [x] Delete `mixer.rs`, moving the limiter tests
- [x] Tests: tail is not lengthened by padding; old settings with `agc_enabled: true` still load

The resampler stays in the workers during this phase. `Aligner` keeps publishing to the atomics until Phase 4.

### Phase 4: move the resampler into the pipeline

This is a behavior-preserving move, done before sonora so that hardware checks and CPU numbers reflect the final thread layout.

- [x] Add the `DriftResampler` trait, `reset`, and a test pass-through
- [x] Add the input stage to `Pipeline`, and the `source_reopened` call in `try_reopen_sources`
- [x] Remove Rubato and the flush from `BlockForwarder`; call `finish_input` after the session drains its queues
- [x] Keep `stop_writes_blocks_that_are_still_queued_when_the_stream_closes` passing, and add a test that the flush reaches the aligner
- [x] Remove `drift_error` from `CapturePort::open` and all its implementations, and delete the atomics
- [x] Tests: drift error reaches the resampler, reopening resets it, `discontinuity` survives empty output, and the 100 ppm test now asserts through the closed loop

### Phase 5: `SonoraMicGain`

- [x] Add `sonora_steps.rs` and the real factory (`sonora = "=0.2.0"` was already pinned)
- [x] Replace `LinearGain` in production. The harness and exact-sample tests still use it. It stays in the library because `tests/` links `RecordingHarness` without `cfg(test)`.
- [x] Tests: level follows the slider, live change applies, `0.0` is silent

### Phase 6: `SonoraEchoCanceller`

- [x] Add the step, render-then-capture order, and scratch buffers
- [x] Tests from [section 11](#11-testing)
- [x] Skip the step when `system_active` is false

### Phase 7: failure handling and statistics

- [x] Sidecar `audio_processing` code
- [x] Log AEC statistics at session end
- [x] Add `StatsTap` and `ChannelTap` with tests

### Phase 8: hardware check and specs

- [ ] Run the checks in [section 11](#hardware-checks) and record the numbers in D-13. Not run: speakers, headphones, Bluetooth, a wrong output device, unplug, and a one-hour session. No laptop ERLE and no ppm number.
- [x] Update specs per [section 13](#13-spec-updates)
- [x] Add sonora's BSD-3-Clause license at `licenses/sonora-BSD-3-Clause.txt` (copied from the crates.io `sonora-0.2.0` LICENSE). M9 still copies that file into the ZIP.

---

## 11. Testing

Default `cargo test` must not need hardware. Sonora is pure Rust and runs on synthetic signals, so real AEC tests can run in CI. The tests below have no audio devices involved.

### Pipeline tests with fake steps

| Test | Proves |
| --- | --- |
| `echo_reference_is_system_audio_before_gain` | AEC receives raw system audio |
| `tap_between_steps_sees_the_first_step_only` | Taps read the audio at their exact position |
| `failed_step_leaves_audio_and_recording_running` | The failure rule holds |
| `panicking_step_is_disabled_for_the_session` | A panic does not lose the recording |
| `tail_shorter_than_a_frame_keeps_its_length` | Padding never lengthens a recording |
| `mono_mic_is_duplicated_to_both_channels` | Mono downmix and stereo output |
| `slow_channel_tap_drops_without_blocking` | A consumer cannot stall audio |
| `stats_tap_reports_rms_peak_and_clipped_samples` | RMS, peak, and full-scale clip count on one buffer |
| `stats_and_channel_taps_observe_the_frame_at_their_position` | Both taps read the audio where they sit |
| `resampler_panic_falls_back_to_pass_through_and_the_other_source_still_resamples` | One resampler panic does not stop the other source |
| `drift_error_from_aligner_reaches_the_resampler` | The feedback loop works without an atomic |
| `reopened_source_resets_its_resampler_only` | Reopening one source leaves the other alone |
| `discontinuity_survives_a_resampler_call_with_no_output` | The flag is carried to the next output |

Sample test, in the required form:

```rust
#[test]
fn echo_reference_is_system_audio_before_gain() {
    // Arrange
    let seen = Arc::new(AtomicU32::new(0));
    let mut pipeline = pipeline_with(
        Chain::new().process(RecordReferencePeak::new(Arc::clone(&seen))),
        Chain::new().process(FixedGain::new(0.5)),
    );
    pipeline.push_mic(tone(Some(0), FRAME_LEN, 0.1));
    pipeline.push_system(tone(Some(0), FRAME_LEN, 0.4));

    // Act
    let _ = pipeline.pull(0, false);

    // Assert
    let peak = f32::from_bits(seen.load(Ordering::Relaxed));
    assert!((peak - 0.4).abs() < 0.0001);
}
```

### Sonora tests with synthetic signals

| Test | Proves |
| --- | --- |
| `delayed_system_audio_is_reduced_in_the_mic` | Real echo cancellation, comparing energy after convergence |
| `speech_without_render_passes_nearly_unchanged` | Headphone users are not harmed |
| `near_speech_survives_during_echo` | Double talk keeps our voice; the threshold comes from the spike |
| `slider_change_applies_during_a_stream` | Live gain works |

A closed-loop drift test with real `StereoResampler` belongs here too: `drift_loop_settles_under_steady_timeline_error`. It covers the feedback path in isolation. It does not prove real device drift is seen; only S9 can.

Use noise-like or speech-like signals for AEC tests, and compare energy with thresholds set from spike results. A pure tone is a poor test for AEC. Do not assert exact samples.

### Hardware checks

These are manual and belong in the M9-style evidence notes:

- Laptop speakers and built-in mic: play a voice call, speak at the same time, listen to the WAV, and compare transcripts with the same session on the current build.
- Wired and Bluetooth headphones: the voice is not degraded.
- A Bluetooth speaker, whose output delay is large.
- Selected system device differs from the device actually playing.
- Unplug and reopen the mic and system device during a session.
- A one-hour session: compare WAV duration with the session clock, check that no new `overrun` problems occur, and record CPU use.

---

## 12. Risks

| # | Risk | Mitigation |
| --- | --- | --- |
| R1 | **The recording can't be undone.** We store only the processed mix, so a bad AEC result can't be recovered afterwards. | Spike S6 and hardware checks before shipping. Stems via `ChannelTap` are a natural later addition. |
| R2 | **Always on with no off switch.** AEC can reduce our own voice while others talk (double talk). On a device where this is bad, users have no way out. | Test explicitly. If checks show real harm, revisit the "always on" decision before release. |
| R3 | **Sonora is pre-1.0 and has limited field history.** | Exact pin. Production code imports it only in `sonora_steps.rs`. Failure rules protect the recording. |
| R4 | **Hot mic audio may hard-clip** in the gain step, where it is soft-limited today. | S4 measures it, then the decision gate applies. |
| R5 | **Added CPU on the session loop**, which also writes the WAV. Falling behind fills the 64-block capture queue and creates gaps. | S8 and the one-hour check. The `overrun` code already reports this. |
| R6 | **Reference and mic are misaligned**, for example after a late system block is replaced by silence. | Brief echo only; AEC3 re-estimates. Not a data-loss risk. |
| R7 | **Loopback is not what the speakers play**, if the user selected another output. | AEC finds nothing to remove, and the mic stays as it is today. Harmless, though echo stays. |
| R8 | **Two instances cost more** than one. | Gain-only should be cheap. S8 measures it. |
| R9 | **Moving the resampler changes timing.** Correction now runs when a block leaves the queue, not when it enters. A stalled WAV write also delays resampling. | The queue absorbs stalls as it does today. Phase 4 keeps the old tests, and the one-hour check confirms it. |
| R10 | **Drift may be invisible to the loop**, so sources could separate slowly in long recordings. | S9 measures it. It would be an existing bug, and a fix would be separate work. |

---

## 13. Spec updates

Do these in the same change as the behavior they describe, per `AGENTS.md`.

| Spec | Update |
| --- | --- |
| `skrivli-product.md` | Add automatic echo removal on speakers. Remove automatic gain from the Record tab and settings (lines 92 and 187) and from the deferrable list (line 255). Note that the mic is recorded as mono. |
| `skrivli-app-design.md` | Section 6.2: AEC is app-side (line 358). Section 6.4: replace the gain, AGC, and meter text (lines 371 to 373) with this pipeline. Move drift correction out of capture, and state that flexaudio converts the device rate (line 370). Remove Auto from the UI text (lines 485 to 494) and settings (line 515). Update the echo risk (line 638) and the mixer note (line 663). |
| `skrivli-implementation-plan.md` | Add these phases as a new package and check them off with evidence. Note that the M3.3 AGC item is superseded. |
| `decisions.md` | Add D-13: sonora pin, spike numbers including S9, instance layout, always-on AEC, Auto removal, mono mic, resampler location. Correct D-06: the resampler is 48 kHz to 48 kHz, owned by the pipeline, and reads the error directly. Update the "mixer" wording in D-03 and D-04. |

Line numbers are from the current specs and may shift.

---

## 14. Done means

- [ ] Recording on speakers has clearly less echo than the previous build, confirmed by listening and by ERLE in the logs. Not run.
- [ ] Headphone recordings have no audible degradation. Not run.
- [ ] A one-hour session: WAV duration against the session clock, no new overrun problems, and CPU use recorded. Not run.
- [x] The gain slider works live during a recording (synthetic stream). Auto is gone. Old settings that still contain `agc_enabled` load, and `mic_gain` applies.
- [x] Meters behave as described in [section 6](#6-reading-audio-at-any-point): microphone after echo cancellation and gain, system after the fixed gain, mixed snapshot after the limiter. Idle meters stay outside the pipeline. The Record tab draws the microphone and system meters.
- [x] A new tap or processor needs one new line in the factory, plus the new type. `StatsTap` and `ChannelTap` exist and are not in the production factory.
- [x] Production code imports `sonora` only in `sonora_steps.rs`. `sonora_spike.rs` is `#[cfg(test)]` and also imports it. `mixer.rs` is gone.
- [x] Capture workers contain no Rubato, and `CapturePort::open` takes no drift atomic.
- [x] S9 is recorded as not measured. The drift algorithm was not changed. If a later 30-minute run shows the loop cannot see real clock drift, that fix is a separate change. It is not part of this refactor.
- [x] Timeline, gap, drift, durability, and recovery behavior is unchanged, with the moved tests still passing. Confirmed by `cargo test` in Phase 8; no Rust changed in that phase.
- [x] `cargo test` is clean without hardware: 220 passed, 0 failed, 3 ignored (spike S6, S8, and the 10-second S9 probe). Phase 8 did not change Rust, so `cargo fmt` and clippy were not re-run.
- [x] All specs in [section 13](#13-spec-updates) are updated.

---

## Sources

- [sonora repository](https://github.com/dignifiedquire/sonora)
- [sonora API docs](https://docs.rs/sonora/latest/sonora/), read at version 0.2.0
- [crates.io metadata](https://crates.io/api/v1/crates/sonora)
