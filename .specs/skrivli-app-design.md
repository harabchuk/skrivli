# Skrivli — Application Design and Implementation Plan

**Platform:** Windows desktop (initially Windows 10/11, x86_64 MSVC)  
**Language:** Rust (stable; repository-pinned toolchain)  
**UI:** **GPUI Kit** (`gpui-kit`) on GPUI, with its styled component library; pin a Windows-tested version  
**Status:** Greenfield application design and phased implementation plan.

Skrivli records microphone and system playback audio concurrently, aligns and mixes the sources into a local audio file, and optionally transcribes a completed recording with an offline Whisper-compatible model. It operates from a desktop window and Windows notification area (system tray).

This document defines a new Windows-first application built from scratch with Rust and **GPUI Kit**. **`AGENTS.md` is authoritative for source placement, layering and coding conventions.** Use **GPUI Kit as the primary application UI framework**, including its bundled, compatible GPUI runtime and styled components; verify the pinned Kit version on Windows before committing to window/tray integration APIs.

---

## 1. Goals and non-goals

### Goals (v1)

- Record **microphone** input and **system playback** (loopback) at the same time via flexaudio.
- Timestamp, synchronize and mix both streams into one audio recording without losing the timeline when one source is silent.
- Save to a **user-selected directory** and persist that selection.
- Optionally transcribe **after recording stops**, on-device, with no upload to a cloud service.
- Display a selectable, copyable **plain-text transcript** in the Recordings detail pane.
- Support two main tabs: **Record** (devices, controls and meters) and **Recordings** (library, playback and transcript).
- Support tray-based app lifecycle and start/stop, keyboard shortcut and resilient local recording storage.

### Non-goals (v1)

- Speaker diarization; video or screen capture.
- Cloud accounts, synchronization or telemetry.
- Real-time transcription while audio is still being recorded.
- Cross-platform audio/UI support, although domain logic should stay platform-independent.

---

## 2. Greenfield project foundation

Skrivli begins as a new Rust application. Design, implement, and test each subsystem as a native Rust module from the outset; there is no existing application code or data format to preserve.

1. Create a **single Cargo package** with `src/main.rs` as a thin entry point and `src/lib.rs` as a testable application library. Introduce a workspace only if an independent component has a concrete need.
2. Pin the Rust toolchain when appropriate, commit `Cargo.lock`, and select a **Windows-tested GPUI Kit revision** with its compatible GPUI runtime. Prove that a basic window starts and renders on Windows before building the full UI.
3. Implement feature-oriented, cohesive Rust modules with clear domain, service, infrastructure, and presentation boundaries; follow **`AGENTS.md`** for source placement and coding conventions.
4. Integrate native libraries only where needed, such as whisper.cpp through Rust bindings or a tightly scoped FFI layer. Keep native build steps isolated and reproducible.
5. Set up Windows application identity, icon, manifest, and version metadata using suitable Rust-compatible build tooling. Validate DPI behavior with the pinned GPUI backend.
6. Establish logging, typed errors, configuration defaults, deterministic tests, and formatting/linting checks from the first milestone.

---

## 3. Technology choices

| Concern | Rust/Windows choice | Notes |
| --- | --- | --- |
| Application | Cargo; one Rust package, `lib.rs` + `main.rs` | `cargo build`, `cargo test`, `cargo clippy` |
| UI | **GPUI Kit** (`gpui-kit`), pinned Windows-tested version | Primary framework; re-exports compatible GPUI, `gpui-base`, `gpui-component`, and assets; use its styled components and app setup |
| Windows interfaces | `windows` crate plus flexaudio for capture | Shell, COM device notifications, dialogs, hotkeys; flexaudio owns WASAPI/cpal capture |
| Audio capture | flexaudio 0.8 (`Mic` + `SystemLoopback`) behind `CapturePort` | Dedicated worker threads; classic loopback keepalive on |
| Device format conversion | flexaudio normalizer to 48 kHz stereo `f32`, plus Rubato `Async` drift nudge | Internal 48 kHz stereo `f32`; device ids from `flexaudio::devices()` |
| Audio mixing | Rust DSP module, bounded SPSC buffers | Timestamp alignment, drift handling, limiter, meters |
| Recording | PCM16 stereo 48 kHz WAV first | `.wav.partial` while writing; finalize and recover on startup |
| Optional FLAC | `flac-codec` encode/decode | Post-processing worker; user opt-in |
| Transcription | whisper.cpp via maintained Rust bindings or a pinned FFI adapter | Local CPU inference; explicitly controlled native build, optional acceleration later |
| Default model | Quantized multilingual `base` model, subject to verified catalog/hash | Model isn't shipped; download only after user confirmation |
| Persistent settings | `serde` + `serde_json` under `%APPDATA%\Skrivli\settings.json` | Atomic replacement and validation |
| Recordings/metadata | Filesystem + JSON sidecars | Avoid DB for v1 |
| Async work | Dedicated audio threads and one transcription worker; channels to UI | GPUI foreground tasks only for UI-coordinating work |
| Diagnostics | `tracing` + rotating file sink, or comparable maintained solution | `%LOCALAPPDATA%\Skrivli\logs\` |
| Tests | Built-in Rust `#[test]`, integration tests in root `tests/` | No hardware for deterministic DSP tests |

