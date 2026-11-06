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
| Device format conversion | flexaudio normalizer to 48 kHz stereo `f32`. The pipeline's Rubato nudge runs on the session thread | Internal 48 kHz stereo `f32`; device ids from `flexaudio::devices()` |
| Audio pipeline | `Pipeline` on the session thread, bounded SPSC buffers | Align, mono microphone downmix, echo cancellation, sonora microphone gain, fixed system gain, sum, soft limit, peak taps |
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
 │ app/bootstrap + shutdown + settings_writer + bridge + menus           │
 │   (composition root: depends on views; views do not import app)       │
 │ views/main_window + views/{actions,state,hooks}                       │
 │  ├─ views/recording     ← ui/components/{level_meter,...}             │
 │  ├─ views/recordings    ← library, playback + transcript              │
 │  ├─ views/settings      ← preferences                                  │
 │  └─ views/model_prompt  ← choose/download model                        │
 │ shell commands → service hosts; service events → presentation snapshots│
 └─────────────────────▲──────────────────────────┬───────────────────────┘
                       │ structured events         │ commands
 ┌─────────────────────┴──────────────────────────▼───────────────────────┐
 │ services/recording  services/playback  services/level_monitor  …       │
 │    session lifecycle   PlayerHost        idle meters                   │
 └──────────┬───────────────────────────┬───────────────────┬──────────────┘
            │                           │                   │
 ┌──────────▼───────────────────────────▼───────────────────▼──────────────┐
 │ infrastructure/                                                      │
 │  audio: flexaudio mic + loopback → SPSC → Pipeline → peak snapshots │
 │  files: WAV writer / reader / cpal output / writability probe / FLAC │
 │  ai: decode → 16 kHz mono → whisper.cpp → transcript                  │
 │  storage: settings / library / sidecars (no OS APIs)                 │
 │  platform/: shell, tray, trash, watch, replace, clocks, disk, wake   │
 └───────────────────────────────────────────────────────────────────────┘
                        domain/: AudioBlock, RecordPhase, ThemePreference,
                                 formats, timeline, recording metadata
```

`domain/` contains no GPUI, Windows, COM, file I/O or AI dependency. Shared pure types such as `AudioBlock`, `RecordPhase`, and `ThemePreference` live there. `services/` express use cases and depend on small ports where implementation interchangeability or testing warrants them. `infrastructure/` implements adapters (including the recordings-folder writability probe under `files/`). OS calls are forbidden outside `infrastructure/platform/<os>/`. `app/` is the process composition root: it wires the platform host, workers, and hooks into `views/`. Views own the shell reducer (`Presentation`, `AppCommand`, bridge drain) and call injected save/exit hooks; they do not import `app/`. UI rendering must never own a capture handle or call Whisper.

### 4.2 Threads and communication

| Execution context | Responsibilities | Constraints |
| --- | --- | --- |
| GPUI foreground | Windows, entities, views, focus, menu state, message presentation | No blocking audio/file/model operations |
| Microphone capture thread | flexaudio stream worker and timeline mapping | Short bounded packet processing, no UI calls. Drift resampling is on the session thread |
| Loopback capture thread | Separate COM apartment + loopback event loop | Handle silence, discontinuity and device invalidation |
| Pipeline/writer thread | Align 10 ms blocks, run the chains, write the file, publish meter snapshots | Bounded latency; protect recording on I/O error |
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

### 4.4 Adding a platform

Ship another OS only when that target is committed. v1 is Windows only. The compiler target selects the host. `infrastructure/platform/mod.rs` is the only `target_os` branch in the app. `#[cfg(windows)]` compiles `platform/windows/`. A Linux port adds `platform/linux/` and a `#[cfg(target_os = "linux")]` re-export of the same function names. Use `target_os`, not `cfg(unix)`, because macOS is also Unix. Do not use a Cargo feature as the OS switch.

One binary contains one host. `cargo build` on Windows compiles only the Windows module. `cargo build --target x86_64-unknown-linux-gnu` (or a native `cargo build` on Linux) sets `target_os = "linux"`, type-checks `platform/linux/`, and does not type-check `platform/windows/`. OS-only crates follow that: `[target.'cfg(windows)'.dependencies]` for the `windows` crate, and `[target.'cfg(target_os = "linux")'.dependencies]` for Linux crates. `build.rs` already keys off `CARGO_CFG_TARGET_OS`, which matches the target triple during a cross compile.

