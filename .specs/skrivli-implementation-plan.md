# Skrivli — Milestone-Based Implementation Plan

**Target:** Windows 10/11, x86_64-pc-windows-msvc  
**Stack:** Rust stable, GPUI Kit/GPUI, flexaudio (WASAPI/cpal), local whisper.cpp  
**Source of truth:** `.specs/skrivli-product.md` for what the user can do; `.specs/skrivli-app-design.md` for architecture; `AGENTS.md` for source placement, layering, and coding conventions.  
**Status:** Execution backlog; tasks unchecked until implemented and verified.

## How to execute this plan

- Implement milestones **M0 → M9** in order. Within each milestone, complete numbered work packages in order unless a dependency is noted. Each package should be an independently reviewable PR or small sequence of commits.
- **Definition of done for every package:** code builds on pinned Windows toolchain, automated tests for testable logic, no warnings from `cargo clippy --all-targets -- -D warnings`, formatted source, typed errors at external boundaries, explicit resource ownership and shutdown, and updated documentation if the architecture changed.
- **Release gates are empirical:** a passing unit test is not evidence that GPUI Kit, WASAPI, Windows Shell, `flac-codec`, or whisper.cpp works on Windows hardware. Run manual checks at the designated gates.
- Use one Cargo package (`src/main.rs` and `src/lib.rs`) initially. Do not introduce a workspace, database, custom UI library, custom lock-free queue or general dependency-injection framework without demonstrated need.
- Prefer GPUI Kit's version-matched GPUI and standard components. Verify actual APIs against the **pinned** source before writing integrations. No API signatures or library capabilities described here are presumed verified.
- Maintain `.specs/decisions.md` (or small ADR files) for unresolved choices, outcome, tested toolchain/device and rationale. Record the exact commit/version of GPUI Kit and whisper bindings when confirmed.

### Cross-cutting contracts to establish early

1. **Layering:** `domain` has no platform, GPUI, file I/O, or AI types. `services` own use cases and depend on traits in `services/ports.rs`. `infrastructure` implements those traits. OS calls stay in `infrastructure/platform/windows/` (selected only from `infrastructure/platform/mod.rs`). `app/` is the composition root (`bootstrap`, `bridge`, `menus`, `runtime`, `settings_writer`, `shutdown`) and does not own the shell reducer. `views/` owns `Presentation`, `AppCommand`, and `ShellHooks` and does not import `app/`. `ui/` is Kit overrides and custom controls.
2. **Thread ownership:** GPUI foreground must never perform capture, decoding, disk scanning, model inference or blocking joins. Every worker has an owner, cancellation/shutdown signal and join strategy; COM objects stay in the correct apartment.
3. **Commands/events:** a single command path drives toolbar, tray and hotkey. Use discrete structured events, coalesce high-rate progress and meter snapshots and marshal all UI state changes to the GPUI executor.
4. **Persistence:** settings and JSON sidecars are schema-versioned and written using a temporary sibling file, flush/sync as appropriate, then replace/rename; handle Windows replacement semantics and recovery errors explicitly.
5. **Recording clock:** all audio events map to a monotonic QPC-based session timeline; absence of packets means source silence/gap, **not** elapsed-time suspension.
6. **Safety:** do not log audio or transcript contents. Preserve the primary WAV on any transcription or FLAC failure. Never delete usable partials without explicit recovery/cleanup logic.

### Proposed application-facing types (design sketches)

The live command path is `views/actions.rs` (`AppCommand`, including `Transcribe` and `CancelTranscription`) and `services/ports.rs` (`HostCommand` from the tray). Recording phase is `domain/recording.rs` `RecordPhase`. Library, playback, and recording publish snapshots (`LibrarySnapshot`, `PlayerSnapshot`, `RecordSnapshot`) rather than one shared event enum. The sketches below are the original shape; new jobs extend those modules instead of adding a second command type.

```rust
pub enum AppCommand {
    StartRecording,
    StopRecording,
    Transcribe { recording_id: RecordingId },
    CancelTranscription { job_id: JobId },
    RefreshLibrary,
}

pub enum RecordingState {
    Idle,
    Starting,
    Recording { started_at: SessionInstant },
    Stopping,
    Error { recoverable: bool },
}

pub enum AppEvent {
    RecordingStateChanged(RecordingState),
    RecordingSaved { recording_id: RecordingId },
    RecordingFailed { message: String },
    DevicesChanged,
    LibraryChanged,
    TranscriptionProgress { job_id: JobId, percent: u8 },
    TranscriptionCompleted { recording_id: RecordingId },
}
```

Define `RecordingId`, `JobId`, timestamp units and error types in their owning modules. Keep UI-facing error messages separate from diagnostic `tracing` context.

---

## M0 — Foundation and architecture validation

**Goal:** A bootable, testable Windows GPUI Kit application with baseline infrastructure.  
**Dependencies:** None. **Exit deliverable:** clean executable opens a themed GPUI Kit window, exits correctly, and passes Cargo checks.

### M0.1 — Repository and toolchain

