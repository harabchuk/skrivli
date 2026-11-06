# Skrivli decisions

## D-01 — GPUI Kit revision and Windows window APIs

**Outcome:** pin `gpui-kit` **0.9.0** (crates.io, checksum in `Cargo.lock`). It re-exports GPUI from the package **`gpui-pre` 0.3.11** (library name `gpui`). Do not depend on `gpui` or `gpui-component` separately. Optional `gpui-fast` is not enabled.

**Toolchain observed:** rustc/cargo 1.99.1 via rustup, host `x86_64-pc-windows-msvc`, MSVC 14.51 (Visual Studio 18 Build Tools). Debug `cargo run` opened a window titled Skrivli with the Foundation status line, a Kit **Ready** button, and a Kit select showing Foundation. Tab moved the focus ring onto that select. The window was then closed. 100% / 150% DPI and multi-monitor placement were **not** executed.

**Bootstrap (from the v0.9.0 hello-world sample, not from memory):**

1. `gpui_kit::application().with_assets(gpui_kit::assets::Assets)`
2. `.run(|cx| { ... })`
3. `gpui_kit::init(cx)` before any styled widget
4. Theme preference: `ThemePreference` in settings (`system` / `light` / `dark`). Kit `ThemeMode` is only Light/Dark; System maps through `Theme::sync_system_appearance`. Explicit modes use `Theme::change`. Applied at startup from `ui::theme::apply_theme` and again when the tab-bar control cycles or (for System) when `observe_window_appearance` fires.
5. `gpui_kit::open_window(WindowOptions, cx, |window, cx| ...)` which mounts `gpui_kit::base::Root`
6. `window.set_window_title("Skrivli")` inside the builder

**Known Windows limitation:** `TitleBar::on_close_window` is Linux-only in gpui-component 0.9.0 and does nothing on Windows. Close interception on Windows is `window.on_window_should_close`. No extra adapter was required for creating a window, focusing it, or using a Kit button and `Select`.

**Manifest collision:** `gpui-pre` 0.3.11 embeds `resources/windows/gpui.manifest.xml` as `RT_MANIFEST` 1, already containing `dpiAwareness` `PerMonitorV2` and `dpiAware` `true/pm`. `build.rs` cannot embed `resources/skrivli.manifest` as well: `link.exe` fails with `CVT1100` duplicate manifest. Cargo features are additive, so the app cannot turn off `gpui-pre`'s default `windows-manifest` feature through `gpui-kit`. `build.rs` embeds the icon and `VERSIONINFO` only. `resources/skrivli.manifest` is the checked-in DPI contract and matches that PerMonitorV2 setting. It is not linked.

### APIs to use in M1 (checked against gpui-kit 0.9.0 docs and gpui-pre 0.3.11 docs; not exercised on hardware)

| Concern | API |
| --- | --- |
| Foreground scheduling | `cx.spawn` (weak entity + `AsyncApp`), `cx.spawn_in` / `window.spawn` when the task needs the window, `cx.background_spawn` for CPU work. Meter polls should be one spawned task awaiting `cx.background_executor().timer(Duration)` (`BackgroundExecutor::timer` on gpui-pre 0.3.11), then `this.update` on the foreground. Do not notify once per audio frame. |
| Render lifecycle | `Render::render` builds elements from entity state. `cx.notify()` marks that entity. `window.refresh()` dirties the whole window. `window.on_next_frame` runs before the next draw and does not itself mark the window dirty. Do not call `notify` / `refresh` / `request_animation_frame` unconditionally from `render`. |
| Close interception | `window.on_window_should_close` while the window still exists; return `false` to cancel a platform close. `cx.on_window_closed` runs after the window is gone (`&mut App`, `WindowId`). `window.remove_window()` is an explicit removal and does not go through the should-close callback. M0 quits when `cx.windows()` is empty. Close-to-tray is M1; the Linux-only title-bar hook is not the Windows path. |
| Selection / editor | Kit `Select` (`SelectState` + `SearchableVec` + `IndexPath`) is the dropdown. It is keyboard-operable (Tab, Enter, arrows, Escape, Space) per the 0.9.0 select doc. Text editing stays on Kit inputs; M0 does not open an editor. Window-scoped selection is owned by `Root`. |
| Overlay / dialog position | `gpui_kit::open_window` installs one `Root` per window. Dialogs use `window.viewport_size()` and stay inside the content. `window.fully_visible_bounds()` is the conservative window-local rect. `window.scale_factor()` is only for device pixels at a platform boundary, not for GPUI layout sizes. Positioning behavior at 100%/150% and across monitors is **unverified**. |
| Timers | `BackgroundExecutor::timer`. Frame work uses `window.on_next_frame`. |
| Window sizing | `WindowOptions.window_bounds` via `WindowBounds::centered`, `window_min_size`, `focus: true`. M0 requests 720×480 centered and a 560×420 minimum. Restored bounds and monitor clamping are M1. Actual pixel size on this display was **not measured**. |