**Technical validation points:** (1) GPUI Kit and its pinned GPUI Windows backend and window lifecycle; (2) WASAPI loopback behavior and format negotiation on target devices; (3) whisper.cpp binding, model formats, and licensing; (4) `flac-codec` encode/decode of 48 kHz stereo PCM16. Do not claim all of these APIs are already supported by a particular crate until proven with a small executable.

---

## 4. Architecture and execution model

### GPUI Kit integration rules

- Use **`gpui-kit` as the primary UI dependency** in `Cargo.toml`. It supplies the matching GPUI runtime and re-exports GPUI Base, GPUI Component and default assets. Avoid independently mixing versions of its constituent crates.
- Bootstrap using the **GPUI Kit application and window APIs**, and initialize components with `gpui_kit::init(cx)` before rendering any styled widgets. Consult the pinned release for exact signatures.
- Prefer Kit components for buttons, selection controls, inputs, tabs, data tables, menus, dialogs, tooltips and read-only transcript display. Implement bespoke controls only for application-specific needs such as audio meters.
- Use Kit themes, design tokens, focus, overlays and accessibility patterns; do not build a parallel design system.
- Do **not** include optional `gpui-shell` JavaScript extension hosting in v1.
- Verify Windows behavior of the pinned GPUI Kit release on a real device, particularly window setup, overlays, text selection and keyboard interactions. Native tray, hotkeys, and folder picking remain platform adapters when necessary.

Example Cargo dependency for the currently documented release (validate against the repository lockfile before use):

```toml
[dependencies]
gpui-kit = "0.9.0"
```

This is a firm framework decision, **not** an optional later UI improvement.

### 4.1 Logical architecture

```text
 Windows application process
 ┌──────────────────────── GPUI Kit / GPUI foreground thread ─────────────────────────┐
 │ app/bootstrap + app/state                                             │
 │ views/main_window                                                     │
 │  ├─ views/recording     ← ui/components/{record_button,level_meter,...}│
 │  ├─ views/recordings    ← library, player + transcript                 │
 │  ├─ views/settings      ← preferences                                  │
 │  └─ views/model_prompt  ← choose/download model                        │
 │ app/actions → service commands; service events → observable UI state   │
 └─────────────────────▲──────────────────────────┬───────────────────────┘
                       │ structured events         │ commands
 ┌─────────────────────┴──────────────────────────▼───────────────────────┐
 │ services/recording      services/level_monitor   services/transcription│
 │    recording lifecycle      idle meters            queued jobs          │
 └──────────┬───────────────────────────┬───────────────────┬──────────────┘
            │                           │                   │
 ┌──────────▼───────────────────────────▼───────────────────▼──────────────┐
 │ infrastructure/                                                      │
 │  audio: flexaudio mic + loopback → SPSC → timed mixer → peak snapshots │
 │  files: WAV writer / reader / playback / optional FLAC encoder       │
 │  ai: decode → 16 kHz mono → whisper.cpp → transcript                  │
 │  storage: settings / library / sidecars / folder watcher             │
 │  windows: tray / hotkey / shell dialogs / autostart                   │
 └───────────────────────────────────────────────────────────────────────┘
                        domain/: pure models, timestamp rules, text
```

`domain/` contains no GPUI, Windows, COM, file I/O or AI dependency. `services/` express use cases and depend on small ports where implementation interchangeability or testing warrants them. `infrastructure/` implements adapters. `app/` wires services into views. UI rendering must never own a capture handle or call Whisper.

### 4.2 Threads and communication

| Execution context | Responsibilities | Constraints |
| --- | --- | --- |
| GPUI foreground | Windows, entities, views, focus, menu state, message presentation | No blocking audio/file/model operations |
| Microphone capture thread | flexaudio stream worker, timeline mapping, drift resampler | Short bounded packet processing, no UI calls |
| Loopback capture thread | Separate COM apartment + loopback event loop | Handle silence, discontinuity and device invalidation |
| Mixer/writer thread | Merge 10 ms blocks, disk output, meter atomics | Bounded latency; protect recording on I/O error |
| Idle level-monitor worker(s) | Meter selected sources without recording | Stop/hand over before recording starts |
| Transcription worker | At most one queued job at a time, decode/model infer/save | Cancellation, progress, no GPUI access |
| Optional storage watcher/encoder | Directory notifications and FLAC conversion | Debounce updates; no UI thread I/O |

Use bounded channels/queues for commands and discrete events (`RecordingStarted`, `RecordingStopped { path }`, `RecordingFailed`, `TranscriptProgress`, `TranscriptReady`, `DevicesChanged`). Aggregate high-rate meter data into atomic snapshots and read it from a GPUI-owned timer/task at about 30 Hz; **never send one GPUI notification per audio frame**. Route worker events back to UI through a thread-safe application bridge scheduled on the GPUI foreground executor, using the *pinned* GPUI revision's supported API. Avoid `std::thread::spawn` without a shutdown owner; retain join handles or supervised worker lifecycle.

**COM ownership:** initialize/uninitialize COM on each thread that uses it. Keep COM interfaces in their owning apartment; do not assert `Send`/`Sync` for incompatible interface wrappers. Prefer native worker threads for timing-sensitive capture; foreground GPUI `spawn` is not a replacement for an audio thread. Use RAII guards for events, multimedia thread scheduling and COM resources.

### 4.3 State ownership

- `RecordingService` is the authoritative owner of recording lifecycle and output path.
- `TranscriptionService` owns queue, cancellation and progress; persisted sidecar mirrors durable state.
- `RecordingLibrary` owns the on-disk catalog as read from the configured directory.
- GPUI state owns selected tab/row, open overlays, focus, window size and **snapshots** of service state.
- Tray and hotkey actions dispatch the same application commands as the Record button. They must not implement a second recording state machine.