- [x] Initialize one binary/library Cargo package (`src/main.rs`, `src/lib.rs`), commit `Cargo.lock`, `rust-toolchain.toml`, `.gitignore`, `README.md` and existing `AGENTS.md`/`app-design.md`.
- [x] Add feature-oriented module roots `app`, `domain`, `services`, `infrastructure`, `ui`, `views`, `common`; only create concrete submodules when used.
- [x] Configure MSVC target and Windows SDK prerequisites; document reproducible setup in README.
- [x] Add CI on a Windows runner for `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, and `cargo build --release`; no hardware-dependent tests in default CI.

**Validate:** fresh clone builds with documented prerequisites; `cargo test` succeeds without device access.

### M0.2 — GPUI Kit Windows compatibility spike (blocking)

- [x] Select and pin a GPUI Kit revision and bundled GPUI runtime; start from its official minimal sample, **not** guessed bootstrap APIs.
- [x] Create/close a real window, initialize GPUI Kit, apply a theme, draw a Kit button/select, and verify input/focus/keyboard interaction.
- [x] Check supported foreground task/event scheduling, `Render` lifecycle, close interception, selection/editor behavior, overlay/dialog positioning, timers, and window sizing on Windows.
- [ ] On a Windows 10/11 machine, run the shell at 100% and 150% scaling and with the window moved to a second monitor. Per-monitor DPI awareness is already the gpui-pre manifest (`PerMonitorV2`); `resources/skrivli.manifest` documents that contract and is not linked, because a second manifest fails the link (D-01). Confirm logical-pixel layout at the 560×420 minimum: tab bar, theme-cycle button, Settings gear, Record and Recordings content, Kit dialogs (Settings, About, tray explanation, exit-while-recording), and the level meter stay inside the window and keep a visible focus ring. Move the window between monitors, quit, and confirm `domain/window_frame` restores into `PlatformDisplay::visible_bounds` instead of opening off-screen. Write the measured scale factors, any Kit overlay or focus bugs, and the workaround into `.specs/decisions.md`.

**Decision gate:** if any required Windows window/keyboard/text API is missing, record a targeted adapter/workaround and verify it **before** committing to dependent UI work.

### M0.3 — Common runtime, settings and Windows resources

- [x] `common/error.rs`: typed application errors with contextual diagnostics and user-safe messages; `common/paths.rs`: Roaming AppData settings, Local AppData logs/models/rescue and Documents recording defaults.
- [x] `common/logging.rs`: `tracing` initialization, bounded rotating logs (initially ~1 MB × 3), no captured content in logs, panic diagnostic hook.
- [x] `infrastructure/storage/settings.rs`: default `Settings` struct, schema version, `serde_json` load/validate/migrate-or-fallback, atomic save and temp-directory unit tests.
- [x] Add `resources/` icon, manifest, version metadata and `build.rs` only if needed; document DPI awareness configuration.
- [x] Specify single-instance named mutex and second-instance activation handshake (including startup race); implement basic guard now, complete activation in M1.
- [x] Document COM initialization per capture/Windows worker, foreground ownership, command/event bridge, shutdown order and error propagation.

**Validate:** settings round-trip and malformed-file fallback, distinct process cannot concurrently write recording state, clean process shutdown.

### M0 acceptance

- [x] A real Windows 10/11 machine runs the packaged debug executable with GPUI Kit content.
- [x] Cargo format/clippy/test/release-build checks pass; GPUI Kit version and known Windows limitations are documented.

---

## M1 — Application shell, navigation and tray lifecycle

**Goal:** Usable desktop shell with a single coherent app lifecycle; no recording hardware dependency yet.  
**Dependencies:** M0 GPUI Kit and runtime validation.

### M1.1 — Application composition and action bridge

- [x] `app/bootstrap.rs`: initialize GPUI Kit, create application context, own service handles and window lifetime.
- [x] `app/state.rs`: selected tab, window bounds, status summary and presentation snapshots. `app/actions.rs`: typed app intents wired to command bridge.
- [x] Implement worker-to-GPUI event delivery through a supported foreground scheduling mechanism; add a fake event producer to prove cross-thread changes can safely rerender.
- [x] Establish shutdown controller with idempotent request/acknowledge/join; initial workers may be fakes.

### M1.2 — GPUI Kit shell views

- [x] `views/main_window.rs`: resizable themed window, min target 560 × 420 logical px, status strip and Record/Recordings tabs.
- [x] `views/recording.rs`, `views/recordings.rs`: placeholder controls and empty states; `views/settings.rs`: placeholder dialog/pane.
- [x] Use GPUI Kit buttons, tabs, menus, inputs, dialogs and theme tokens. Keyboard navigation and focus must work; save/restore window bounds with monitor-safe clamping.
- [x] Implement File/Help actions: Settings, Open Recordings Folder, About, Exit (functional where already possible).

### M1.3 — Windows tray and instance handoff

- [x] `infrastructure/platform/windows/tray.rs`: native notification icon, stable GUID where appropriate, Show/Hide, Start/Stop (disabled until M2), Open Folder, Settings and Exit.
- [x] Integrate a supported message window/pump with thread-safe action handoff; never assume GPUI HWND subclassing is supported.
- [x] Implement configured close-to-tray/minimize behavior and explicit Exit, including first-use explanation.
- [x] Re-add tray icon on Explorer `TaskbarCreated`, remove icon on normal Exit, keep one authoritative lifecycle controller.
- [x] Complete second-instance activation (signal first process to focus/show; second exits without touching recordings).

**Validate:** close/minimize/show/hide repeated, tray action dispatch, taskbar restart, second-instance race, clean Exit without orphaned background threads.

### M1 acceptance

- [ ] Manual pass on Windows hardware, written up in `.specs/decisions.md` (D-02 still lists these checks as unrun). Switch Record and Recordings; cycle `ThemePreference` (System, Light, Dark) from the tab-bar control; open Settings from the gear and About from that dialog. The window has no File or Help menu. Resize and move the window, restart, and confirm bounds persist through `ShellHooks` into `app/settings_writer` (debounced, off `render`) and land on a visible monitor. Close and minimize with close-to-tray on, show again from the tray, and Exit. Tray clicks are `HostCommand` values mapped to `AppCommand` and drained on the GPUI thread; Exit removes the notify icon and `ShutdownController` joins workers after `application().run` returns. Restart Explorer and confirm `TaskbarCreated` re-adds the `Shell_NotifyIconW` icon and that menu still hits the same command path. Start a second process while the first window exists and once during startup: it signals `Local\Skrivli.Activate` and exits without a window and without touching a recording. Only the primary process owns `Presentation`.

---

## M2 — Microphone capture and first persistent WAV

**Goal:** Reliable microphone-only start/stop from UI, resulting in playable crash-conscious WAV.  
**Dependencies:** M1 command bridge and shell. **Primary vertical slice:** Record button → service → native capture → file → UI saved event.

### M2.1 — Audio domain, device discovery and folder choice

- [x] `domain/audio.rs`: `AudioFormat`, frame counts, monotonic session time and sample index conversion with checked integer arithmetic.
- [x] `infrastructure/audio/devices.rs`: enumerate `eCapture`/`eRender` endpoints via MMDevice; expose stable IDs and friendly display names; select Windows default communications microphone and default render endpoint.
- [x] Design `DeviceSelection` with explicit/default modes and missing-device fallback notification; persist stable endpoint choices.
- [x] `infrastructure/platform/windows/shell.rs`: native folder picker; validate write permissions and folder creation. Wire GPUI Kit device selectors in `views/recording.rs`. Destination Browse moves to Settings (not on the Record tab).
- [x] Test absent/disabled devices and microphone privacy denied. Do not allow endpoint/path modifications during active recording.

### M2.2 — Mic capture worker and normalization

- [x] `infrastructure/audio/flex_capture.rs`: flexaudio `SourceKind::Mic` / `SystemLoopback` behind `CapturePort`; worker owns the stream, maps `frame_index` onto the session timeline, and applies Rubato drift nudge.
- [x] Device catalog from `flexaudio::devices()`; COM `IMMNotificationClient` only wakes re-enumeration.
- [x] Bound memory and packet queues; no blocking disk writes, UI access, unbounded allocations or high-contention locks in hot loop.
- [x] Expose testable interfaces for synthetic packet input and a fake capture backend.

### M2.3 — WAV writer and recording service

- [x] `infrastructure/files/wav.rs`: streaming PCM16 stereo 48 kHz RIFF writer (`hound`) to collision-safe `Skrivli_YYYY-MM-DD_HH-MM-SS.wav.partial`; quantize/clamp `f32`, write/repair RIFF/data sizes periodically (~2 s), flush/finalize then rename.
- [x] Choose and document a RIFF boundary policy now: **safe stop or rotate** before 32-bit size overflow; do not write invalid headers. Include tests for boundary math without generating multi-GB files.
- [x] `services/recording.rs`: `Idle → Starting → Recording → Stopping → Idle/Error`, start readiness handshake, owned workers, bounded channel and orderly shutdown/drain/flush. Stop must be idempotent.
- [x] Ensure UI shows state/timer/errors and delegates to service, never directly opens capture. Persist basic sidecar info now if this makes recording identity stable; expand in M5.
- [x] Add `tests/recording_flow.rs` using fake capture, injectable clock and temporary recording directory.

**Validate:** one-minute mic WAV opens in a common Windows player; duration approximately matches wall clock, data/header lengths correct; repeated start/stop and startup failure clean up threads/files.

### M2 acceptance

- [ ] On Windows hardware, from the Record tab, choose a microphone (default communications device or an explicit flexaudio id) and a writable folder through `DesktopShell::pick_recordings_folder`. Start and stop go `AppCommand` → `RecordingHost`. The view does not open a capture stream. Stopping produces a finalized `Skrivli_YYYY-MM-DD_HH-MM-SS.wav` (the `.wav.partial` name is gone), a sibling sidecar replaced through `platform::replace_file`, and a duration close to the session clock. Quit, relaunch, and play that file in a normal Windows player and from the Recordings tab (`PlayerHost` plus `infrastructure/files/player.rs`). A refused start (privacy denial or unwritable folder) leaves no capture thread and no header whose sizes disagree with the bytes on disk. `cargo test` keeps using the fake `CapturePort` in `services/recording_harness.rs`.

---

## M3 — System loopback, synchronization, mixing and meters

**Goal:** Concurrent microphone/system recording with a continuous, synchronized timeline.  
**Dependencies:** M2 mic worker, WAV writer and service lifecycle.

### M3.1 — Loopback worker and per-source transport

- [x] `FlexSystemCapture`: flexaudio `SourceKind::SystemLoopback` with classic silent-render keepalive; separate worker from the mic stream.
- [x] Idle silence is delivered as keepalive packets; remaining gaps still filled from the session clock. M3.4 moved that fill into the aligner.
- [x] Select a proven bounded SPSC queue for each source. Define timestamped packet/block representation, maximum queue latency and overflow/drop/gap accounting. Do not invent a custom lock-free queue unless necessary.

### M3.2 — Deterministic mixer and drift correction

- [x] `domain/chunk_plan.rs`: pure mapping from QPC ticks to 48 kHz sample indices, with integer/rational conversion that avoids cumulative floating-point drift. Define session origin `t0` and treatment of latency, missing packets, overlaps and discontinuities.
- [x] `infrastructure/audio/mixer.rs`: emit 480-frame (10 ms) stereo blocks; wait for input until data available or block lags real time by about 150 ms, then fill absent source with zeros. M3.4 deleted this file. The aligner emits the blocks.
- [x] Implement source-specific gains (mic `1.0`, system `0.8` initially), stable summation and tested soft limiter. Capture gaps and overruns in diagnostics and sidecar metadata.
- [x] Handle clocks that drift apart (test ±100 ppm); gradual drift uses Rubato adaptive resample ratio on each capture stream (D-06); sudden holes/overlaps stay sparse silence/skip.
- [x] Add synthetic tests: different start times, silent output for 30 s, 5 ms gaps, overlapping/late packets, discontinuity, queue overrun, drift, clipping, missing source, and final tail drain.

### M3.3 — Live gain, meters and idle monitoring

- [x] `services/level_monitor.rs`: monitors selected devices while idle, stopping and fully releasing handles before a recording starts; resumes after save. Share underlying device-opening policy without parallel capture sessions fighting over endpoints.
- [x] Expose atomic peak snapshots for microphone, loopback and mixed output plus gain; GPUI polls near 30 Hz and renders in custom `ui/components/level_meter.rs` using GPUI Kit tokens.
- [x] Wire live mic gain slider, automatic gain toggle and system level display; ensure gain changes are applied to worker-owned processing state safely. Implement AGC only if enabled, initially off; target values per design document with smooth updates. **Superseded by M3.4:** the Auto checkbox was removed and `Agc` is gone. This box stays checked as the historical M3.3 completion. Microphone gain is the manual slider.
- [x] UI readouts use dBFS (−60 to 0), peak hold and clear text indication not color alone.

### M3.4 — Audio pipeline (echo removal, manual gain, session resampler)

Landed after M3.3. `Pipeline` and `Aligner` replace the all-in-one mixer. The M3 acceptance box above stays checked. The hardware gate for this package stays open.

- [x] `mixer.rs` is deleted. The production factory is `SonoraPipelineFactory` in `infrastructure/audio/sonora_steps.rs`. Production code imports `sonora` only in that file (`sonora = "=0.2.0"`). `sonora_spike.rs` is `#[cfg(test)]` measurement code, also imports `sonora`, and stays.
- [x] When system audio is on, the microphone chain is echo cancellation, then sonora microphone gain, then the microphone peak tap. When system audio is off, there is no canceller. The system chain is fixed gain 0.8, then the system peak tap. The mix chain is the soft limiter, then the mixed peak tap. The microphone is downmixed to mono before that chain and copied to both WAV channels.
- [x] Drift resampling is 48 kHz to 48 kHz on the session thread. The aligner exposes the error as a value. `CapturePort::open` has no drift argument. Capture workers have no Rubato. flexaudio converts the device rate.
- [x] Automatic gain is removed. An old settings file that still contains `agc_enabled` loads, and `mic_gain` applies. The settings schema stays 4.
- [x] Sidecar code `audio_processing` is written once at session end. The sidecar schema stays 3. When the canceller ran, session end logs `echo_return_loss_enhancement` and `delay_ms`. That enhancement is the capped internal statistic, not full-band ERLE.
- [x] `StatsTap` and `ChannelTap` exist and are not in the production factory. `LinearGain` remains on `PassThroughPipelineFactory` for the harness. It is not the production microphone step.
- [x] Tests cover these behaviors, without hardware: steps run in order and a tap sees the audio at its position; a failed or panicking step leaves the recording running; a short tail is not lengthened by padding; a mono microphone is copied to both channels; the echo reference is system audio before system gain; delayed system audio is reduced and near speech survives; speech with a silent reference is nearly unchanged; the slider level follows, including a change during the stream, and zero gain is silent; an old `agc_enabled` file loads and keeps `mic_gain`; `audio_processing` is noted once when a sonora constructor is skipped, and omitted when system audio is off; the aligner's drift error reaches the resampler; reopening resets that source's resampler only; a discontinuity survives an empty resampler output; a resampler panic passes that source through and the other source still resamples; the closed drift loop settles under a steady timeline error; timeline tests still cover a start offset, 30 s of silent system audio, gaps, overlaps, late blocks, the tail, overrun, and 100 ppm.
- [x] Sonora's BSD-3-Clause text is in `licenses/sonora-BSD-3-Clause.txt`, copied from the crates.io registry file `sonora-0.2.0/LICENSE`. M9 still copies that file into the ZIP.
- [ ] Hardware gate stays open. Not run: laptop speakers, wired headphones, Bluetooth headphones, a Bluetooth speaker, a selected output that is not the device actually playing, unplug and reopen during a session, and a one-hour session. No listening comparison, no laptop ERLE, and no ppm number. S9's 30-minute drift measurement was not run. The drift algorithm was not changed. If a later 30-minute run shows the loop cannot see real clock drift, that fix is a separate change. It is not part of this refactor.