Keyboard focus is wired (`focus_handle().tab_stop(true)`, `track_focus`, `window.focus`, Kit button and select). On this machine Tab moved the visible focus ring onto the select. DPI scaling and a second monitor were not part of that check.

## D-02 — Tray pump, close interception, and single-instance handoff

**Outcome:** M1 uses three separate Windows objects. The GPUI window stays on the GPUI thread. A dedicated worker thread owns a message-only window of class `Skrivli.TrayMessage` and pumps `GetMessageW` there. That procedure is not attached to the GPUI HWND (`Zed::Window`). Tray commands are `TrayCommand` values sent on an `std::sync::mpsc` channel. A foreground task started with `Context::spawn_in` awaits `BackgroundExecutor::timer` (150 ms) and drains the channel on the GPUI thread, coalescing `ShellPulse` events into one presentation update. The same path serves reserved GPUI actions (`cx.on_action` → `dispatch` → `MainWindow::handle`). The in-window `AppMenuBar` (File/Help) was removed; Settings opens from a tab-bar gear (`TabBar::suffix` + `IconName::Settings`), with About reachable from the Settings dialog.

**Notify icon:** `Shell_NotifyIconW` with GUID `{7C3E8A21-4B6D-4E91-9F2A-1D5C8B0E6A44}`, tooltip `Skrivli`, and `NOTIFYICON_VERSION_4`. `NIM_DELETE` runs on `WM_CLOSE` before the thread exits. `RegisterWindowMessageW("TaskbarCreated")` calls `NIM_ADD` again. The icon is the system application icon, not a recording-state icon.

**Show and hide:** `Platform::hide` on gpui-pre 0.3.11 Windows is a no-op, and `Window` has no hide method. The shell finds `Zed::Window` titled `Skrivli` and calls `ShowWindow` (`SW_SHOW` plus `Window::activate_window`, or `SW_HIDE`). It does not subclass that HWND. `window.on_window_should_close` returns false when `close_to_tray` is set (default true) and hides instead; the first hide opens a Kit alert and stores `tray_explanation_seen`. Explicit Exit calls `window.remove_window`, which does not go through that callback, then `cx.quit`. Minimize is not a GPUI callback: the same foreground timer sees `IsIconic` and hides when close-to-tray is on. `WindowOptions.window_min_size` is 560×420, and `domain::window_frame` clamps saved bounds against injected work-area rectangles. Restored bounds use `PlatformDisplay::visible_bounds`. Saves are debounced about 400 ms and written by a background settings thread, not from `render`.

**Single instance:** mutex `Local\Skrivli.SingleInstance`. The winner creates auto-reset event `Local\Skrivli.Activate` on a listener thread before `application().run`. The loser opens that event, `SetEvent`, and exits 0 without a window and without reading or writing recordings. The listener posts `AppCommand::Show` on the same channel. If the GPUI window is not up yet, the message stays queued and is applied when the shell drains it; if `FindWindowW` still misses, `pending_show` retries on the timer. This replaces the earlier `RegisterWindowMessageW` sketch: a window message cannot be delivered before the window exists.

**Shutdown:** `ShutdownController::request` is idempotent and runs stop hooks once (tray `WM_CLOSE`, settings-writer stop, shared `AtomicBool`). Workers call `acknowledge` and are `join`ed after `application().run` returns, not on the GPUI thread. Dropping `SingleInstance` then releases the mutex.