Illustrative service messages (adapt to the actual code):

```rust
pub enum AppCommand {
    StartRecording,
    StopRecording,
    Transcribe { recording_id: RecordingId },
    CancelTranscription { job_id: JobId },
}

pub enum AppEvent {
    RecordingStateChanged(RecordingState),
    RecordingSaved { path: std::path::PathBuf },
    TranscriptionProgress { job_id: JobId, percent: u8 },
    TranscriptionCompleted { recording_id: RecordingId },
    RecoverableError { message: String },
}
```

These are illustrative types, not ready-to-compile declarations; IDs and state types belong to the appropriate domain/service modules.

---

## 5. Source layout

Follow `AGENTS.md` for module placement. Target structure (only add files when implemented):

```text
skrivli/
├── AGENTS.md
├── app-design.md
├── Cargo.toml
├── Cargo.lock
├── build.rs                        # only for Windows resources/native build
├── assets/                         # bundled non-Win32 UI assets
├── resources/                      # .ico, Windows manifest, resource script
├── .specs/                         # additional design/ADR documents
├── src/
│   ├── main.rs                     # entry point
│   ├── lib.rs                      # library modules for integration tests
│   ├── app/
│   │   ├── mod.rs
│   │   ├── bootstrap.rs            # GPUI init, dependency wiring
│   │   ├── state.rs                # foreground presentation state
│   │   └── actions.rs              # intent routing
│   ├── domain/
│   │   ├── mod.rs
│   │   ├── audio.rs                # format/frame definitions
│   │   ├── ring_buffer.rs          # if custom lock-free ring justified
│   │   ├── chunk_plan.rs           # chunking/timeline logic
│   │   ├── recording.rs            # recording metadata + status
│   │   └── transcript.rs           # segments, text formatting
│   ├── services/
│   │   ├── mod.rs
│   │   ├── recording.rs            # one recording lifecycle
│   │   ├── level_monitor.rs
│   │   ├── transcription.rs        # queued jobs
│   │   └── ports.rs                # external capability traits, as needed
│   ├── infrastructure/
│   │   ├── mod.rs
│   │   ├── audio/
│   │   │   ├── mod.rs
│   │   │   ├── devices.rs
│   │   │   ├── capture.rs
│   │   │   ├── loopback.rs
│   │   │   ├── mixer.rs
│   │   │   ├── resampler.rs
│   │   │   └── format.rs
│   │   ├── ai/
│   │   │   ├── mod.rs
│   │   │   ├── whisper.rs
│   │   │   └── models.rs
│   │   ├── storage/
│   │   │   ├── mod.rs
│   │   │   ├── settings.rs
│   │   │   ├── library.rs
│   │   │   ├── sidecar.rs
│   │   │   └── watcher.rs
│   │   ├── files/
│   │   │   ├── mod.rs
│   │   │   ├── wav.rs
│   │   │   ├── flac.rs
│   │   │   ├── reader.rs
│   │   │   └── player.rs
│   │   └── windows/
│   │       ├── mod.rs
│   │       ├── autostart.rs
│   │       ├── hotkeys.rs
│   │       ├── clipboard.rs
│   │       ├── tray.rs
│   │       └── dialogs.rs
│   ├── ui/
│   │   ├── mod.rs
│   │   ├── theme.rs
│   │   └── components/
│   │       ├── mod.rs
│   │       ├── device_picker.rs
│   │       ├── level_meter.rs
│   │       ├── record_button.rs
│   │       └── tabs.rs
│   ├── views/
│   │   ├── mod.rs
│   │   ├── main_window.rs
│   │   ├── recording.rs
│   │   ├── recordings.rs
│   │   ├── settings.rs
│   │   └── model_prompt.rs
│   └── common/
│       ├── mod.rs
│       ├── error.rs
│       ├── paths.rs
│       └── logging.rs
└── tests/
    ├── recording_flow.rs
    ├── transcription_flow.rs
    └── support/mod.rs
```

Rust conventions: snake_case modules; types group by cohesive responsibility rather than one file per type; module-private helpers stay private; `Result` and typed errors at boundaries; `Drop`/RAII for resource ownership; `#[cfg(test)] mod tests` co-located with the implementation. Integration tests in root `tests/` exercise public library APIs. The root `tests/support` directory is a helper module, not an automatically runnable test crate.

### 5.1 Module ownership rules

| Module | Primary responsibility |
| --- | --- |
| `app/` | GPUI Kit bootstrap, dependency wiring, application-wide actions and lifecycle |
| `domain/` | Audio formats, timestamp/chunk types, recording and transcript data models, pure text formatting |
| `services/` | Recording sessions, idle level monitoring, transcription jobs and orchestration |
| `infrastructure/audio/` | flexaudio capture adapters, device catalog, mixing and resampling |
| `infrastructure/ai/` | Whisper engine integration and local model management |
| `infrastructure/storage/` | JSON settings, recording library, sidecars and directory watching |
| `infrastructure/files/` | WAV writer, optional FLAC encoder/reader via `flac-codec`, audio reader and player |
| `infrastructure/windows/` | System tray, global hotkeys, startup behavior and Windows dialogs |
| `ui/` | Reusable GPUI Kit compositions and custom GPUI elements such as level meters |
| `views/` | GPUI screens, window composition and presentation state |
| `tests/` | End-to-end application integration tests and reusable test support |