**Validate:** 10-minute manual two-source recording with intermittent silence, headphone mic and shared sync cue; document sync offset/drift, meter overhead, no timeline contraction during system silence. The section 11 checks in `.specs/skrivli-audio-pipeline-plan.md` are the open hardware gate for M3.4 and were not run. Phase 8 `cargo test`: 220 passed, 0 failed, 3 ignored (spike S6, S8, and the 10-second S9 probe).

### M3 acceptance

- [x] Mic + system audio are audible and aligned in one playable WAV; deterministic mixer test matrix passes; meters remain responsive while idle and recording.

---

## M4 — Recording durability, device recovery and shutdown

**Goal:** Preserve maximum usable audio under device, filesystem and process failures.  
**Dependencies:** M3 full recording pipeline.

### M4.1 — Device and source-failure handling

- [x] Register `IMMNotificationClient`; callback only queues a device-changed event. Re-enumeration/restart occurs on workers, not inside COM callback.
- [x] Detect device invalidation/privacy/format failures; keep remaining source alive, insert timeline silence, attempt appropriate default endpoint reopen and record gaps/source changes.
- [x] Ensure single-source operation when mic or render endpoint is unavailable; report degraded mode in app status.

### M4.2 — Storage safeguards and recovery

- [x] Check destination writable and free space before start and about every 30 s; gracefully finalize below initially 200 MB, with a visible warning.
- [x] Periodically repair partial-WAV headers and implement startup recovery: inspect `.wav.partial`, validate sizes/PCM boundaries, repair if safe, rename to `.recovered.wav` without overwriting existing files.
- [x] On write or folder failure, preserve usable partial files and attempt bounded rescue in `%LOCALAPPDATA%\Skrivli\rescue\` only when original bytes can still be read; report unrecoverable failures accurately.
- [x] Strengthen atomic sidecar state writes, corrupt metadata handling and safe partial preservation; define cleanup policy, no automatic deletion of unknown partial files.

### M4.3 — Shutdown and fault injection

- [x] Explicit Exit while recording asks to stop/save; close-to-tray never stops capture. Windows session-ending notifications trigger bounded best-effort finalization.
- [x] Supervise workers and join on normal shutdown; surface worker panic, capture stall, disk error and session rollback failures through typed events.
- [x] Add tests for disk full/write error, forced worker termination, crash-interrupted header, output-folder removal, restart recovery, duplicate Stop and stale events.

**Validate:** kill process mid-recording and recover partial on next boot; unplug capture device and retain timeline; stop with low disk or unwritable folder without silently corrupting prior files.

### M4 acceptance

- [x] Every induced failure either yields a playable recording/recoverable partial or produces an explicit actionable error with preserved artifacts where possible.

---

## M5 — Recording library, metadata and playback

**Goal:** Browse and manage saved recordings from the app.  
**Dependencies:** M2 durable WAV and M4 recovery, recording IDs.

### M5.1 — Catalog and sidecars

- [x] `domain/recording.rs`: stable recording identity, format and metadata. `infrastructure/storage/sidecar.rs`: versioned `<basename>.json` containing start/end, duration, devices, gap/errors, transcription status and links.
- [x] `infrastructure/storage/library.rs`: scan configured directory for supported WAV/FLAC, join `.json`/`.txt`, sort newest first, preserve entries with malformed/missing sidecars, skip in-progress partials except recovery presentation.
- [x] `infrastructure/storage/watcher.rs`: Windows directory events, debounce and full refresh fallback. Preserve selection by stable ID across rescans and renames.
- [x] Test collisions, malformed metadata, external move/delete and accidental unpaired files.

### M5.2 — Recordings UI and player

- [x] Implement GPUI Kit list/table with Name, Date, Duration, Size and Transcript status; selectable rows, sorting, keyboard focus, responsive split/detail layout and empty/error states.
- [x] `infrastructure/files/reader.rs` and `player.rs`: background streaming decoder/player with Play/Stop and state events; changing selection stops playback. No file I/O during UI render.
- [x] Wire Open Folder, Refresh, Rename and Delete. Ensure rename groups audio/sidecar/transcript coherently and handles partial failures; delete via Recycle Bin after confirmation, not unconditional permanent deletion.
- [x] Keep app status and selection stable on external file events; do not block UI while scanning large directories.

**Validate:** dozens/hundreds of files, malformed sidecar, external edits, playback interruption, successful and partially failing rename/delete actions.

### M5 acceptance

- [x] Saved recordings appear with correct metadata; playback and file operations are coherent and do not orphan associated files silently.

---

## M6 — Offline transcription backend and model management

**Goal:** Complete a background offline transcription job into timestamped plain text.  
**Dependencies:** M5 library/sidecars and M0 native build infrastructure. **High-risk spike:** whisper.cpp binding and Windows native packaging should be tested before implementing the complete UI.

### M6.1 — Engine binding spike and models

- [x] Spike whisper.cpp on `x86_64-pc-windows-msvc`, CPU only, before any transcript UI. Compare a maintained Rust binding with a minimal FFI crate. Pin the crate and the whisper.cpp revision in `Cargo.lock` and record the choice, license, and notice files in `.specs/decisions.md` (D-07). The spike loads one model, runs a short inference, reports progress, and aborts cooperatively on a worker thread. The native build stays inside that crate's build script, or in `build.rs` flags that apply only to the native library. `views/`, `services/`, and `domain/` stay free of whisper types and of `cfg` for the engine.
- [x] Add `infrastructure/ai/whisper.rs` as the only module that holds engine pointers. The wrapper keeps model and context on the owning thread, turns progress and abort callbacks into plain values, and keeps engine stdout and stderr out of the log. Expose it through an inference port on `services/ports.rs` (load, transcribe a chunk, cancel, progress). `services/transcription.rs` depends on that port. `app/bootstrap.rs` builds the real adapter the same way it builds `PlatformHost`. Tests supply a fake. The GPUI foreground thread, including `Render::render`, never calls the engine.
- [x] Add `infrastructure/ai/models.rs` to find `ggml-*.bin` files under `AppPaths::models_dir()` (bases come from `platform::user_paths`) and under a path the user picked. Before a job starts, open the file and confirm the pinned engine accepts that variant. Missing, unreadable, and unsupported files are distinct `AppError` values. `user_message()` carries neither transcript text nor audio samples.
- [x] Check in a model manifest whose URL, SHA-256, byte size, variant, and license were verified against the downloaded bytes (D-08). A download starts only from an explicit user action: write a temp sibling, report progress, honor cancel, hash the file, and promote it with `infrastructure::platform::replace_file` only when the digest matches. A failed or cancelled download leaves no accepted model. Bootstrap, recording stop, and background retry do not start a download.
- [x] Once the user selects it, jobs default to the verified quantized multilingual base model. Offer tiny, base, and small only for rows in that manifest that the pinned engine accepts. Store the choice on `Settings` with a schema bump and migrate-or-fallback in `infrastructure/storage/settings.rs`. A `schema_version` newer than this build still falls back to defaults.

### M6.2 — Input pipeline and transcription text

- [x] Decode off the GPUI thread. Keep `infrastructure/files/reader.rs` as the streaming PCM16 WAV reader that `services/playback.rs` (`PlayerHost`) and `infrastructure/files/player.rs` already use. Add a separate transcription reader that pulls the finalized WAV in bounded chunks. When M8 lands FLAC, that reader also accepts FLAC through `flac-codec`. MP3 stays out of v1 until a decoder is pinned and tested. A refused file names the container or codec. The reader opens the finalized path, never the live `.wav.partial`.
- [x] Convert each decoded chunk to 16 kHz mono `f32` inside `infrastructure/ai/`, with an explicit downmix and an anti-alias resample. Leave `infrastructure/audio/resampler.rs` as the capture-stream Rubato drift nudge (D-06). Unit-test duration, the pitch and amplitude of a 1 kHz sine, and attenuation of energy above 8 kHz. Logs stay free of samples.
- [x] Put transcription window planning in a new `domain/transcript.rs`. Leave `domain/chunk_plan.rs` as the QPC-to-48 kHz session timeline. The planner cuts about 10-minute windows (30 minutes until D-18) with a fixed overlap, attaches a global timestamp offset to each window, and marks the overlap for later de-duplication. It accepts a streaming length and does its work without holding a two-hour recording in one buffer.
- [x] In `domain/transcript.rs`, add segment and time types and a deterministic UTF-8 formatter. Timestamps render as `[HH:MM:SS]`, so 3661 seconds is `[01:01:01]`. Paragraphs break after a pause of about 1.5 seconds or a span of about 60 seconds. The formatter trims whitespace and drops an immediately repeated identical line while keeping genuine repeated speech. `infrastructure/storage` writes `<basename>.txt` through the existing temp-sibling, `sync_all`, and `platform::replace_file` path, using `transcript_path_for` beside the audio. A failed text write leaves the previous `.txt` and the WAV bytes as they were.
- [x] Cover timestamps past one hour, overlap stitching, a genuine repeated line that must remain, an empty or silent result, and non-ASCII text. Assert the WAV bytes are unchanged after a successful format and after a failed text write. Colocate pure tests on `domain/transcript.rs` and put the file-write cases in `tests/transcription_flow.rs`.

### M6.3 — Service queue and persistence

- [x] Add `services/transcription.rs` with one FIFO queue and one inference worker. The host follows `RecordingHost` and `PlayerHost`: `spawn` from `app/bootstrap.rs`, `shutdown` from `app/shutdown.rs`, and `join` after `application().run` returns. States run `Queued → Preparing → Transcribing → Done`, `Failed`, or `Cancelled`. Cancel sets a flag the engine callback observes; the worker then releases the file handle and the model context. A second `Transcribe` for a `RecordingId` that is already queued or running is a no-op. Job ids use `domain/ids.rs` `JobId`.
- [x] `views/actions.rs` already accepts `AppCommand::Transcribe` and `CancelTranscription` and today stores `NOT_AVAILABLE`. Replace that arm with an effect the Recordings pane hands to the transcription host, on the same command path as the tray (`HostCommand` → `AppCommand`). Coalesce progress into `Presentation` the way meter snapshots are coalesced, so one engine callback does not become one GPUI notify. Persist significant transitions and the final status in the sidecar `transcription` body (`infrastructure/storage/sidecar.rs` already stores `status`, `progress_percent`, and `model`). Bump `SIDECAR_SCHEMA` when new fields are required, and keep a malformed sidecar as a library row. On the next launch, an in-progress sidecar status becomes interrupted and retryable. The worker does not resume that job on its own.
- [x] Auto-transcribe stays off unless `Settings` enables it, and the default is off. When it is on, enqueue only after `RecordingHost` has renamed the WAV and replaced the sidecar. The job opens that finalized path read-only. Inference leaves the recording bytes unchanged. A failed job sets `TranscriptStatus::Failed` (`domain/recording.rs`) and the WAV stays playable.
- [x] Drive `tests/transcription_flow.rs` with a fake inference port, a temporary directory, and no whisper.cpp or microphone. Cover success, engine failure, cancel, cancel racing the completion callback, an invalid model, a long input that stays chunked, and FIFO order for two queued recordings. Default `cargo test` does not download a model or open WASAPI.

**Validate:** English and a non-English sample run offline after the model file is already installed. A two-hour file stays within a bounded working set, keeps timestamp offsets across windows, and can be cancelled. A failed or cancelled job leaves the source WAV playable and the sidecar status explicit.

### M6 acceptance

- [x] With a local model already on disk, a queued job writes a timestamped `<basename>.txt` beside the finalized audio, updates the sidecar through `replace_file`, and surfaces progress and cancel on the existing `AppCommand` path. Success, failure, and cancel each leave a durable `TranscriptStatus` and an unchanged WAV.

---

## M7 — Transcript experience and end-to-end UI flow

**Goal:** Record → save → transcribe → read/copy entirely through app.  
**Dependencies:** M5 library UI and M6 transcription backend.

### M7.1 — Transcript and model interactions

- [x] Add `views/model_prompt.rs` as a GPUI Kit dialog opened from the Recordings detail pane. It is 560px wide and always lists the manifest variants, with the selected variant shown outside the list. Download installs that variant; Cancel and Retry apply to the download. The dialog cannot be dismissed until the phase is Ready, then the window closes it. The view does not call Win32, hash a file, or open a socket. Download, cancel, and retry are `AppCommand` effects the window hands to the model host off `render`, matching Start/Stop. `DesktopShell::pick_model_file` stays for a later local-file picker and is not called from this dialog. Missing, hash-mismatch, and unsupported files each get their own Kit message. Progress is a coalesced snapshot on `Presentation`. `app/bootstrap.rs` constructs the host.
- [x] On the selected row in `views/recordings.rs`, add Transcribe, Cancel, and Retry. Transcribe and Cancel dispatch the `AppCommand` variants already declared in `views/actions.rs`. Retry is a `Transcribe` of a row whose `TranscriptStatus` is `Failed`, with one `apply_command` arm so the effect stays explicit. The table and the detail pane show `TranscriptStatus::display`: `—`, `Queued`, `n%`, `Ready`, `Failed`. Failed detail uses `AppError::user_message`, not engine logs. Transcribe is disabled while that `RecordingId` is queued or running. A progress tick updates the selected row and the status strip in place, and leaves the `DataTable` model otherwise intact.
- [x] Show a finished transcript in a GPUI Kit read-only editor that can select, select-all, copy, and move by keyboard. Confirm the widget against gpui-kit 0.9.0 before writing the view. Load `<basename>.txt` on the background executor and store the string on the entity. A non-selectable `div` of the whole text is not the transcript view. If the pinned editor stalls on a long file, record the limit and the chosen widget in `.specs/decisions.md` before shipping a substitute. Verify Unicode and that scroll position survives a progress snapshot.
- [x] Feed progress and completion through the foreground drain `app/bridge.rs` already uses for `RecordSnapshot`, `LibrarySnapshot`, and `PlayerSnapshot`. `views/state.rs` reduces them into `Presentation`. Keep the library selection with `domain::recording::preserve_selection` across rescans. Completion sets `TranscriptStatus::Ready` and fills the detail text without changing the current tab. Playback still stops only when the selection changes, which `PlayerHost` already does.

### M7.2 — Completion integration

- [x] Present transcription completion through a Windows notification implemented in `infrastructure/platform/windows/` behind a new port on `services/ports.rs`. The tray worker already owns the message-only window; the notification uses that lifetime and does not subclass the GPUI HWND. The inference callback only records a structured event. The service emits it, and the platform adapter shows it when the user setting allows. Activating the notification posts `AppCommand::Show` and selects that `RecordingId` when `LibraryHost` still has the row.
- [x] Bind each job to its `RecordingId` and to the audio and `.txt` paths captured when the job leaves `Queued`. `LibraryHost::rename` and `LibraryHost::delete` notify the transcription host before they change files. A rename or delete of that id while the job is queued or running cancels the job, or fails it, before any text replace. `replace_file` for the transcript runs only when those paths still belong to that id. A rename that commits before the write may retarget the same id; otherwise the job fails and writes nothing. Delete still goes through the `Trash` port and includes a `.txt` the job has already promoted, plus the sidecar.
- [ ] Write the manual pass into `.specs/decisions.md`: record then transcribe from the detail pane; auto-transcribe left off, then turned on for one new recording; missing-model prompt satisfied by a verified local file; cancel; retry after `Failed`; restart with the `.txt` and sidecar `Ready` still shown; network disconnected after the model is installed. Each failure leaves the WAV playable in the Recordings tab.

### M7 acceptance

- [ ] From the Recordings tab a user can install a model, transcribe a finished recording, watch progress, cancel or retry, restart the app, and copy a long transcript from the Kit editor. The GPUI thread stays responsive, and the WAV and sidecar remain a coherent pair.

---

## M8 — Preferences, keyboard/Windows integration and optional formats

**Goal:** A configurable, accessible, Windows-native-feeling app.  
**Dependencies:** M1–M7 stable functional flow.

### M8.1 — Settings and live configuration

- [x] Settings foundation and transcription model: grouped layout (`ui/components/settings_rows.rs`), close-to-tray `Switch`, version footer, and a Transcription row showing the model in use with **Change model…** (opens the model dialog; **Choose model…** when none is installed). Evidence: `settings_model_row` and `version_text` unit tests; manual pass not run.
- [ ] Extend `views/settings.rs` with the remaining controls. The dialog is opened from the tab-bar gear. Add Kit controls for recordings directory (read-only text plus Browse), default microphone and system endpoints (`StoredEndpoint`), microphone gain (the manual slider), optional stems, WAV and FLAC retention, transcription model, language, CPU thread count, optional VAD, auto-transcribe, and start-hidden. Theme stays on the tab-bar `ThemePreference` cycle so the dialog cannot show a second, disagreeing value. Browse uses `DesktopShell`. Saves go through `ShellHooks::save_settings` into `app/settings_writer`, which already writes a temp sibling, `sync_all`, and `platform::replace_file`. `render` does not write the file.
- [ ] Validate in the `Settings` constructor: non-empty directory, gain inside the range the recording service already applies, and a thread count the machine can honor. Apply gain, theme, and close-to-tray on the running session. Hold directory, endpoints, model, language, threads, and VAD until the next recording or the next job. While `Presentation::inputs_locked()` is true, disable directory and device controls the same way the Record tab does. Hotkey and autostart apply immediately and report a failure inline in the dialog.
- [ ] Bump the `Settings` schema in `infrastructure/storage/settings.rs`. New fields deserialize with `#[serde(default)]` so an older `settings.json` still loads. A `schema_version` newer than this build still falls back to defaults. Tests cover a round-trip, a missing device id and a missing model id falling back to the default with a non-blocking notice, and a recording that keeps the endpoints captured in its `StartRequest` after a settings edit.

