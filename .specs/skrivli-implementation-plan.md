# Skrivli — Milestone-Based Implementation Plan

**Target:** Windows 10/11, x86_64-pc-windows-msvc  
**Stack:** Rust stable, GPUI Kit/GPUI, flexaudio (WASAPI/cpal), local whisper.cpp  
**Source of truth:** `app-design.md` for product and technical design; `AGENTS.md` for source placement, layering and coding conventions.  
**Status:** Execution backlog; tasks unchecked until implemented and verified.

## How to execute this plan

- Implement milestones **M0 → M9** in order. Within each milestone, complete numbered work packages in order unless a dependency is noted. Each package should be an independently reviewable PR or small sequence of commits.
- **Definition of done for every package:** code builds on pinned Windows toolchain, automated tests for testable logic, no warnings from `cargo clippy --all-targets -- -D warnings`, formatted source, typed errors at external boundaries, explicit resource ownership and shutdown, and updated documentation if the architecture changed.
- **Release gates are empirical:** a passing unit test is not evidence that GPUI Kit, WASAPI, Windows Shell, Media Foundation, or whisper.cpp works on Windows hardware. Run manual checks at the designated gates.
- Use one Cargo package (`src/main.rs` and `src/lib.rs`) initially. Do not introduce a workspace, database, custom UI library, custom lock-free queue or general dependency-injection framework without demonstrated need.
- Prefer GPUI Kit's version-matched GPUI and standard components. Verify actual APIs against the **pinned** source before writing integrations. No API signatures or library capabilities described here are presumed verified.
- Maintain `.specs/decisions.md` (or small ADR files) for unresolved choices, outcome, tested toolchain/device and rationale. Record the exact commit/version of GPUI Kit and whisper bindings when confirmed.

### Cross-cutting contracts to establish early

1. **Layering:** `domain` has no platform or I/O dependencies; `services` own use-case state machines; `infrastructure` owns COM, threads, disk and FFI; `app` wires commands/events; `views` and `ui` contain presentation only.
2. **Thread ownership:** GPUI foreground must never perform capture, decoding, disk scanning, model inference or blocking joins. Every worker has an owner, cancellation/shutdown signal and join strategy; COM objects stay in the correct apartment.
3. **Commands/events:** a single command path drives toolbar, tray and hotkey. Use discrete structured events, coalesce high-rate progress and meter snapshots and marshal all UI state changes to the GPUI executor.
4. **Persistence:** settings and JSON sidecars are schema-versioned and written using a temporary sibling file, flush/sync as appropriate, then replace/rename; handle Windows replacement semantics and recovery errors explicitly.
5. **Recording clock:** all audio events map to a monotonic QPC-based session timeline; absence of packets means source silence/gap, **not** elapsed-time suspension.
6. **Safety:** do not log audio or transcript contents. Preserve the primary WAV on any transcription or M4A failure. Never delete usable partials without explicit recovery/cleanup logic.

### Proposed application-facing types (design sketches)

These are contracts to refine, not guaranteed compiling code:

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
- [ ] Test 100%/150% display scaling and multiple displays. Record issues/workarounds in `.specs/`.

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

- [x] `infrastructure/windows/tray.rs`: native notification icon, stable GUID where appropriate, Show/Hide, Start/Stop (disabled until M2), Open Folder, Settings and Exit.
- [x] Integrate a supported message window/pump with thread-safe action handoff; never assume GPUI HWND subclassing is supported.
- [x] Implement configured close-to-tray/minimize behavior and explicit Exit, including first-use explanation.
- [x] Re-add tray icon on Explorer `TaskbarCreated`, remove icon on normal Exit, keep one authoritative lifecycle controller.
- [x] Complete second-instance activation (signal first process to focus/show; second exits without touching recordings).

**Validate:** close/minimize/show/hide repeated, tray action dispatch, taskbar restart, second-instance race, clean Exit without orphaned background threads.

### M1 acceptance

- [ ] Tab/navigation/window state works via GPUI Kit; Explorer restart restores tray icon; only one process controls app state.

---

## M2 — Microphone capture and first persistent WAV