Prefer channels and typed events for worker-to-UI communication, with updates dispatched onto the GPUI foreground executor. Avoid blocking UI rendering with device or inference work.

---

## 6. Audio recording design

### 6.1 Device discovery and selection

- Enumerate active `eCapture` and `eRender` endpoints via Windows Core Audio; read stable endpoint IDs and display-friendly names.
- Show **Microphone** (default: Windows default communications input) and **System audio** (default output endpoint) selectors.
- Store explicit endpoint IDs in settings. If an ID disappears, fall back to the relevant Windows default and show a non-blocking notice.
- Register a Windows endpoint-notification callback. It should enqueue a lightweight event, not reinitialize streams inside the callback.
- Do not allow a selection change during a recording; support mid-recording device loss via the recovery policy below.

### 6.2 flexaudio capture

- Two independent shared-mode streams, one capture endpoint and one render endpoint with `AUDCLNT_STREAMFLAGS_LOOPBACK`.
- Prefer event-driven buffering and a roughly 200 ms engine buffer if supported; check the negotiated format/device period rather than assuming one setting fits every device.
- Internal processing format: **48,000 Hz, stereo, interleaved `f32`**. Try suitable shared-mode format conversion only after probing support. On rejection, open the device mix format and perform explicit channel mapping, sample conversion and resampling.
- A capture worker drains all available packets for each event, converting `AUDCLNT_BUFFERFLAGS_SILENT` to zero frames and retaining first-frame QPC/device timestamps. On `DATA_DISCONTINUITY`, record a gap marker and let the mixer repair the timeline.
- No synchronous filesystem I/O, GPUI entity operations, blocking locks, model calls or unbounded allocations in the hot capture loop.
- Audio packets move via bounded SPSC buffers (or an equally suitable proven crate) to the mixer. Define overflow and backpressure handling: report overrun, drop according to a documented policy and preserve elapsed timeline with gaps rather than indefinitely blocking capture.
- Optionally request Windows communications processing for microphone capture, but treat AEC/noise processing as device-dependent and separate from app-side gain control.

### 6.3 Silence and loopback keep-alive

System loopback uses flexaudio’s classic silent-render keepalive so idle endpoints keep delivering silence on the device clock. The mixer must **never infer that the recording clock has stopped**. Use session-timeline alignment (`frame_index` anchored at open) and synthesize silence for remaining gaps. Timestamp-based gap filling remains required even with keepalive.

### 6.4 Timeline, mixing and clock drift

- Session time origin `t0`: monotonic QPC timestamp at start.
- Convert packet timestamps into positions on the 48 kHz sample timeline, taking timestamp-unit conversions and device latency into account.
- Operate in **10 ms / 480-frame** output blocks. A source gap of roughly 5 ms becomes silence; duplicated/overlapping late frames are discarded according to a deterministic rule.
- Emit when sufficient data exists or when a block is more than about **150 ms** behind real time; a stalled input contributes silence. These thresholds are initial values and should be validated by tests on actual devices.
- Two devices have independent hardware clocks. Each capture stream uses Rubato asynchronous resampling (device rate → 48 kHz) with a tiny adaptive ratio driven by mixer timeline error, so ppm-level clock drift is corrected without sparse drop/duplicate splices. Sudden holes still become silence; sudden overlaps are still discarded.
- Mix `clamp(mic_gain * mic + system_gain * system)` with initial mic gain `1.0`, system gain `0.8`, and a tested soft limiter to avoid clipped speech. Downmix/duplicate mono microphone channels explicitly.
- Optional **mic AGC**, off by default: approximately 0.75 s RMS analysis, −18 dBFS speech target, gain limits 0.5×–4×, noise gate below −45 dBFS, smooth gain transitions. This is application-side processing only.
- Publish per-source and mixed peak snapshots plus effective microphone gain for meters, not per-sample UI events.
- Optional (off by default): record separate `*.mic.wav` and `*.system.wav` stems for future troubleshooting/diarization.

### 6.5 Audio files, metadata and crash safety

- Name recordings `Skrivli_YYYY-MM-DD_HH-MM-SS.wav` in local time, with a collision suffix.
- Write into `*.wav.partial` first, as **48 kHz, stereo, PCM16 WAV**, using the `hound` library for encoding and header updates. Ensure the writer can periodically repair its RIFF/data length fields (e.g. every two seconds) and finalize lengths on close. Standard RIFF size limitations must be handled explicitly: stop/rotate before overflow or adopt a compatible large-file format; do not silently create corrupt recordings.
- When stopping: drain queued samples, flush and close the writer, persist metadata, and rename to final `.wav`. Use a unique temporary path within the destination directory to keep rename semantics reliable.
- On launch, detect unfinished `.partial` WAV files, validate enough header/data to recover, repair lengths and rename as `*.recovered.wav`. Preserve originals or report an error if recovery isn't safe.
- Check free space before start and periodically (~30 s). Below a safety threshold (initially 200 MB) stop gracefully, finalize what can be saved, and notify.
- If the original output folder becomes inaccessible mid-recording, retain a usable partial if possible and attempt recovery to `%LOCALAPPDATA%\Skrivli\rescue\` using a documented bounded strategy; **do not promise that already-failed filesystem writes can be recovered**.
- Optional post-recording lossless FLAC encoding with the `flac-codec` crate; only remove WAV after verifying the output (reopen/decode) and if user opted in.
- Store a sidecar `<basename>.json` containing start/end, duration, chosen device IDs, format, recording problems, transcription state, model and transcript linkage. Use a versioned schema and atomic JSON writes.

### 6.6 Recording lifecycle

```text
Idle ──start──▶ Starting ──ready──▶ Recording ──stop──▶ Stopping ──saved──▶ Idle
                   │                    │                  │
                   └─────error──────────┴────error─────────┴──▶ Error/Recovery