### M8.2 — OS commands and accessibility

- [ ] Add a global-hotkey port to `services/ports.rs` and implement it in `infrastructure/platform/windows/hotkeys.rs` with `RegisterHotKey` and `UnregisterHotKey`. Register on the tray message-only window (`Skrivli.TrayMessage`), which already pumps `GetMessageW`. A second message pump, and any subclass of the GPUI HWND, stay out of this work. Default binding is `Ctrl+Alt+R`, stored on `Settings`, with a disable flag. When the OS reports a conflict, keep the previous binding and surface `AppError::user_message`. The hotkey callback sends the same start/stop `HostCommand` the tray sends, which `views/actions.rs` already maps onto `AppCommand`. Unregister inside tray shutdown, before that window is destroyed.
- [ ] Add an autostart port and `infrastructure/platform/windows/autostart.rs`. The Settings checkbox reads the real OS registration (enabled, disabled, or failed), not only the JSON flag. Default is off. Pick one mechanism that works for a portable ZIP and record it in `.specs/decisions.md`: a per-user Run value, or a Startup-folder shortcut, aimed at the current executable path. Disabling removes that registration. First launch does not register. The service calls the port; registry and shell calls stay in `platform/windows/`.
- [ ] Walk the shell with the keyboard: Record tab (device selects, folder, start/stop, gain), Recordings tab (table, play/stop, rename, delete, transcribe, transcript editor), Settings, and the model prompt. Focus rings stay visible. Shortcuts that exist are named on the control or its tooltip. Confirm Light, Dark, and System, then 100%, 150%, and 200% scaling and a second monitor, using the M0.2 notes. Long paths ellipsize inside the 560×420 minimum. Privacy, device, disk, and model failures show `user_message()` in Kit inline text or a Kit dialog.
- [ ] Manual pass while a recording is active: restart Explorer with the hotkey registered; grant and deny autostart; take `Ctrl+Alt+R` from another app and confirm Skrivli keeps the previous binding; start from the tray and stop from the hotkey, then the reverse. Both paths hit one `RecordingHost`. A second start while `RecordPhase::Recording` is a no-op, and stop stays idempotent.