**Goal:** Reliable microphone-only start/stop from UI, resulting in playable crash-conscious WAV.  
**Dependencies:** M1 command bridge and shell. **Primary vertical slice:** Record button → service → native capture → file → UI saved event.

### M2.1 — Audio domain, device discovery and folder choice

- [x] `domain/audio.rs`: `AudioFormat`, frame counts, monotonic session time and sample index conversion with checked integer arithmetic.
- [x] `infrastructure/audio/devices.rs`: enumerate `eCapture`/`eRender` endpoints via MMDevice; expose stable IDs and friendly display names; select Windows default communications microphone and default render endpoint.
- [x] Design `DeviceSelection` with explicit/default modes and missing-device fallback notification; persist stable endpoint choices.
- [x] `infrastructure/windows/dialogs.rs`: native folder picker; validate write permissions and folder creation. Wire GPUI Kit device selectors and destination read-only field in `views/recording.rs`.
- [x] Test absent/disabled devices and microphone privacy denied. Do not allow endpoint/path modifications during active recording.

### M2.2 — Mic capture worker and normalization

- [x] `infrastructure/audio/flex_capture.rs`: flexaudio `SourceKind::Mic` / `SystemLoopback` behind `CapturePort`; worker owns the stream, maps `frame_index` onto the session timeline, and applies Rubato drift nudge.
- [x] Device catalog from `flexaudio::devices()`; COM `IMMNotificationClient` only wakes re-enumeration.
- [x] Bound memory and packet queues; no blocking disk writes, UI access, unbounded allocations or high-contention locks in hot loop.
- [x] Expose testable interfaces for synthetic packet input and a fake capture backend.

### M2.3 — WAV writer and recording service

- [x] `infrastructure/files/wav.rs`: streaming PCM16 stereo 48 kHz RIFF writer to collision-safe `Skrivli_YYYY-MM-DD_HH-MM-SS.wav.partial`; quantize/clamp `f32`, write/repair RIFF/data sizes periodically (~2 s), flush/finalize then rename.
- [x] Choose and document a RIFF boundary policy now: **safe stop or rotate** before 32-bit size overflow; do not write invalid headers. Include tests for boundary math without generating multi-GB files.
- [x] `services/recording.rs`: `Idle → Starting → Recording → Stopping → Idle/Error`, start readiness handshake, owned workers, bounded channel and orderly shutdown/drain/flush. Stop must be idempotent.
- [x] Ensure UI shows state/timer/errors and delegates to service, never directly opens capture. Persist basic sidecar info now if this makes recording identity stable; expand in M5.
- [x] Add `tests/recording_flow.rs` using fake capture, injectable clock and temporary recording directory.

**Validate:** one-minute mic WAV opens in a common Windows player; duration approximately matches wall clock, data/header lengths correct; repeated start/stop and startup failure clean up threads/files.

### M2 acceptance

- [ ] On real Windows hardware a user can choose microphone and folder, record and stop; a finalized WAV survives app restart and plays back.

---

## M3 — System loopback, synchronization, mixing and meters

**Goal:** Concurrent microphone/system recording with a continuous, synchronized timeline.  
**Dependencies:** M2 mic worker, WAV writer and service lifecycle.

### M3.1 — Loopback worker and per-source transport

- [x] `FlexSystemCapture`: flexaudio `SourceKind::SystemLoopback` with classic silent-render keepalive; separate worker from the mic stream.
- [x] Idle silence is delivered as keepalive packets; remaining gaps still filled by the mixer from the session clock.
- [x] Select a proven bounded SPSC queue for each source. Define timestamped packet/block representation, maximum queue latency and overflow/drop/gap accounting. Do not invent a custom lock-free queue unless necessary.

### M3.2 — Deterministic mixer and drift correction

- [x] `domain/chunk_plan.rs`: pure mapping from QPC ticks to 48 kHz sample indices, with integer/rational conversion that avoids cumulative floating-point drift. Define session origin `t0` and treatment of latency, missing packets, overlaps and discontinuities.
- [x] `infrastructure/audio/mixer.rs`: emit 480-frame (10 ms) stereo blocks; wait for input until data available or block lags real time by about 150 ms, then fill absent source with zeros.
- [x] Implement source-specific gains (mic `1.0`, system `0.8` initially), stable summation and tested soft limiter. Capture gaps and overruns in diagnostics and sidecar metadata.
- [x] Handle clocks that drift apart (test ±100 ppm); gradual drift uses Rubato adaptive resample ratio on each capture stream (D-06); sudden holes/overlaps stay sparse silence/skip.
- [x] Add synthetic tests: different start times, silent output for 30 s, 5 ms gaps, overlapping/late packets, discontinuity, queue overrun, drift, clipping, missing source, and final tail drain.