```

- **Start:** validate destination/free space; stop idle level monitor; resolve endpoints; initialize capture workers and optional keep-alive; start mixing and writer; publish `Recording` only after resources are ready.
- **Stop:** transition once (idempotent stop); request capture shutdown; join/drain workers safely; finalize output and sidecar; publish `RecordingSaved` and restart idle level monitor.
- **During errors:** isolate a lost input, attempt the Windows default replacement when appropriate, insert silence for the gap and annotate sidecar. If all usable inputs fail, stop with a recoverable recording where possible.
- **Exit:** explicit application exit while recording requests confirmation to stop and save. For Windows logoff/shutdown notifications, start a bounded best-effort finalize process without assuming indefinite time is available. Window close-to-tray is *not* app exit.

---

## 7. Transcription design

### 7.1 Engine and FFI ownership

- Use a pinned whisper.cpp revision exposed through a maintained Rust crate/bindings or a minimal `whisper-sys`-style adapter. Verify ABI/API compatibility and bundle notices/licenses. Keep FFI in `infrastructure/ai/whisper.rs` or a dedicated internal binding crate if justified.
- Build CPU inference by default. Do not enable GPU backends until there is a supported Windows packaging path.
- Load the selected model once per active service session; optionally unload after about five minutes of idle time to release memory.
- Defaults: multilingual auto language detection, translation off, conservative CPU thread count (roughly available cores minus one), no console output from inference.
- Translate engine progress/abort callbacks into structured events and a cancellation flag, without touching GPUI from the callback.
- Collect segment text, timestamps and engine metadata into domain transcript types. Keep all engine-owned pointers/buffers inside their safe lifetime and thread context.
- Optional voice activity detection, after validating supported model and engine version, to skip silence and reduce fabricated text.

### 7.2 Input normalization and chunking

- Decode WAV/MP3 using a validated library; decode FLAC with `flac-codec` when needed. Do not assume any one decoder supports all target formats.
- Convert decoded input to **16 kHz mono `f32`** with explicit downmix and anti-alias resampling. Verify sample counts and timestamps.
- Do not decode a two-hour file into a single in-memory buffer. Stream/cut long recordings into approximately **30-minute chunks** with overlap, offset segment timestamps and deduplicate content in overlap regions.
- Keep audio source immutable during transcription. A failed job must never alter the recorded file.

### 7.3 Local model management

- Discover `ggml-*.bin` in `%LOCALAPPDATA%\Skrivli\models\` plus explicitly chosen user paths.
- On first use without a model, show a GPUI Kit dialog/model-choice prompt: download the default quantized multilingual base model or browse for a local model file.
- Downloads must be **user-initiated only**, with progress, resumability where supported, and **SHA-256 verification against a maintained trusted manifest** before a file is accepted. Download to a temporary file and rename after verification.
- Provide tiny/base/small and suitable English/multilingual options only when the pinned engine supports the selected model files. Do not hardcode unverified model sizes or hashes.
- Language, model and CPU thread count are configurable. A local-model workflow is fully offline after installation.

### 7.4 Plain-text output

Write UTF-8 without BOM to `<basename>.txt`, for example:

```text
[00:00:03] Okay, let's get started with the weekly sync.