**Not verified on this machine:** Explorer restart (`TaskbarCreated` is implemented and was not exercised; Explorer was not restarted). A live second process started during window creation. 100% / 150% DPI and a second monitor. Close, minimize, show, and hide were not clicked through a full manual pass. `cargo run` was used only to see whether the process stayed up, then `skrivli.exe` was stopped.

## COM, foreground ownership, commands, shutdown, errors

**COM:** flexaudio owns capture COM apartments on its worker threads. The device-watch thread still calls `CoInitializeEx` / `CoUninitialize` for `IMMNotificationClient` only. COM pointers stay in that apartment. Do not mark those wrappers `Send` or `Sync`. The GPUI foreground thread does not initialize a capture apartment and does not join workers inline.

**Foreground ownership:** `app::run` loads settings and starts logging before `application().run`. The GPUI callback only initializes Kit, applies the theme, and opens the window. Disk and inference do not run inside `render`.

**Command / event bridge:** implemented as D-02. One `AppCommand` path. A foreground `spawn_in` task awaits `BackgroundExecutor::timer` and drains the channel. High-rate meters are still out of scope.

**Shutdown order:** D-02. `request` on the last window or Exit, `cx.quit()`, `application().run` returns, `ShutdownController::join`, drop the mutex, then drop the log guard. No capture threads exist yet.

**Errors:** `AppError` `Display` includes context and the source. `user_message()` is a fixed sentence and does not include paths, audio, or transcript text. Panic hook logs `panic at file:line:column` and does not format the panic payload. Logs are a non-blocking `tracing` subscriber writing `skrivli.log` under `%LOCALAPPDATA%\Skrivli\logs`, rotating at about 1 MB with two archives (three files). Settings unknown `schema_version` values above the supported version fall back to defaults. Saves write a temp sibling, `sync_all`, then `ReplaceFileW` when the destination exists (`std::fs::rename` fails on Windows in that case).

## D-03 — Capture through flexaudio

**Outcome:** microphone and system capture go through **flexaudio 0.8** behind `CapturePort`. Mic uses flexaudio-mic (cpal/WASAPI). System uses flexaudio-os-windows classic WASAPI loopback with silent-render keepalive enabled by default. Device ids for selection come from `flexaudio::devices()` (cpal names / FriendlyName), not MMDevice `GetId`. Hot-plug wake still uses `IMMNotificationClient` on a COM-owning watch thread; re-enumeration calls flexaudio. Skrivli keeps its own `Pipeline`: the aligner places blocks on the session timeline, and the chains apply echo removal, gains, the limiter, and meters. The Rubato drift nudge runs on the session thread. The WAV writer is unchanged. `SourceKind::Mix` is not used (product needs separate meters and single-source degrade).

**Not spiked on hardware:** opening a real flexaudio endpoint was not run in this session. `cargo test` never calls `FlexMicCapture::open` / `FlexSystemCapture::open` against hardware. Mapping helpers and fake recording ports cover the port contract.

## D-04 — Bounded capture queue (microphone and loopback)

**Outcome:** each source sends timestamped blocks on its own `std::sync::mpsc::sync_channel` with capacity 64. On `TrySendError::Full` the **new** packet is rejected, an overrun is counted, and the capture loop does not wait. The aligner keeps the timeline by inserting silence (capped at 5 s per hole) when the next accepted block's sample index jumps ahead, and the session records `overrun`, `gap`, `discontinuity`, and `drift` on the sidecar. There is no custom lock-free queue.

The second source did not change the policy. Microphone and loopback each have one queue, both reject-new. Capacity is 64 blocks. At a 10 ms period that is about 640 ms of audio, which sits past the aligner's 150 ms lateness limit. A longer device period holds fewer milliseconds in the same 64 blocks. Overflow, dropped packets, and the silence that replaces them are counted as overruns and gaps.

**Normal stop drains every layer:** the worker calls flexaudio `stop` and polls the chunks still in flexaudio's ring as its docs direct. It does not resample. The session then closes the stream and pulls the queue empty. The pipeline flushes its resamplers into the aligner before the tail pull, so the end of a recording is not cut off. The flush can add up to about 10 ms of mostly silent audio. Queue rejection still applies during this drain, because the worker runs before `close` returns and cannot wait for the session. A fatal capture error is queued after the remaining audio. A forced stop from low disk, a write failure, or the size limit does not drain.