### M3.3 — Live gain, meters and idle monitoring

- [x] `services/level_monitor.rs`: monitors selected devices while idle, stopping and fully releasing handles before a recording starts; resumes after save. Share underlying device-opening policy without parallel capture sessions fighting over endpoints.
- [x] Expose atomic peak snapshots for microphone, loopback and mixed output plus gain; GPUI polls near 30 Hz and renders in custom `ui/components/level_meter.rs` using GPUI Kit tokens.
- [x] Wire live mic gain slider, automatic gain toggle and system level display; ensure gain changes are applied to worker-owned processing state safely. Implement AGC only if enabled, initially off; target values per design document with smooth updates.
- [x] UI readouts use dBFS (−60 to 0), peak hold and clear text indication not color alone.

**Validate:** 10-minute manual two-source recording with intermittent silence, headphone mic and shared sync cue; document sync offset/drift, meter overhead, no timeline contraction during system silence.

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

- [ ] `domain/recording.rs`: stable recording identity, format and metadata. `infrastructure/storage/sidecar.rs`: versioned `<basename>.json` containing start/end, duration, devices, gap/errors, transcription status and links.
- [ ] `infrastructure/storage/library.rs`: scan configured directory for supported WAV/M4A, join `.json`/`.txt`, sort newest first, preserve entries with malformed/missing sidecars, skip in-progress partials except recovery presentation.
- [ ] `infrastructure/storage/watcher.rs`: Windows directory events, debounce and full refresh fallback. Preserve selection by stable ID across rescans and renames.
- [ ] Test collisions, malformed metadata, external move/delete and accidental unpaired files.

### M5.2 — Recordings UI and player

- [ ] Implement GPUI Kit list/table with Name, Date, Duration, Size and Transcript status; selectable rows, sorting, keyboard focus, responsive split/detail layout and empty/error states.
- [ ] `infrastructure/files/reader.rs` and `player.rs`: background streaming decoder/player with Play/Stop and state events; changing selection stops playback. No file I/O during UI render.
- [ ] Wire Open Folder, Refresh, Rename and Delete. Ensure rename groups audio/sidecar/transcript coherently and handles partial failures; delete via Recycle Bin after confirmation, not unconditional permanent deletion.
- [ ] Keep app status and selection stable on external file events; do not block UI while scanning large directories.

**Validate:** dozens/hundreds of files, malformed sidecar, external edits, playback interruption, successful and partially failing rename/delete actions.

### M5 acceptance

- [ ] Saved recordings appear with correct metadata; playback and file operations are coherent and do not orphan associated files silently.

---

## M6 — Offline transcription backend and model management

**Goal:** Complete a background offline transcription job into timestamped plain text.  
**Dependencies:** M5 library/sidecars and M0 native build infrastructure. **High-risk spike:** whisper.cpp binding and Windows native packaging should be tested before implementing the complete UI.

### M6.1 — Engine binding spike and models

- [ ] Evaluate maintained Rust whisper.cpp bindings versus minimal FFI; select, pin exact versions and build on x86_64 MSVC CPU-only. Verify ownership, callbacks, cancellation and required notices/licenses.
- [ ] `infrastructure/ai/whisper.rs`: thin safe wrapper around model/context, progress callback and abort flag; no engine pointers cross invalid lifetime/apartment boundaries; avoid stdout noise.
- [ ] `infrastructure/ai/models.rs`: find installed `ggml-*.bin` in Local AppData/models or user-selected path; validate file readability and model compatibility before job.
- [ ] Create a trusted manifest with **verified** URLs, SHA-256 digests, size/variant and licenses; download only on explicit user action to a temp file, expose progress/cancel, verify digest, then promote atomically. Never guess hashes or download a model in the background.
- [ ] Default to compatible quantized multilingual base model after user selection; allow available tiny/base/small choices only when verified.