[00:00:09] First item is the release schedule...
```

- Merge adjacent short segments into readable paragraphs, breaking after a pause greater than ~1.5 s or a paragraph of roughly 60 s.
- Trim whitespace and conservatively remove repeated identical hallucinated lines while avoiding removal of genuine repeated speech.
- Preserve timestamps as `[HH:MM:SS]`, including recordings lasting longer than an hour.

### 7.5 Job queue and UI notifications

```text
Queued → Preparing → Transcribing { percent } → Done | Failed | Cancelled
```

- FIFO queue; one job in inference at a time; cancellation is cooperative.
- Persist significant job transitions (and final status) to sidecar; progress snapshots are ephemeral and throttled for UI.
- If auto-transcribe is enabled, enqueue after a recording has finalized; otherwise only on user request. Default **off**.
- On completion, show a Windows notification/tray signal when supported and update GPUI library state. Clicking a relevant notification selects that recording if possible.
- Model missing/corrupt, decode error, OOM and cancellation are represented as distinct user-facing results. Preserve audio on all errors.

---

## 8. GPUI Kit user interface design

**Component policy:** GPUI Kit is mandatory for standard widgets and styling. Custom raw GPUI elements are for screen composition and app-specific controls (especially audio meters); native Windows UI is reserved for OS integrations.

### 8.1 Main window

- Resizable GPUI Kit application window, minimum target roughly **560 × 420 logical pixels**, with remembered dimensions and monitor-safe restore.
- Two top tabs: **Record** and **Recordings**. Use GPUI Kit navigation/tab components where they meet requirements, **not** Win32 `CTabCtrl`/`CDialogImpl` child windows.
- Adopt **GPUI Kit themes, design tokens, typography, buttons, inputs, selectors, tabs, menus, dialogs, tooltips, and other styled controls** as defaults. Use GPUI layout primitives (`div`, flex/grid) to compose screens; avoid duplicating GPUI Kit widgets in custom code.
- No in-window File/Help menu bar. Settings opens from a gear button on `TabBar::suffix` (`IconName::Settings`); About is linked from the Settings dialog. Open recordings folder and Exit stay on the Recordings tab and tray.
- A compact status strip shows recording state, duration and transcription progress.
- Use logical-pixel layout, Windows DPI-aware assets and explicit keyboard focus/shortcuts. Test per-monitor scaling on real Windows devices.
- Keep views declarative: update model/entity state when a service event arrives, notify GPUI to rerender, and never allocate an audio device during `Render::render`.

### 8.2 Record tab

```text
┌──────────────────────────────────────────────────┐
│ INPUT                                            │
│ Microphone      [ Default communications      ▾ ]│
│ System audio    [ Default output device       ▾ ]│
│                                                  │
│ OUTPUT                                           │
│ Save to         [ C:\Users\…\Skrivli ] [Browse] │
│                                                  │
│               [ ● Start recording ]              │
│                     00:12:34                     │
│                      ● REC                       │
│ Mic  ███████████░░░  −12 dB   Gain ──●── 1.0× [Auto] │
│ System █████░░░░░░░░░░░░░       −24 dB           │
│                                                  │
│ [inline device/permission/disk warning]          │
└──────────────────────────────────────────────────┘
```

- Compose **GPUI Kit** select/combo, button, input, slider, checkbox, status, and dialog components for the recording form. Implement custom `level_meter` (and specialized recording indicator only if needed) using GPUI primitives and GPUI Kit theme tokens. Do **not** create a parallel theme system; use GPUI Kit tokens and component styling.
- Choose a folder with a Windows folder-picker adapter; show it in a read-only path field. Default `%USERPROFILE%\Documents\Skrivli\` where writable.
- Disable source selection and destination changes while recording. Gain and Auto (AGC) remain live, with the slider disabled when Auto is active.
- A red recording state, elapsed timer and status text; ensure information is accessible without relying on red color or a blinking indicator alone.
- Meter data: post-gain microphone and system peak on −60..0 dBFS, green→amber→red range and a ~1 s peak hold. Draw from throttled snapshots; maintain meter monitoring while idle using `LevelMonitor`, then hand ownership to the recording session.
- Missing devices/unwritable destination show inline GPUI Kit feedback; material failures use GPUI Kit dialogs or a Windows TaskDialog through a platform adapter when specifically required.
- Global toggle shortcut default `Ctrl+Alt+R` using Windows `RegisterHotKey`; allow disable/rebinding and handle conflicts explicitly.

### 8.3 Recordings tab

- Responsive **split-pane** layout: recording list left, selected detail right. At narrow widths, a stacked/detail-navigation fallback is acceptable.
- List columns: **Name | Date | Duration | Size | Transcript**. Sort newest first; clickable sorting, keyboard selection, stable selection keyed by recording ID/path.
- Transcript statuses: `—`, `Queued`, `42%`, `Ready`, `Failed`.
- Right pane: empty state until selected; then **Play / Stop**, file metadata, plain-text transcript in a selectable and copyable read-only text view; or **Create transcript** button if absent. Prefer a GPUI Kit read-only editor/text component, and verify selection, clipboard, keyboard scrolling and large-text performance; a plain `div().child(text)` is not an adequate substitute.
- Actions: Play, Transcribe, Open folder, Rename, Delete, Refresh. Delete goes to the Windows Recycle Bin through a Windows Shell adapter and includes associated audio/`.txt`/`.json`; ask for confirmation and handle partial failures.
- Double-click/Enter can start playback; selection change stops playback by design.
- Background directory watcher debounces changes; the library scans WAV/FLAC files and matches sidecars/transcripts. Preserve selected recording after refresh.
- Playback audio runs through a file/player adapter; never decode files or drive playback from GPUI rendering.

### 8.4 Model prompt and settings

**Model prompt:** available installed models, browse button, explicit download action, verification/progress/cancel feedback, errors and retry.

**Settings:** recording directory, default endpoints, gain/AGC, optional stems, FLAC and WAV retention; transcription model, language, CPU threads, VAD, auto-transcribe; close-to-tray, Windows autostart, start hidden and global hotkey. Validate choices before persisting, and apply mutable settings without restarting where safe.

### 8.5 System tray (native Windows integration)

- The tray is **not a GPUI widget**: implement Windows `Shell_NotifyIconW` behind `infrastructure/windows/tray.rs`, potentially using a supported tray crate if it fulfills shell lifecycle requirements.
- Idle/recording icons and tooltip `Recording HH:MM:SS`.
- Left click toggles window visibility. Right-click menu: **Show Skrivli**, **Start/Stop recording**, **Open recordings folder**, **Settings**, **Exit**.
- Minimize hides to tray. Close hides to tray by default (configurable), with a one-time message explaining the app is still running. Exit is explicit.
- Register notification icon using a stable GUID and current notify-icon version where feasible. Re-add after Windows Explorer restarts (`TaskbarCreated`).
- Notifications: transcription completed, low disk space, device recovery. Respect notification availability, user settings and OS focus assist.
- Tray callback/window-message bridging must have a safe lifecycle: a dedicated hidden HWND/message pump if required by the GPUI backend, with explicit handoff of events to the app command bridge. Do not assume GPUI's main HWND can be subclassed safely without backend support.

### 8.6 Single instance

Use an OS-level named mutex such as `Local\Skrivli.SingleInstance`. A second instance should signal the first to show/focus itself using a registered message or a small authenticated local IPC handoff and exit. Avoid a race between mutex acquisition and discovery of a not-yet-initialized window. Do not let the second process touch the active `.partial` recording.

---

## 9. Failures, recovery and privacy

| Scenario | Required behavior |
| --- | --- |
| No microphone / no playback endpoint | Allow single-source recording, clearly indicate absent source |
| Microphone access denied | Explain privacy permissions; offer `ms-settings:privacy-microphone` |
| Exclusive-mode device conflict or unsupported capture format | Explain failure and continue with remaining viable source when possible |
| Endpoint unplugged/default changed | Re-open appropriate endpoint, mark gap, synthesize silence and annotate sidecar |
| Loopback silent | Preserve monotonic recording time; fill missing source with zeros |
| Device clock drift | Compensate conservatively and expose diagnostic counters in logs |
| Destination deleted/unmounted/full | Stop/finalize as safely as possible, rescue if possible, do not discard partial files |
| Recording writer/encoder failure | Preserve usable WAV, report problem and never silently overwrite |
| Corrupt/missing transcription model, OOM | Mark failed, keep audio intact, allow retry |
| Worker panic/shutdown | Surface fatal service state; join/clean up workers, preserve recovery artifact |
| Closing window while recording | Close-to-tray does not stop recording; explicit Exit requests save confirmation |

Privacy by default: no accounts, telemetry, network transcription or automatic model download. Model download is explicit; logs must not include captured audio or transcript text. Recording files and transcripts remain user-owned local files. Logs: rotating files under `%LOCALAPPDATA%\Skrivli\logs\` (initial policy ~1 MB × 3); redact sensitive file/endpoint details where they aren't required for troubleshooting.

---

## 10. Build and Windows packaging

- Build target: `x86_64-pc-windows-msvc` on a supported Windows 10/11 development machine. Verify the selected GPUI Kit distribution’s minimum Windows and compiler requirements on target machines.
- Install the Rust toolchain and the required MSVC/Windows SDK build tools for Windows linking and any native dependencies.
- Cargo owns application dependency graph. Pin the exact GPUI Kit version (and its compatible GPUI runtime) and whisper/ggml integration versions; commit `Cargo.lock` for the app.
- If linking whisper.cpp directly, configure its source/build via a documented Rust `build.rs`/`cmake`/`cc`-based build (or a maintained Rust wrapper's build process). Keep C++ build flags scoped to the native library.
- Use `build.rs` and an appropriate resource compiler for Windows manifest, icon and version info; keep source resources in `resources/`.
- Recommended local commands:

```powershell
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build --release
```

- Optional `scripts/start.ps1` and `scripts/test.ps1` wrap Cargo; keep Cargo as the application build and test entry point.
- Initial deliverable is a portable ZIP with executable, required native runtime files, licensing notices and a README. Later consider WiX/MSIX if installation, autostart and shell integration require it. Do not bundle speech models by default; download on user request.

---

## 11. Test strategy

### 11.1 Unit tests (deterministic, no hardware)

Put small tests beside Rust implementations with `#[cfg(test)] mod tests`; extract large suites into an adjacent `tests/` submodule when necessary. Use injected clocks, temporary directories, synthetic audio and fake service ports.