## D-05 — RIFF size limit

**Outcome:** recordings are canonical PCM16 stereo 48 kHz RIFF, not RF64. Encoding uses the `hound` crate (`WavWriter` + periodic `flush` for header repair, `finalize` on close). The header is 44 bytes. The RIFF size field stores `36 + data_bytes` and must fit in `u32`, so the largest safe payload is `u32::MAX - 36` (`4_294_967_259` bytes, about 6 h 13 min). Before a write that would pass that limit, the writer emits only the frames that still fit, repairs the header, and stops. It does not publish a header whose length fields disagree with the bytes on disk. Lengths are rewritten about every 2 s of audio and again on finalize. A crash leaves `*.wav.partial`; launch repairs a canonical header and renames it to `*.recovered.wav`, or leaves the partial in place when the header is not safe to trust.

## D-06 — Adaptive Rubato resampling for gradual drift

**Outcome:** flexaudio converts each device to 48 kHz stereo `f32`. The pipeline owns one Rubato `Async` polynomial (Cubic) resampler per source on the session thread. Both are constructed at 48 kHz to 48 kHz, so the ratio nudge is the only rate change. It reads that source's timeline error from the aligner directly. There is no `AtomicI64`, and `CapturePort::open` does not take a drift argument. Sudden holes of at least about 1 ms become silence on the shared QPC timeline; sudden overlaps are dropped from the front of the late packet. Gradual clock error stays in the aligner's last error and is applied with `set_resample_ratio` (ramped, ±1% max relative). The aligner does not drop or duplicate frames for gradual drift. Diagnostics count when absolute error passes about 20 ms (`960` frames). Tests cover ±100 ppm error and that a positive drift error increases resampler output length. Whether that error sees a real device clock against QPC was not measured (S9 in this decision).

Classic WASAPI silent-render keepalive is **enabled** via flexaudio’s default `windows_classic_loopback_keepalive`. Idle system capture therefore keeps delivering device-clocked silence instead of stalling; the aligner still fills any remaining timeline gaps.

## D-10 — M4 durability: reopen, rescue, exit, session end