### M6.2 — Input pipeline and transcription text

- [ ] `infrastructure/files/reader.rs`: validate supported decoder coverage for WAV/FLAC/MP3 and M4A (via suitable decoder/Media Foundation); errors identify unsupported codecs.
- [ ] Normalize streamed audio to 16 kHz mono `f32`, with channel mapping and anti-alias filtering. Unit-test duration, sine pitch/amplitude, high-frequency alias rejection.
- [ ] `domain/chunk_plan.rs`: split long recordings into approximately 30-minute windows with overlap; apply global timestamp offsets and de-duplicate overlaps, without loading a two-hour recording into memory.
- [ ] `domain/transcript.rs`: segment/time types and deterministic UTF-8 formatter using `[HH:MM:SS]`, paragraph breaks (~1.5 s pause or ~60 s span), conservative repeated-line cleanup. Atomically write `<basename>.txt`.
- [ ] Test >1-hour timestamp formatting, segment stitching, repeated genuine speech, empty/silent transcript, and Unicode content.

### M6.3 — Service queue and persistence

- [ ] `services/transcription.rs`: FIFO queue with one inference worker, `Queued → Preparing → Transcribing → Done/Failed/Cancelled`; cancellation via flag + cooperative callback.
- [ ] Guard duplicate jobs for same recording, track job IDs, throttle progress to UI, persist significant transitions and final status to sidecar. Restart policy: recover previous in-progress job as interrupted/retryable; no implicit auto-resume unless explicitly designed.
- [ ] Auto-transcribe **off** by default; when enabled, enqueue only after WAV/sidecar finalization. Never modify recording audio during inference.
- [ ] Test fake engine success/failure, cancellation, cancellation race at completion, invalid model, large input and queue ordering in `tests/transcription_flow.rs`.

**Validate:** English and non-English samples offline; 2-hour audio has bounded memory, correct time offsets and cancellable job; transcription failures preserve source WAV.

### M6 acceptance

- [ ] A queued local model transcribes finalized audio to correctly timestamped `.txt` with durable success/failure state, progress and cancellation.

---

## M7 — Transcript experience and end-to-end UI flow

**Goal:** Record → save → transcribe → read/copy entirely through app.  
**Dependencies:** M5 library UI and M6 transcription backend.

### M7.1 — Transcript and model interactions

- [ ] `views/model_prompt.rs`: GPUI Kit installed-model picker, Browse, explicit Download, integrity check feedback, progress, Cancel and Retry. Handle corrupt/missing/unsupported model distinctly.
- [ ] Add Transcribe/Cancel/Retry actions to selected recording; show statuses `—`, `Queued`, `n%`, `Ready`, `Failed` and consistent detail-pane messages.
- [ ] Use a GPUI Kit **read-only selectable editor/text component** for large plain-text transcripts. Validate select-all, clipboard, keyboard navigation, scroll position and Unicode; no simple nonselectable `div` substitution.
- [ ] Wire progress and completion events to state snapshots, preserving selection and avoiding frequent full-table rebuilds.

### M7.2 — Completion integration

- [ ] Add Windows completion notification/tray indication where supported and enabled; opening notification selects the correct recording when possible.
- [ ] Define behavior when recording is renamed/deleted during queued transcription: cancel or pin identity and fail gracefully; never write transcript to stale/incorrect destination.
- [ ] End-to-end integration scenarios: record and manually transcribe, auto-transcribe path, missing-model prompt, cancel/retry, app restart with transcript present, network disconnected after model installed.

### M7 acceptance

- [ ] A user can run the full offline workflow and copy a long transcript without UI freezes or losing recording data.

---

## M8 — Preferences, keyboard/Windows integration and optional formats

**Goal:** A configurable, accessible, Windows-native-feeling app.  
**Dependencies:** M1–M7 stable functional flow.

### M8.1 — Settings and live configuration

- [ ] Build GPUI Kit settings UI for directory, default endpoints, mic gain/AGC, optional stems, WAV/M4A retention, model, language, CPU threads, optional VAD, auto-transcribe, close-to-tray and start-hidden.
- [ ] Validate input, atomically persist, distinguish immediate changes from those deferred until next recording/job; disable hazardous controls during active recording.
- [ ] Add settings migration tests and missing device/model fallback behavior; maintain schema compatibility within v1.