- Ring buffer: capacity, SPSC order, wrap-around, overflow policy and concurrent stress tests.
- Mixer: QPC alignment, silent intervals, overlapping/late packets, bounded wait, 100-ppm simulated drift, gains and clipping/limiter.
- Resampler: 48→16 kHz 1 kHz sine retains pitch/amplitude, >8 kHz aliases attenuated, stereo downmix checks.
- WAV: correct RIFF sizes and PCM16 encoding, periodic repair, crash-interrupted recovery, large-file boundary handling.
- Transcript: time formatting beyond one hour, paragraph grouping, repeated-line suppression, overlap chunk stitching.
- Library: audio/text/JSON pairing, sorting, stable selection keys, malformed metadata, partial file recovery.
- Services: invalid transitions, idempotent stop, cancellation, retry, startup failure rollback, single-job queue behavior.
- Settings: defaults, roundtrip JSON, invalid saved endpoint/path fallback.

### 11.2 Root integration tests

Root `tests/recording_flow.rs` and `tests/transcription_flow.rs` should exercise public application services with fake capture/decoder/engine/storage ports, not actual devices. Include temp-directory fixtures and fake event sinks. Integration tests may be gated by Windows target as appropriate.

### 11.3 Windows hardware/manual verification

- Record 10 minutes with intermittent system playback/speech, inspect WAV duration and sync using a shared audible cue.
- Unplug/replug USB headset and change the Windows default audio device mid-take.
- Record more than two hours; verify memory, file length limits, recovery behavior and long-transcript chunking.
- Test silence while capturing loopback and check timeline continuity.
- Close to tray, restart Explorer, and verify tray icon/menu/actions recover.
- Test hotkey conflicts, multiple displays and DPI changes, microphone privacy denial, a disconnected output folder, disk space exhaustion.
- Transcribe English and at least one non-English sample; record word accuracy and real-time factor for tiny/base models.
- Verify UI remains responsive when transcoding/transcribing and that cancelled jobs release model/file resources.