**Outcome (M4):** Mid-session capture errors clear the failed source, keep the other side, fill silence via the aligner, annotate sidecar `gap` / `capture` / `loopback` / `source_change`, and attempt reopen of the Windows default endpoint (`""` device id) on the recording host thread—never inside `IMMNotificationClient`. Device-catalog updates while recording call `RecordingHost::devices_changed` so a missing source can reopen after hot-plug. Free space and writability are rechecked about every 30 s (`SPACE_CHECK_INTERVAL`); below 200 MB the session finalizes with `NOTICE_LOW_DISK`. Write/folder failures release the partial, attempt a bounded rescue copy into `%LOCALAPPDATA%\Skrivli\rescue\` only when bytes are readable, and otherwise report `NOTICE_PARTIAL_KEPT` / `NOTICE_SAVE` without claiming recovery. Cleanup policy: never auto-delete unknown `*.wav.partial` files; `abandon` only after a failed start; startup recovery may rename safe partials to `*.recovered.wav`. Corrupt sidecars are left in place. Explicit Exit while recording opens a confirm dialog (`ConfirmExit`); close-to-tray still only hides. The tray window is a hidden top-level (not `HWND_MESSAGE`) so `WM_QUERYENDSESSION` / `WM_ENDSESSION` emit `SessionEnding` → stop/finalize without confirmation. Worker panic and a simple stall watchdog surface `NOTICE_WORKER` / `NOTICE_STALL`. Hardware unplug and live logoff were not exercised in this session; `cargo test` covers injected faults without devices.

## D-11 — M5 library identity, sidecars, watch, and playback

**Outcome (M5):**

- **Sidecar home:** write+load live in `infrastructure/storage/sidecar.rs` (schema **v3**, still accepts v2). `infrastructure/files/sidecar.rs` re-exports write for the recording service. Transcription fields default empty until M6; a present `.txt` surfaces as Ready.
- **Identity:** `RecordingId` from sidecar `id` when valid; unpaired/malformed entries use a synthetic FNV-1a of the display stem. Selection is preserved by id across rescans and in-app renames that keep the sidecar. External rename of unpaired files may drop selection (no durable id without sidecar).
- **Catalog:** `LibraryHost` scans on a worker; `*.wav.partial` skipped; `*.recovered.wav` listed; `.m4a` listed if present (playback deferred — WAV only). Colliding sidecar ids keep both rows.
- **Watch:** `FindFirstChangeNotificationW` with ~350 ms debounce plus a 30 s poll fallback. Missing folders poll until they appear.
- **Delete:** `SHFileOperationW` with `FOF_ALLOWUNDO` (Recycle Bin) for audio + `.json` + `.txt` after confirm.
- **Playback:** streaming PCM16 WAV via `cpal` 0.16 on a dedicated player thread; selection change stops playback. Device sample-rate negotiation beyond canonical 48 kHz stereo is best-effort (request WAV rate/channels on the default output device).
- **UI:** GPUI Kit `DataTable` + detail pane; Rename uses Kit dialog + `Input`; Delete uses confirm alert. Narrow stacked fallback not implemented (fixed split).

**Not exercised on hardware this session:** large-folder browse (hundreds of files), live Explorer edits while the app is open, and audible WASAPI playback of a real recording.

## D-12 — One platform tree; Windows is the only host

**Outcome:** Every Win32 / COM / `std::os::windows` call lives under `infrastructure/platform/windows/`. `platform/mod.rs` is `cfg(windows)` today and `compile_error!` on other targets. Traits (`DesktopShell`, `Trash`, `DirectoryWatch`, `SingleInstance`, `Tray`, recording clocks/capture) live in `services/ports.rs`. Free functions `replace_file`, `user_paths`, `spawn_device_watch`, `clock`, `disk`, and `performance_counter` are selected in `platform/mod.rs`. flexaudio capture stays in `infrastructure/audio/`; adapters take an injected `PerformanceCounter`. Future OS work follows **Adding a platform** in `skrivli-app-design.md` (§4.4). Product scope stays Windows until another host ships.

## D-07 — whisper.cpp binding pin (CPU, Windows MSVC)

**Outcome:** Pin **`whisper-rs` 0.16** (crates.io semver `0.16`, resolved in `Cargo.lock`) with **default features only** (CPU). No CUDA/Vulkan/Metal/OpenBLAS features. The crate’s `whisper-rs-sys` build script owns the whisper.cpp CMake build; Skrivli does not add a separate FFI crate.

**Adapter boundary:** `infrastructure/ai/whisper.rs` is the only module that holds `WhisperContext` / engine pointers. Services depend on `InferencePort` in `services/ports.rs` (plain `f32` samples, `Arc<AtomicBool>` cancel, `Arc<dyn Fn(u8)>` progress, plain `InferenceSegment` values). `domain/`, `views/`, and other `services/` modules stay free of whisper types and of engine `cfg`.

**Native build requirements (x86_64-pc-windows-msvc):** CMake on `PATH` (VS Build Tools CMake and/or Scoop), a working MSVC toolset (`VsDevCmd -arch=amd64`), and **libclang** for bindgen (`LIBCLANG_PATH` → directory with `libclang.dll`). Machine note: winget `LLVM.LLVM` MSI needed elevation and was cancelled (exit 1602). Successful compile used portable `clang+llvm-18.1.8` under gitignored `.tools/llvm/` (and/or Scoop LLVM). With `LIBCLANG_PATH` set, `whisper-rs` **0.16.0** / `whisper-rs-sys` **0.15.0** built cleanly on this host.

**Licenses / notices:** whisper.cpp and ggml are MIT. Ship whisper.cpp NOTICE/license text with the Windows distribution (M9). Model files are separate downloads (D-08), not bundled.

**Runtime contract:** load one model on the inference worker thread; `FullParams` with `SamplingStrategy::Greedy { best_of: 1 }`; disable engine stdout (`set_print_*` false); progress and abort via safe callbacks; never log transcript text or audio samples. Default `cargo test` uses a fake `InferencePort` and does not download models or run real inference.

## D-08 — Checked-in model manifest (verified HF LFS digests)

**Outcome:** `resources/models/manifest.json` lists the only variants Skrivli offers for download in v1. Source base: `https://huggingface.co/ggerganov/whisper.cpp/resolve/main/`. License: **MIT**. Digests were supplied as verified Hugging Face LFS SHA-256 values for this pin:

| Variant id | Filename | Bytes | SHA-256 |
| --- | --- | ---: | --- |
| `tiny-q5_1` | `ggml-tiny-q5_1.bin` | 32152673 | `818710568da3ca15689e31a743197b520007872ff9576237bda97bd1b469c3d7` |
| `base-q5_1` (DEFAULT) | `ggml-base-q5_1.bin` | 59707625 | `422f1ae452ade6f30a004d7e5c6a43195e4433bc370bf23fac9cc591f01a8898` |
| `small-q5_1` | `ggml-small-q5_1.bin` | 190085487 | `ae85e4a935d7a567bd102fe55afc16bb595bdb618e11b2fc7591bc08120411bb` |

**Download policy:** explicit user action only (`infrastructure/ai/models.rs`). Write a temp sibling, report progress, honor cancel, verify size + SHA-256, promote with `platform::replace_file` only on match. Failed or cancelled downloads leave no accepted model. Bootstrap, recording stop, and background retry must not start a download. Discovery also accepts a user-picked `ggml-*.bin` path; missing / unreadable / unsupported are distinct `AppError` contexts.

**Settings:** schema **v4** stores `selected_model` (default `base-q5_1`) and `auto_transcribe` (default `false`). Older schemas migrate with serde defaults; newer `schema_version` still falls back to defaults.

## D-13 — Sonora 0.2.0

**Outcome:** `sonora = "=0.2.0"` is pinned and the spike questions S1–S8 were measured on synthetic audio. Production uses two instances: `SonoraEchoCanceller` when system audio is captured, always on, with no setting, then `SonoraMicGain`, on separate `AudioProcessing` values. The microphone is mono inside the pipeline. **GAIN GATE: PASS. AEC GATE: PASS. S9: NOT MEASURED.** Hardware checks in the audio pipeline plan were not run. The license text for packaging is `licenses/sonora-BSD-3-Clause.txt`.

**Pin and toolchain:** `sonora` **0.2.0** exact (`Cargo.lock` also has `sonora-aec3`, `sonora-agc2`, `sonora-common-audio`, `sonora-fft`, `sonora-ns`, and `sonora-simd` at 0.2.0). `cargo update -p sonora` changed nothing further. Host compiler: `rustc 1.99.1 (58cf8f9c3 2026-10-13)`, `stable-x86_64-pc-windows-msvc`. Spike code is `src/infrastructure/audio/sonora_spike.rs` (`#[cfg(test)]` only). Slow, device, and timing tests are `#[ignore]`.

**S1 — builds.** Release `skrivli.exe` for `x86_64-pc-windows-msvc` is **33,195,008 bytes before and after** adding the dependency (delta **0**). The binary does not call sonora, so the linker omitted it. The release `libsonora` rlib is 1,135,998 bytes and is linked only into the test harness. Growth of an exe that actually calls sonora was not measured.

**S2 — 479 frames panics.** A 480-frame capture returns `Ok(())`. Capture and render of 479 frames both panic, caught with `catch_unwind`: `range end index 480 out of range for slice of length 479` (`audio_buffer.rs`). It is not an `Error`. Padding to 480 is mandatory.

**S3 — live gain.** 440 Hz tone, amplitude 0.2, gain-only instance. `set_capture_post_gain` applies on the next `process_capture_f32` and ramps across that 10 ms frame. Steady peaks after the ramp, with `capture_level_adjustment` configured:

| Gain | Peak | Expected |
| ---: | ---: | ---: |
| 0.0 | 0.00000 | 0.00000 |
| 0.5 | 0.10000 | 0.10000 |
| 1.0 | 0.20000 | 0.20000 |
| 4.0 | 0.80000 | 0.80000 |