Do not sprinkle `cfg` through views, services, domain, or storage. If a call needs the OS, it belongs on the host surface in `services/ports.rs` (traits) or as a free function on `infrastructure::platform` (`replace_file`, `user_paths`, `spawn_device_watch`, `clock`, `disk`, `performance_counter`). `domain/` stays free of GPUI, OS APIs, file I/O, and AI types.

**Implement these pieces for the new OS** before that OS can record, hide to tray, and manage the library. Behavior must match the Windows product promises (trash rather than permanent delete, one running instance, close-to-tray, atomic settings and sidecars).

- **Desktop shell.** Folder picker, reveal the recordings directory in the file manager, and show/hide/minimize the main window. Use GPUI’s window API when that backend’s hide and minimize actually work. The Windows path stays `ShowWindow` because gpui-pre 0.3.11’s Windows hide is a no-op (D-01).
- **Tray.** Status icon, the same commands as `HostCommand` / `AppCommand` (show/hide, start/stop, open folder, settings, exit), and a best-effort session-end signal so logoff still finalizes a recording. Re-add the icon if the shell restarts. Windows uses `Shell_NotifyIconW` on a private message window, not a GPUI subclass.
- **Single instance.** An OS lock plus a way for the second process to ask the first to show itself, then exit without touching a `.partial` file. The guard drops on the thread that acquired it.
- **Trash.** Move the recording and its `.json` / `.txt` sidecars to the system trash. Windows: Recycle Bin via `SHFileOperationW`.
- **Directory watch.** Debounced `Changed`, plus `Poll` when notifications are unavailable. Missing folders are polled until they appear. Windows uses `FindFirstChangeNotificationW`. A later replacement may use the `notify` crate behind this same trait; do not call `notify` from `services/library.rs`.
- **Atomic replace.** `replace_file` must replace an existing destination and remove the temp name. Windows uses `ReplaceFileW` because `std::fs::rename` fails when the destination exists. Unix rename replaces; that body is written only with the Unix host.
- **User directories.** Fill the three bases `AppPaths::from_bases` already takes. Windows: `%APPDATA%`, `%LOCALAPPDATA%`, `%USERPROFILE%`. A macOS host maps Application Support, Logs, and the home directory. A Linux host maps XDG config, state, and home. Default recordings stay under Documents via `AppPaths::recordings_dir`.
- **Recording clock, performance counter, free space.** `RecordingClock::local` is civil local time. `PerformanceCounter::sample` returns `(ticks, ticks_per_second)` for the existing timeline in `domain/chunk_plan.rs` (Windows: QPC). `FreeSpace` reports bytes available for the recordings directory (Windows: `GetDiskFreeSpaceExW`).
- **Capture.** Do not write a new `CapturePort` for Linux or macOS. flexaudio already selects the backend (cpal mic; PipeWire, WASAPI, or macOS system loopback) and normalizes to 48 kHz stereo `f32`. Keep `infrastructure/audio/flex_capture.rs`. Pass that OS’s `PerformanceCounter` into the existing adapters so the worker does not call QPC. Re-enumerate with `flexaudio::devices()` into the same `DeviceCatalog`. Implement `spawn_device_watch` for that OS: Windows needs `IMMNotificationClient` because flexaudio’s `DeviceWatcher` is a no-op except on Linux; an OS where `flexaudio::watch_devices` reports events should use that and still call `enumerate_catalog`. Do not change the pipeline. A capture adapter under `platform/<os>/` is only for a target flexaudio does not support.
- **Playback.** `services/playback.rs` owns the play/stop command loop (`PlayerHost`). `files/player.rs` opens the cpal output session. Keep cpal’s default host unless that host cannot play PCM16 WAV; a special output adapter then belongs in `platform/<os>/` behind a small port used by `files/player.rs`.
- **Packaging.** Windows icon and manifest stay in `build.rs`. Add that OS’s bundle, `.desktop` file, or equivalent beside it. Do not compile `winresource` for the new target.
- **Not built yet.** When autostart, global hotkey, or whisper.cpp land, their OS APIs and native build flags go under `platform/<os>/` (or that crate’s build script), behind a port in `services/ports.rs`. Services do not link shell or inference libraries themselves.