### M8.2 — OS commands and accessibility

- [ ] `infrastructure/windows/hotkeys.rs`: configurable RegisterHotKey toggle, default `Ctrl+Alt+R`, conflict detection, unregister/re-register and shared app command dispatch.
- [ ] `infrastructure/windows/autostart.rs`: opt-in startup registration with clear enable/disable status; choose/document a mechanism suitable for portable ZIP distribution. No surprise autostart.
- [ ] Improve keyboard focus, shortcut discoverability, dark/light theme, high DPI, multi-monitor bounds, error feedback and long filename/path rendering. Verify keyboard-only operation.
- [ ] Test Explorer restart, autostart permissions, hotkey conflicts and tray/foreground synchronization under active recording.

### M8.3 — Optional post-processing (feature-gated)

- [ ] Only if core M0–M7 gates are green: implement `infrastructure/files/m4a.rs` with Media Foundation sink writer in background worker. Verify output by reopening/decoding before any optional WAV removal.
- [ ] Optional separate source stems and VAD only after validated engine/device integration; default disabled. Treat failed post-processing as nonfatal and preserve WAV.
- [ ] If native interop or packaging is unstable, **defer M4A/stems/VAD** and explicitly document the v1 exclusion rather than blocking essential release.

### M8 acceptance

- [ ] Settings survive restart, hotkey and tray share recording state, major app functions are keyboard accessible, and optional conversions never risk original WAV.

---

## M9 — Verification, packaging and release

**Goal:** Reproducible portable Windows ZIP that runs on a clean target machine.  
**Dependencies:** All mandatory gates M0–M8.

### M9.1 — Automated and hardware validation

- [ ] Run clean `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, `cargo build --release` on supported MSVC toolchain.
- [ ] Run manual matrix on Windows 10 and 11 where available: default mic, USB mic, headset, no mic, no render device, privacy denial, device swap/default change, loopback silence, 10-minute two-source capture, >2-hour endurance and crash recovery.
- [ ] Exercise active recording close-to-tray, explicit Exit, logoff/shutdown best effort, second launch, Explorer restart, DPI 100/150/200%, keyboard-only use, transcription cancel/retry and long transcript selection.
- [ ] Measure audio duration/sync, queue overruns/drift, CPU/memory, startup latency, inference real-time factor and worst-case UI stalls. Record test hardware and measured results; do not invent pass thresholds not verified in design.

### M9.2 — Portable distribution

- [ ] Embed icon, manifest and version metadata; package release executable plus only required runtime DLLs/assets, license/notice files, README and privacy notes.
- [ ] Check runtime loader dependencies on clean VM; verify no developer-installed MSVC/native library dependency is silently assumed.
- [ ] Verify ZIP extraction path with spaces/Unicode, writeable user directories, settings and model location, Windows Defender/SmartScreen behavior; code signing is recommended if distribution warrants it.
- [ ] Confirm no model is bundled/downloaded by default; ensure explicit model installation and documented disk/network impact.
- [ ] Final privacy audit: no account, telemetry or transcript/audio uploads; logs exclude content, and user can find/delete recordings and models.

### M9 acceptance

- [ ] From a clean Windows machine, a new user can unzip, launch, record mic + system, stop, find/play recording, explicitly install a model, transcribe offline and copy text. Test report and reproducible ZIP are attached to release.

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
| D-09 | M8 | Media Foundation M4A viability and optional feature priority | Encode/decode/packaging proof |

## Scope discipline

**Mandatory v1:** M0–M7 core goals, M8 essential settings/hotkey/accessibility, M9 distribution.  
**Optional/defer if needed:** M4A, separate recording stems, VAD, advanced AGC, expanded theming and installers.  
**Outside v1:** live transcription, cloud sync, diarization, summaries, GPU transcription and cross-platform support.

This plan is derived from the supplied Skrivli app design. It intentionally converts its decisions into implementation tasks and acceptance evidence without claiming untested Rust/GPUI Kit/Windows APIs already work.