Join steps at the change were 0.00348, 0.00012, 0.00488, and 0.00205. The largest is under the ~0.011 step of the 0.2 tone at unity, so the boundary is not a click. The scaler ramps across that frame (at 4.0× the transition step was 0.04465 against a steady step of 0.04609). Without `capture_level_adjustment`, the same setter left the peak at 0.20000 for every gain. It is a no-op unless that component was configured at build.

**S4 — hard clip.** A 0.9 tone at 1.0× stays at peak **0.89999998**. At 4.0× the settled peak is **1.00000000**, with **396 of 480** samples at `|x| ≥ 0.999`. `soft_limit(3.6)` is **0.94736844**, so sonora hard-clips where today's limiter would not. Unity is usable (no NaN, not silence, not distorted). This is a known difference, not a gain-gate failure.

**S5 — delay.** Impulse of 0.5. Gain-only (`capture_level_adjustment` on, internal rate 48 kHz): onset **0** samples, peak delay **0**, peak **0.50000**. AEC-only (default config, internal rate **32 kHz**): onset **421** samples (**8.77 ms**), peak delay **436** samples (**9.08 ms**), peak **0.14010**. The 32 kHz rate is `Pipeline`'s default `Rate32kHz` cap: AEC turns on band splitting, so 48 kHz audio is resampled down and back. Nine milliseconds is under one mix frame. The mic will not noticeably lag system audio. The gain instance itself adds no delay.

**S6 — AEC with `set_stream_delay_ms(0)`.** Noise-like render, echo at 0.3×, then 3 s of separate speech-like noise. Full-band energy ERLE on the last 2 s of an 8 s echo-only prefix: **40.25 dB at 20 ms**, **40.30 dB at 100 ms**, **60.08 dB at 300 ms**. `statistics().delay_ms` was **16, 96, and 296**. During double-talk, best echo correlation stayed ≤ **0.023**. Speech energy ratio was **0.207 / 0.217 / 0.160**. A silent-render control kept speech energy ratio **0.845** and correlation **+0.560** at a lag of 430 samples. During double-talk the best speech correlation was about **−0.43** at a lag of about 462 samples: the voice is not deleted, and it is not a clean copy. Left-only and right-only echo at 100 ms each gave **22.31 dB** ERLE, so both render channels are references. `statistics().echo_return_loss_enhancement` was about **7.8 dB** on the stereo cases. That field is sonora's capped internal estimate (default LF cap is 4× linear), not the full-band energy ratio above.

**S7 — stereo render, mono capture.** One frame is accepted (`Ok` for render, delay 0, and capture). `Pipeline::default`: max rate **32 kHz**, `multi_channel_render` **true**, `multi_channel_capture` **true**, `capture_downmix_method` **AverageChannels**. `proc_sample_rate_hz()` on the AEC instance is **32000**. Mono capture is not downmixed. Stereo render is kept because `multi_channel_render` is true. Average-channel downmix would apply if render were forced to one channel, or if capture were stereo and the processing buffer were mono. The S6 left/right result is the mix-down check: each side alone is cancelled.

**S8 — CPU.** Release build, 6,000 frames of 10 ms at 48 kHz after 200 warmup frames. Each frame ran two `StereoResampler`s (48 kHz to 48 kHz), the AEC instance (render, `set_stream_delay_ms(0)`, capture), and the gain instance held at 2.0×. Average **88.4 µs per frame (0.088 ms)**. Wall time 530.6 ms. The proposed target was under 2 ms. This run is under it.

**S9 — not measured.** The 30-minute hardware comparison was not run. **Do not invent a ppm number.** A 10 s probe on the default microphone and system loopback did open: mic `frame_index` advanced 480,960 while QPC covered 479,747 frames (delta +1,213, 501 chunks, max `|drift error|` **0**); system advanced 480,000 vs QPC 479,044 (delta +956, 500 chunks, drift error **0**). One flexaudio chunk is 960 frames. Those deltas are about one chunk of poll timing, so they are not a clock-drift measurement. A 10-second probe is not S9. From the code: `map_timeline` anchors once, then uses `base + frame_index`. flexaudio 0.8.1 documents `frame_index` as frames produced and `pts_ns` as not a media clock. `plan_packet` compares that count with the aligner's cursor. After the anchor, the error does not see the device clock against QPC. The drift algorithm was not changed in this refactor. If a later 30-minute run shows the loop cannot see real clock drift, that fix is a separate change. It is not part of this refactor.