**Prove it**

- `cargo test` on that host uses fakes for capture, clock, disk, trash, and watch. It does not open a microphone.
- `cfg` the tests that need a real shell, tray, or loopback device.
- Update `skrivli-product.md` only when the OS is actually something a user can run. Until then this section is the contract and the product stays Windows.
- Run `cargo fmt` and `cargo clippy --all-targets -- -D warnings` on the new target.

**Do not**

- Do not put a second recording state machine in the tray.
- Do not claim `Send` or `Sync` for COM or other apartment-bound wrappers.
- Do not log audio samples or transcript text.
- Do not copy Win32 paths (`Local\Skrivli.SingleInstance`, `%LOCALAPPDATA%\Skrivli\rescue\`) into another OS. The rescue directory comes from `AppPaths::rescue_dir()`.

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
│   │   ├── bridge.rs               # background shell pulse
│   │   ├── menus.rs
│   │   ├── runtime.rs              # menu dispatch onto MainWindow
│   │   ├── settings_writer.rs
│   │   └── shutdown.rs
│   ├── domain/
│   │   ├── mod.rs
│   │   ├── audio.rs                # formats, AudioBlock, endpoints
│   │   ├── chunk_plan.rs           # QPC session timeline (48 kHz); transcript windows live in transcript.rs
│   │   ├── recording.rs            # metadata + RecordPhase
│   │   ├── theme.rs                # ThemePreference
│   │   └── transcript.rs           # segments, text formatting
│   ├── services/
│   │   ├── mod.rs
│   │   ├── recording.rs            # one recording lifecycle
│   │   ├── audio_pipeline/         # Pipeline, aligner, chains
│   │   ├── recording_harness.rs    # fake capture for tests
│   │   ├── playback.rs             # PlayerHost command loop
│   │   ├── level_monitor.rs
│   │   ├── transcription.rs        # queued jobs
│   │   ├── models.rs               # user-initiated model download host
│   │   └── ports.rs                # external capability traits, as needed
│   ├── infrastructure/
│   │   ├── mod.rs
│   │   ├── audio/
│   │   │   ├── mod.rs
│   │   │   ├── devices.rs          # flexaudio catalog (no Win32)
│   │   │   ├── flex_capture.rs
│   │   │   ├── resampler.rs
│   │   │   ├── sonora_steps.rs     # only production import of sonora
│   │   │   └── format.rs
│   │   ├── ai/
│   │   │   ├── mod.rs
│   │   │   ├── whisper.rs
│   │   │   ├── models.rs
│   │   │   └── fetch.rs            # HTTP ModelSource; tests never construct it
│   │   ├── storage/
│   │   │   ├── mod.rs
│   │   │   ├── settings.rs
│   │   │   ├── library.rs
│   │   │   └── sidecar.rs
│   │   ├── files/
│   │   │   ├── mod.rs
│   │   │   ├── wav.rs
│   │   │   ├── flac.rs
│   │   │   ├── reader.rs
│   │   │   ├── player.rs           # cpal output session
│   │   │   └── writable.rs         # recordings folder probe
│   │   └── platform/
│   │       ├── mod.rs              # cfg(windows) → windows; else compile_error
│   │       └── windows/            # only tree allowed to use the windows crate
│   │           ├── mod.rs          # host() + free functions
│   │           ├── shell.rs
│   │           ├── tray.rs
│   │           ├── single_instance.rs
│   │           ├── trash.rs
│   │           ├── watcher.rs
│   │           ├── replace.rs
│   │           ├── paths.rs
│   │           ├── time_and_disk.rs
│   │           └── device_wake.rs
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
│   │   ├── actions.rs              # AppCommand, bridge drain
│   │   ├── state.rs                # Presentation
│   │   ├── hooks.rs                # save/exit hooks from app
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
| `app/` | Process composition root: GPUI Kit bootstrap, worker wiring, shutdown, settings writer |
| `domain/` | Audio formats, `AudioBlock`, `RecordPhase`, `ThemePreference`, timeline, recording metadata |
| `services/` | Recording sessions, playback host, idle level monitoring, transcription jobs, model download host |
| `infrastructure/audio/` | flexaudio capture adapters, device catalog, the 48 kHz drift resampler, and the sonora echo and gain steps (no OS APIs). Production code imports `sonora` only in `sonora_steps.rs` |
| `infrastructure/ai/` | Whisper engine integration and local model management |
| `infrastructure/storage/` | JSON settings, recording library and sidecars (calls `platform::replace_file`) |
| `infrastructure/files/` | WAV writer/reader, cpal output session, writability probe, optional FLAC |
| `infrastructure/platform/` | Sole home for OS APIs: shell, tray, trash, watch, replace, clocks, disk, device wake |
| `ui/` | Reusable GPUI Kit compositions and custom GPUI elements such as level meters |
| `views/` | GPUI screens, shell reducer (`Presentation` / commands), window composition |
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
- A capture worker drains all available packets for each event, converting `AUDCLNT_BUFFERFLAGS_SILENT` to zero frames and retaining first-frame QPC/device timestamps. On `DATA_DISCONTINUITY`, record a gap marker and let the pipeline repair the timeline.
- No synchronous filesystem I/O, GPUI entity operations, blocking locks, model calls or unbounded allocations in the hot capture loop.
- Audio packets move via bounded SPSC buffers (or an equally suitable proven crate) to the pipeline. Define overflow and backpressure handling: report overrun, drop according to a documented policy and preserve elapsed timeline with gaps rather than indefinitely blocking capture.
- Echo cancellation is app-side, in the session pipeline, not in the capture worker. It runs whenever system audio is being captured. It is always on, and there is no setting to turn it off.

### 6.3 Silence and loopback keep-alive

System loopback uses flexaudio’s classic silent-render keepalive so idle endpoints keep delivering silence on the device clock. The pipeline must **never infer that the recording clock has stopped**. Use session-timeline alignment (`frame_index` anchored at open) and synthesize silence for remaining gaps. Timestamp-based gap filling remains required even with keepalive.

### 6.4 Timeline, mixing and clock drift

- Session time origin `t0`: monotonic QPC timestamp at start.
- Convert packet timestamps into positions on the 48 kHz sample timeline, taking timestamp-unit conversions and device latency into account.
- Operate in **10 ms / 480-frame** output blocks. A source gap of roughly 5 ms becomes silence; duplicated/overlapping late frames are discarded according to a deterministic rule.
- Emit when sufficient data exists or when a block is more than about **150 ms** behind real time; a stalled input contributes silence. These thresholds are initial values and should be validated by tests on actual devices.
- Two devices have independent hardware clocks. flexaudio converts each device's rate to 48 kHz stereo `f32`. Drift correction is a per-source Rubato resampler at 48 kHz to 48 kHz, owned by the pipeline and run on the session thread. It reads that source's timeline error from the aligner directly. There is no atomic shared with the capture worker. Sudden holes still become silence; sudden overlaps are still discarded.
- Each output block is aligned, then the microphone is downmixed to mono (`(L + R) / 2`) before the microphone chain and copied onto both channels of the stored file. When system audio is captured, that chain is echo cancellation, then sonora microphone gain, then the microphone peak tap. When system audio is off, there is no canceller. The system chain is a fixed gain of `0.8`, then the system peak tap. The frames are summed, then a soft limiter, then the mixed peak tap. Echo cancellation is always on while system audio is captured, and there is no setting to turn it off. The file stores that processed mix. If the gain step cannot be created, that recording keeps the microphone at unity.
- Publish microphone, system, and mixed peak snapshots for the meters, not per-sample UI events. During a recording the microphone meter reads after echo cancellation and gain, the system meter after the fixed gain, and the mixed snapshot after the limiter. The Record tab draws the microphone and system meters. Idle meters stay outside the pipeline and still use peak times the slider, so the microphone meter can change when a recording starts on speakers.
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
- `services/models.rs` `ModelHost` owns that download. `app/bootstrap.rs` spawns it and `app/shutdown.rs` joins it, same as `TranscriptionHost`. The view never hashes a file or opens a socket. Bytes come through `ModelSource` on `services/ports.rs`; `infrastructure/ai/fetch.rs` is the HTTP adapter and tests inject a reader. Progress is one coalesced `ModelSnapshot` on the bridge (latest wins), reduced into `Presentation`.
- Browse calls `DesktopShell::pick_model_file` in `infrastructure/platform/windows/shell.rs`. The host then validates and copies off the GPUI thread. Missing, hash-mismatch (`verify model`), and unsupported files each surface `AppError::user_message()` in a Kit alert.
- Downloads must be **user-initiated only**, with progress, resumability where supported, and **SHA-256 verification against a maintained trusted manifest** before a file is accepted. Download to a temporary file and rename after verification. M7.1 downloads the default manifest variant and states the file size and that a network is required. Choosing another variant in Settings is M8.
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
- `views/state.rs` `Presentation` reduces `TranscriptionSnapshot` into the status strip and the selected row. A progress tick keeps the loaded transcript string and the selected `RecordingId`. Completion sets `TranscriptStatus::Ready` and the detail text without changing the tab. Playback still stops only when the selection changes.
- Failed detail in the pane is `AppError::user_message()`. The status strip may keep the existing static notices (`Transcription failed.`, and the rest).
- Each job binds its `RecordingId`, audio path, and `.txt` path when it leaves `Queued`. `replace_file` for the transcript runs only when those paths still belong to that id. `LibraryHost::rename` and `LibraryHost::delete` call `JobIdentity` before they change files. A queued job for that id is cancelled and writes nothing. A rename that commits before the write retargets the same id (the bound audio and `.txt` paths follow the new names). Otherwise the job fails and writes nothing. Delete cancels a queued or running job, then the `Trash` port receives the audio, the sidecar, and a `.txt` the job has already promoted.
- On completion, the service emits one structured event (`TranscriptNotifier::transcript_ready` on `services/ports.rs`) when `Settings.notify_on_transcript` is on (default on, `serde(default)`, schema stays 4). The Windows adapter (`infrastructure/platform/windows/notify.rs`) only queues the `RecordingId`. The tray thread, which already owns `Skrivli.TrayMessage` and the `Shell_NotifyIconW` icon, shows one `NIF_INFO` balloon. The inference callback does not call Win32 or show UI. Library state still updates through the existing bridge. Activating the balloon posts `HostCommand::ShowRecording`, which shows the window and selects that `RecordingId` only when the latest library snapshot still contains the row. If the row is gone, Show still runs and the selection stays as it was.
- Model missing/corrupt, decode error, OOM and cancellation are represented as distinct user-facing results. Preserve audio on all errors.

---

## 8. GPUI Kit user interface design

**Component policy:** GPUI Kit is mandatory for standard widgets and styling. Custom raw GPUI elements are for screen composition and app-specific controls (especially audio meters); native Windows UI is reserved for OS integrations.

### 8.1 Main window

- Resizable GPUI Kit application window, minimum target roughly **560 × 420 logical pixels**, with remembered dimensions and monitor-safe restore.
- Two top tabs: **Record** and **Recordings**. Use GPUI Kit navigation/tab components where they meet requirements, **not** Win32 `CTabCtrl`/`CDialogImpl` child windows.
- Adopt **GPUI Kit themes, design tokens, typography, buttons, inputs, selectors, tabs, menus, dialogs, tooltips, and other styled controls** as defaults. Use GPUI layout primitives (`div`, flex/grid) to compose screens; avoid duplicating GPUI Kit widgets in custom code.
- No in-window File/Help menu bar. `TabBar::suffix` holds a theme-cycle button (left) and Settings gear (`IconName::Settings`). The theme button uses Kit component icons for Light (`Sun`) and Dark (`Moon`). System uses `IconName::Palette` for follow-system. Tooltip names the current preference (Light / Dark / System); preference is the domain `ThemePreference` type persisted via `Settings::theme`. `ThemePreference::next(system_is_dark)` drives the button as a three-slot cycle with Light/Dark swapped from `window.appearance()`: dark OS `[System, Light, Dark]`, light OS `[System, Dark, Light]`. System mode calls `Theme::sync_system_appearance` and re-syncs on `window.observe_window_appearance`; Light and Dark call `Theme::change`. About is linked from the Settings dialog. Open recordings folder and Exit stay on the Recordings tab and tray.
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
│               [ ● Start recording ]              │
│                     00:12:34                     │
│                      ● REC                       │
│ Mic  ███████████░░░  −12 dB   Gain ──●── 1.0×       │
│ System █████░░░░░░░░░░░░░       −24 dB           │
│                                                  │
│ [inline device/permission/disk warning]          │
└──────────────────────────────────────────────────┘
```

- Compose **GPUI Kit** select/combo, button, input, slider, checkbox, status, and dialog components for the recording form. Implement custom `level_meter` (and specialized recording indicator only if needed) using GPUI primitives and GPUI Kit theme tokens: band fills from `success` / `warning` / `danger`, track from the same color at low opacity (Kit Progress pattern), labels from `foreground` / `muted_foreground`. Do **not** create a parallel theme system; use GPUI Kit tokens and component styling.
- The Record tab does not show the save path or a Browse control. Folder selection lives in Settings (read-only path plus Browse via the Windows folder-picker adapter). Default `%USERPROFILE%\Documents\Skrivli\` where writable.
- Disable source selection while recording. The gain slider stays enabled during capture and is disabled only while saving.
- A red recording state, elapsed timer and status text; ensure information is accessible without relying on red color or a blinking indicator alone.
- Meter data: during a recording, microphone peak after echo removal and gain, and system peak after the fixed system gain, on −60..0 dBFS, with a green→amber→red range and a ~1 s peak hold. Draw from throttled snapshots. Idle monitoring stays outside the pipeline, uses peak times the slider, and hands the devices to the recording session when capture starts.
- Missing devices/unwritable destination show inline GPUI Kit feedback; material failures use GPUI Kit dialogs or a Windows TaskDialog through a platform adapter when specifically required.
- Global toggle shortcut default `Ctrl+Alt+R` using Windows `RegisterHotKey`; allow disable/rebinding and handle conflicts explicitly.

### 8.3 Recordings tab

- Responsive **split-pane** layout: recording list left, selected detail right. At narrow widths, a stacked/detail-navigation fallback is acceptable.
- List columns: **Name | Date | Duration | Size | Transcript**. Sort newest first; clickable sorting, keyboard selection, stable selection keyed by recording ID/path.
- Transcript statuses: `—`, `Queued`, `42%`, `Ready`, `Failed`.
- Right pane: empty state until selected; then **Play / Stop**, file metadata, and a finished transcript in GPUI Kit `Editor` with `readonly(true)` (`EditorState`, gpui-kit 0.9.0). That editor can select, select-all, copy, and move by keyboard. `<basename>.txt` is read on `background_spawn` and stored on the entity. A progress tick does not recreate the entity or call `set_value` (that call resets scroll). A missing `.txt` on a Ready row shows `user_message`, not a panic. A plain `div().child(text)` is not the transcript view.
- Actions: Play, Transcribe, Cancel, Retry, Model, Open folder, Rename, Delete, Refresh. Transcribe is disabled while that recording is queued or running. Cancel is enabled only then. Retry is a `Transcribe` command for a row whose status is Failed. The table transcript cell and the status strip update in place; the `DataTable` entity, sort, and selection stay. Delete goes to the Windows Recycle Bin through a Windows Shell adapter and includes associated audio/`.txt`/`.json`; ask for confirmation and handle partial failures. Rename and delete notify the transcription host before files change. A queued or running job for that recording is cancelled, or a rename that lands before the transcript write retargets the same id.
- Double-click/Enter can start playback; selection change stops playback by design.
- Background directory watcher debounces changes; the library scans WAV/FLAC files and matches sidecars/transcripts. Preserve selected recording after refresh.
- Playback audio runs through `services/playback` and the cpal session in `files/player`; never decode files or drive playback from GPUI rendering.

### 8.4 Model prompt and settings

**Model prompt:** `views/model_prompt.rs`, opened from the Recordings detail pane. Lists models `discover` has already accepted. Browse, Download (default manifest variant, with size and a network notice), Cancel, and Retry. Download, cancel, and retry are `AppCommand` effects the window hands to `ModelHost` off `render`. Browse is `DesktopShell::pick_model_file`. Verification errors use Kit alerts.

**Settings:** recording directory, default endpoints, microphone gain, optional stems, FLAC and WAV retention; transcription model, language, CPU threads, VAD, auto-transcribe; close-to-tray, Windows autostart, start hidden and global hotkey. Validate choices before persisting, and apply mutable settings without restarting where safe.

### 8.5 System tray (native Windows integration)

- The tray is **not a GPUI widget**: implement Windows `Shell_NotifyIconW` behind `infrastructure/platform/windows/tray.rs`, potentially using a supported tray crate if it fulfills shell lifecycle requirements.
- Idle/recording icons and tooltip `Recording HH:MM:SS`.
- Left click toggles window visibility. Right-click menu: **Show Skrivli**, **Start/Stop recording**, **Open recordings folder**, **Settings**, **Exit**.
- Minimize hides to tray. Close hides to tray by default (configurable), with a one-time message explaining the app is still running. Exit is explicit.
- Register notification icon using a stable GUID and current notify-icon version where feasible. Re-add after Windows Explorer restarts (`TaskbarCreated`).
- Notifications: transcription completed uses one balloon (`NIF_INFO`) on the existing tray icon when `notify_on_transcript` is on. The click shows the window and selects that recording when the library still has the row. Low disk space and device recovery stay later work. Respect notification availability, user settings and OS focus assist.
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
- Pipeline and aligner: QPC alignment, silent intervals, overlapping and late packets, bounded wait, 100 ppm simulated drift, microphone and system gain, the soft limiter, mono downmix, and echo-reference order.
- Resampler: the capture nudge is 48 kHz to 48 kHz and reads the aligner error directly. Transcription uses a separate 48 kHz to 16 kHz path: a 1 kHz sine retains pitch and amplitude, content above 8 kHz is attenuated, and stereo downmix is checked.
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
| Drift between independent devices | Unit-test 100 ppm deviations; the pipeline reads aligner error and nudges each source's Rubato ratio |
| Driver rejects requested format/conversion | Probe `IsFormatSupported`/mix format; test alternate converter path |
| Long WAV exceeds RIFF 32-bit size | Monitor file length and stop/split or use compatible large-file format |
| Speaker echo | Echo removal is always on while system audio is captured. There is no off switch. Double-talk can reduce the user's own voice while the far side is talking. The stored file is the processed mix, so a bad result cannot be recovered afterwards. |
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
- Consider more platform backends only when there is a committed non-Windows target. Follow section **4.4 Adding a platform**.

---

## 15. Design decisions requiring validation

These are deliberate implementation choices to settle during M0–M3, not implicit claims of already-working APIs:

1. **GPUI Kit version and Windows backend:** confirm `Application`/window creation, focus, close interception, timers/tasks, text selection, and exact event subscription API with a pinned revision.
2. **Capture stack:** flexaudio 0.8 behind `CapturePort` (D-03). The session runs `Pipeline`: align, mono microphone downmix, echo cancellation when system audio is captured, sonora microphone gain, fixed system gain, sum, soft limit, and peak taps. Echo removal is always on in that case and has no setting. The stored file is the processed mix. `mixer.rs` is gone.
3. **Audio buffer implementation:** use a proven SPSC implementation unless custom time-stamped packet semantics demand a bespoke ring; test contention and overrun behavior.
4. **Format conversion:** flexaudio converts the device rate to 48 kHz stereo `f32`. The pipeline resampler is 48 kHz to 48 kHz and reads the aligner's error directly (D-06).
5. **Tray event bridge:** determine safe native HWND/message registration and shutdown interaction with the chosen GPUI Kit / GPUI Windows backend.
6. **Model manifest:** pin actual model URLs, license terms, hashes and formats before exposing downloads.
7. **Recording file maximum:** decide between RIFF-safe duration cap/rotation and RF64 or equivalent for long sessions.

The implementation should favor the smallest working architecture, document real deviations in `.specs/`, and keep `AGENTS.md` and this design synchronized as decisions are validated.