Optional device-dependent tests are manual or explicitly ignored/gated in CI; never make default `cargo test` depend on actual hardware.

---

## 12. Milestones

Each milestone ends with a runnable app or independently validated recording functionality. Schedule and estimates should be based on hands-on validation of the Rust, GPUI Kit, and Windows integrations.

| Milestone | Deliverable | Acceptance gate |
| --- | --- | --- |
| **M0 — Foundation** | Cargo package, pinned Windows-tested GPUI Kit, manifest/resources, logging/settings skeleton, single-instance policy, COM threading design | Opens a GPUI window on target Windows hardware; Cargo checks pass |
| **M1 — Shell UI** | Resizable tabbed window, placeholder Record/Recordings views, tray icon, close/minimize policy | UI/tray lifecycle works including Explorer restart |
| **M2 — Microphone capture** | Device enumeration, single mic flexaudio capture, WAV `.partial`, folder picker, start/stop | Produces playable WAV and cleanly finalizes |
| **M3 — Loopback + mixer** | Loopback capture, timestamp alignment, ring buffers, mixer, gains, level meters and idle monitoring | Synthetic tests pass; real two-source sync verified |
| **M4 — Robustness** | Endpoint changes, disk checks, privacy flow, partial-file recovery, shutdown behavior | Device loss and interrupted recording preserve usable data |
| **M5 — Recording library** | Library scan/watch, sidecars, list, playback, rename/delete/open folder | Files remain correctly paired and selected (implemented; hardware browse/playback spot-check still recommended) |
| **M6 — Transcription** | Pinned whisper.cpp binding, decoder/resampler, model manager, FIFO worker, text formatter | Offline transcription yields correct `.txt` and supports cancel |
| **M7 — Transcript UI** | GPUI Kit text selection/copy, transcript statuses, progress and notification | Entire recording-to-transcript flow works from UI |
| **M8 — Settings and polish** | Settings UI, hotkey/autostart, DPI and keyboard accessibility, optional FLAC | User preferences persist and critical interactions are accessible |
| **M9 — Release** | Release build, portable ZIP, licensing/README/privacy notes, manual test pass | Clean Windows machine can run app and record/transcribe |

---

## 13. Risks and mitigations

| Risk | Mitigation |
| --- | --- |
| Selected GPUI Kit/GPUI distribution does not fully support required Windows APIs/lifecycle | M0 Windows spike; pin exact revision; isolate native tray/dialog integration behind adapters |
| GPUI Kit / GPUI foreground thread stalls from capture/inference | Dedicated workers, bounded channels, throttled state updates |
| COM thread/apartment misuse | flexaudio owns capture COM; device-watch COM stays on its worker; no unchecked cross-thread interface movement |
| Silent loopback produces no packets | QPC timeline and silence insertion; investigate silent render keep-alive |
| Drift between independent devices | Unit-test 100 ppm deviations; Rubato adaptive resample ratio on each capture stream |
| Driver rejects requested format/conversion | Probe `IsFormatSupported`/mix format; test alternate converter path |
| Long WAV exceeds RIFF 32-bit size | Monitor file length and stop/split or use compatible large-file format |
| Speaker feedback/echo | Recommend headphones; AEC only when supported; optional source stems |
| Silence hallucination in Whisper | VAD when verified, conservative duplicate cleanup, progress/cancel |
| whisper.cpp FFI/build mismatch | Pin tested native revision/binding; minimal isolated adapter; CI native-build test |
| High CPU/memory on slow laptops | Quantized model choice, one inference job, bounded chunking, no blocking UI work |
| Recording interrupted by crash/disk failure | Partial files, periodic WAV repair, metadata recovery, rescue path and actionable error |
| Windows Shell/notification behavior changes | Native adapter with manual Windows/Explorer restart tests |

---

## 14. Future work

- Differentiate local microphone from remote participant speech using separate stems and/or diarization.
- Live transcription using rolling windows once the asynchronous audio pipeline is established.
- Local summaries and action items (e.g. llama.cpp), without cloud dependency by default.
- Full-text transcript search across the recording library.
- Optional ggml GPU acceleration on supported Windows systems, with a validated distribution path.
- Consider more platform backends only when there is a committed non-Windows target.

---

## 15. Design decisions requiring validation

These are deliberate implementation choices to settle during M0–M3, not implicit claims of already-working APIs:

1. **GPUI Kit version and Windows backend:** confirm `Application`/window creation, focus, close interception, timers/tasks, text selection, and exact event subscription API with a pinned revision.
2. **Capture stack:** flexaudio 0.8 behind `CapturePort` (D-03); keep Skrivli mixer for gains/meters/degrade.
3. **Audio buffer implementation:** use a proven SPSC implementation unless custom time-stamped packet semantics demand a bespoke ring; test contention and overrun behavior.
4. **Format conversion:** verify negotiated shared-mode capture formats and preferred fallback resampler, rather than assuming auto-conversion is universal.
5. **Tray event bridge:** determine safe native HWND/message registration and shutdown interaction with the chosen GPUI Kit / GPUI Windows backend.
6. **Model manifest:** pin actual model URLs, license terms, hashes and formats before exposing downloads.
7. **Recording file maximum:** decide between RIFF-safe duration cap/rotation and RF64 or equivalent for long sessions.

The implementation should favor the smallest working architecture, document real deviations in `.specs/`, and keep `AGENTS.md` and this design synchronized as decisions are validated.