**What the pipeline keeps**

- Exact 480-frame buffers. A short buffer panics. Padding to 480 is mandatory, because 479 frames panic inside sonora (`range end index 480 out of range for slice of length 479`). That panic is not a returned `Error`.
- Gain instance must be built with `capture_level_adjustment: Some(...)`. The setter is linear, ramps for one frame, and hard-clips at ±1. Hard-clip at 4× is accepted: S4 showed a 0.9 tone at 4.0× reaches full scale (396 of 480 samples at `|x| ≥ 0.999`) where `soft_limit` would stay near 0.95. The gain step stays sonora.
- AEC order is render, then `set_stream_delay_ms(0)`, then capture. Delay hint 0 is enough for 20–300 ms of synthetic delay.
- Default processing caps AEC at 32 kHz and adds about 9 ms (onset 8.77 ms, peak delay 9.08 ms). That delay is accepted. The gain instance adds none, and the system path is not delayed to match. Setting `maximum_internal_processing_rate` to `Rate48kHz` was not measured.
- Do not treat `echo_return_loss_enhancement` as the full-band ERLE. Session end logs that capped statistic and `delay_ms` when the canceller ran.
- Double-talk keeps some speech energy and distorts it. Headphones (silent render) keep most of the speech energy.

**Production decisions**

- Pin stays `sonora = "=0.2.0"`.
- Two instances, in this order when system audio is on: `SonoraEchoCanceller`, then `SonoraMicGain`, then the microphone peak tap. When system audio is off, there is no canceller. System chain: `FixedGain(0.8)`, then the system peak tap. Mix chain: `SoftLimiter`, then the mixed peak tap.
- Echo removal is always on while system audio is captured. There is no off switch. The stored file is the processed mix.
- Automatic gain is removed. `Agc`, the Auto checkbox, and the `agc_enabled` field are gone. An old settings file that still contains `agc_enabled` loads, and the stored `mic_gain` applies. The settings schema version is unchanged (4).
- The microphone is downmixed to mono (`(L + R) / 2`) before the microphone chain and copied to both WAV channels. A true stereo microphone loses its stereo image. A signal on one side only is about 6 dB down.
- Each source has a `StereoResampler` at 48 kHz to 48 kHz on the session thread. It reads `Aligner`'s error directly. Capture workers have no Rubato. `CapturePort::open` takes the endpoint id and the session origin only.
- Production code imports `sonora` only in `src/infrastructure/audio/sonora_steps.rs`. `src/infrastructure/audio/sonora_spike.rs` is `#[cfg(test)]` measurement code and also imports it. The spike stays. `mixer.rs` is deleted. `StatsTap` and `ChannelTap` exist and are not in the production factory. `LinearGain` remains for `PassThroughPipelineFactory` and is not the production microphone step.
- Sidecar problem code `audio_processing` is noted once at session end. The sidecar schema version is unchanged (3).
- The BSD-3-Clause text is `licenses/sonora-BSD-3-Clause.txt`, copied from the crates.io registry file `sonora-0.2.0/LICENSE` (`index.crates.io-1949cf8c6b5b557f`), not from memory. The copyright lines in that file are: Copyright (c) 2011, The WebRTC Project Authors; Copyright (c) 2016, Arun Raghavan and contributors; Copyright (c) 2026, dignifiedquire. M9 still puts this file in the ZIP. whisper.cpp notices stay an M9 item and are not in the tree (D-07).

**Not verified:** the section 11 hardware checks were not run. That is laptop speakers with a voice call and simultaneous speech, wired headphones, Bluetooth headphones, a Bluetooth speaker, a selected output that is not the device actually playing, unplug and reopen during a session, and a one-hour session. There is no listening comparison and no ERLE measured on a laptop. S9's 30-minute drift measurement was not run, and there is no ppm figure. The 10-second probe above is not that measurement. The release exe size with sonora linked was not measured (S1).