### M8.3 — Optional post-processing (feature-gated)

- [ ] After the M0–M7 gates are green, add `infrastructure/files/flac.rs` and confirm the `flac-codec` encode API (`FlacSampleWriter` or its byte writer) against the pinned crate (D-09). A background host, spawned and joined like `PlayerHost`, encodes the finalized WAV. Reopen and decode that FLAC and compare duration and sample bounds before any optional WAV removal. `scan_library` already pairs FLAC with its sidecar and `.txt`. The GPUI thread does not encode. A failed encode leaves the WAV in place and records a sidecar problem string.
- [ ] Add separate `*.mic.wav` and `*.system.wav` stems, and whisper VAD, only after the capture port and the inference port already used by recording and transcription can supply them. Both default off. A failed stem or VAD pass records a sidecar problem and leaves the mixed WAV as the library entry.
- [ ] When the FLAC spike fails encode, decode, or packaging, record the v1 exclusion in `.specs/decisions.md` and `.specs/skrivli-product.md` and skip stems and VAD in that same note. The release stays on WAV plus the offline transcript path.

### M8 acceptance

- [ ] Settings round-trip across a restart through `settings_writer`. The hotkey and the tray drive one `RecordingHost`. The main flows complete from the keyboard at 100% and 150% scaling. A failed FLAC or stem conversion leaves the original WAV playable.

---

## M9 — Verification, packaging and release

**Goal:** Reproducible portable Windows ZIP that runs on a clean target machine.  
**Dependencies:** All mandatory gates M0–M8.

### M9.1 — Automated and hardware validation

- [ ] On the pinned `x86_64-pc-windows-msvc` toolchain, run `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, and `cargo build --release`. `cargo test` stays on fakes (`recording_harness`, fake capture, clock, disk, trash, watch, and the transcription port) and does not open a microphone or whisper.cpp. Attach the command output to the release notes.
- [ ] Where a Windows 10 machine and a Windows 11 machine are available, record the matrix in `.specs/decisions.md`: default communications mic, USB mic, headset, no mic, no render device, microphone privacy denial, unplug or default-device change mid-take, loopback that stays silent, a 10-minute two-source take with a shared audible cue, a session past two hours, and a process kill with `.wav.partial` recovery on the next launch (`*.recovered.wav` when the header is safe). Single-source takes stay on the timeline and say so in the status strip.
- [ ] With a recording active, hide to tray, show again, and Exit through the save confirmation. Exercise `HostCommand::SessionEnding` as far as a logoff or shutdown test allows, then a second launch (`Local\Skrivli.SingleInstance`), an Explorer restart (`TaskbarCreated`), 100/150/200% DPI, a second monitor, keyboard-only use of Record and Recordings, transcription cancel and retry, and selection plus copy in a long transcript.
- [ ] Measure WAV duration against the session clock, sidecar overrun and drift counters, CPU and memory for capture and for one inference, time from process start to the first frame, inference real-time factor on the installed model, and the longest GPUI stall during progress updates. Write the machine, OS build, and numbers into `.specs/decisions.md`. Thresholds are the numbers this pass measured. The design doc does not supply pass limits to invent.

### M9.2 — Portable distribution

- [ ] `build.rs` already embeds the icon and `VERSIONINFO` only, because gpui-pre links its own manifest (D-01). Ship the release executable, the runtime DLLs the loader actually needs, GPUI Kit assets the binary expects, license and notice files for the app and for whisper.cpp, `licenses/sonora-BSD-3-Clause.txt`, `README`, and a privacy note that matches `.specs/skrivli-product.md`. Leave `resources/skrivli.manifest` in the repo as the DPI contract; it is not a second linked manifest.
- [ ] On a clean Windows VM, inspect the release binary with a loader-dependency tool. Every native dependency is either in the ZIP or a Windows component the supported OS already has. A machine without a Visual Studio install still starts the app. Record the dependency list next to the ZIP.
- [ ] Extract the ZIP to a path that contains a space and a non-ASCII character. Launch, confirm settings land in the roaming `AppPaths::settings_file()`, models under `AppPaths::models_dir()`, and logs under `AppPaths::logs_dir()`. Note SmartScreen and Defender behavior. Sign the binary when the distribution channel needs it, and record the choice in `.specs/decisions.md`.
- [ ] The ZIP contains no `ggml-*.bin`. First launch does not download one. The model prompt states the file size and that the download needs a network. After that file is installed, transcription of an existing recording completes with the network disconnected.
- [ ] Privacy check against the product spec: no account, no telemetry, and no upload of audio or transcript text. A log sample from a real recording-plus-transcription session contains neither samples nor transcript lines. The user can open the recordings folder, delete a recording through the Recycle Bin (`Trash`), and remove a model file from the models directory.

### M9 acceptance

- [ ] On a clean Windows machine a new user unzips the package, launches Skrivli, records microphone and system audio, stops, finds and plays the file, installs a model only after choosing to, transcribes offline, and copies the text. The release attaches this test report and a ZIP built from the pinned toolchain and `Cargo.lock`.

---

## Dependency map and review checkpoints

```text
M0 Foundation
  ↓
M1 Shell & tray
  ↓
M2 Mic → WAV (first end-to-end usable slice)
  ↓
M3 Loopback + mixer + meters (core recording feature complete)
  ↓
M4 Robust recording and recovery
  ↓
M5 Recordings library and playback
  ↓
M6 Offline transcription worker
  ↓
M7 Transcript UI and complete workflow
  ↓
M8 Settings / Windows polish / optional formats
  ↓
M9 QA / portable release
```

Some low-risk tasks may run in parallel **after their interfaces are fixed**: the M5 list UI can use fake library data while the storage scanner is implemented; M6 model-binding spike can start after M0 toolchain setup; M8 keyboard audits can begin on M1 views. Avoid parallel changes to recording lifecycle ownership, COM worker contracts, or the audio timeline before those interfaces stabilize.

### Recommended checkpoints

| Checkpoint | After | Demonstration | Blocking issue if missing |
| --- | --- | --- | --- |
| C1 — Window viability | M0 | GPUI Kit window, themed components, keyboard focus on Windows | Do not build UI on unverified backend |
| C2 — Native lifecycle | M1 | Tray and instance handoff survive Explorer restart | Resolve HWND/message-pump strategy |
| C3 — First audio | M2 | Playable finalized microphone WAV | Resolve COM/capture/format/write problems |
| C4 — Core feature | M3 | 10-minute two-source WAV with continuous timeline | Resolve silence and clock drift |
| C5 — Data safety | M4 | Crash and device-loss recovery walkthrough | Do not ship unsafe recording |
| C6 — Local media | M5 | Browse, play, rename/delete existing captures | Resolve identity/sidecar consistency |
| C7 — Model engine | M6 | Local CPU transcription with cancel | Resolve FFI/model manifest before UI polish |
| C8 — User journey | M7 | Capture-to-copy with no network once model installed | Resolve editor/text selection and state bridging |
| C9 — Distribution | M9 | Clean VM passes happy path and adverse test subset | Release blocked |

## Suggested issue/PR template

For each work package above, create an issue with:

```markdown
Title: Mx.y — Short description
Depends on: Mx.y / contract
Owner: TBD
Modules: src/...
Implementation:
- [ ] Specific code changes
- [ ] Unit/integration tests
- [ ] Windows manual check if relevant
Evidence: test command output, manual test notes/screenshots/log counters
Done when: observable milestone acceptance condition
```

## Unresolved decisions register (must be resolved, not assumed)

| ID | Needed by | Decision | Validation method |
| --- | --- | --- | --- |
| D-01 | M0 | GPUI Kit revision and compatible GPUI Windows lifecycle APIs | Executable window/focus/event/close/text spike |
| D-02 | M1 | Tray hidden HWND/message-pump bridge | Explorer restart + shutdown test |
| D-03 | M2 | flexaudio CapturePort adapters + device ids | Real mic/loopback opening spikes |
| D-04 | M2–M3 | Bounded SPSC implementation, overflow/drop/gap semantics | Concurrent stress + synthetic timeline tests |
| D-05 | M2 | RIFF size cap/rotation vs RF64 | Header boundary tests + long-session design |
| D-06 | M3 | Adaptive Rubato ratio for gradual drift; sparse hole/overlap only | ±100 ppm error publication + resampler ratio tests |
| D-07 | M6 | whisper.cpp binding/version, native runtime and licenses | Clean MSVC CPU build + transcription demo |
| D-08 | M6 | Model catalog, exact trusted URLs/digests and compatibility | Verified manifest and corrupted-download test |
| D-09 | M8 | `flac-codec` FLAC encode/decode viability and optional feature priority | Encode/decode/packaging proof |

## Scope discipline

**Mandatory v1:** M0–M7 core goals, M8 essential settings/hotkey/accessibility, M9 distribution.  
**Optional/defer if needed:** FLAC, separate recording stems, VAD, expanded theming and installers.  
**Outside v1:** live transcription, cloud sync, diarization, summaries, GPU transcription and cross-platform support.

This plan is derived from the supplied Skrivli app design. It intentionally converts its decisions into implementation tasks and acceptance evidence without claiming untested Rust/GPUI Kit/Windows APIs already work.
