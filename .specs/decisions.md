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

**Pending manual verification (model and settings UI, Phase 6):** not run, because no GUI was available. These are not verified.
- Step 6.2: at 560x420 with scaling at 100% and 150%, the model dialog and Settings footer buttons stay visible and reachable (Kit scrolls the body).
- Step 6.3: a side-by-side look at rename, delete, tray explanation and exit-while-recording in both themes. Alert dialogs now use `DIALOG_WIDTH` (520px) but keep Kit's built-in buttons, so they have no footer divider.
- Step 6.4: keyboard-only pass over the model dialog, Settings and the status bar item. Tab reaches every radio and button in order, Space selects a radio, Escape closes unless busy, focus rings stay visible.

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

## D-05 — Recording size limit (superseded by D-29)

> Superseded 2026-11-08: recordings are FLAC with a 12 hour cap, see D-29. The text below is the original WAV decision, kept for history.

**Outcome:** recordings are canonical PCM16 stereo 48 kHz RIFF, not RF64. Encoding uses the `hound` crate (`WavWriter` + periodic `flush` for header repair, `finalize` on close). The header is 44 bytes. The RIFF size field stores `36 + data_bytes` and must fit in `u32`, so the largest safe payload is `u32::MAX - 36` (`4_294_967_259` bytes, about 6 h 13 min). Before a write that would pass that limit, the writer emits only the frames that still fit, repairs the header, and stops. It does not publish a header whose length fields disagree with the bytes on disk. Lengths are rewritten about every 2 s of audio and again on finalize. A crash leaves `*.wav.partial` in place; nothing repairs it afterwards (see D-23).

## D-06 — Adaptive Rubato resampling for gradual drift

**Outcome:** flexaudio converts each device to 48 kHz stereo `f32`. The pipeline owns one Rubato `Async` polynomial (Cubic) resampler per source on the session thread. Both are constructed at 48 kHz to 48 kHz, so the ratio nudge is the only rate change. It reads that source's timeline error from the aligner directly. There is no `AtomicI64`, and `CapturePort::open` does not take a drift argument. Sudden holes of at least about 1 ms become silence on the shared QPC timeline; sudden overlaps are dropped from the front of the late packet. Gradual clock error stays in the aligner's last error and is applied with `set_resample_ratio` (ramped, ±1% max relative). The aligner does not drop or duplicate frames for gradual drift. Diagnostics count when absolute error passes about 20 ms (`960` frames). Tests cover ±100 ppm error and that a positive drift error increases resampler output length. Whether that error sees a real device clock against QPC was not measured (S9 in this decision).

Classic WASAPI silent-render keepalive is **enabled** via flexaudio’s default `windows_classic_loopback_keepalive`. Idle system capture therefore keeps delivering device-clocked silence instead of stalling; the aligner still fills any remaining timeline gaps.

## D-10 — M4 durability: reopen, rescue, exit, session end

**Outcome (M4):** Mid-session capture errors clear the failed source, keep the other side, fill silence via the aligner, annotate sidecar `gap` / `capture` / `loopback` / `source_change`, and attempt reopen of the Windows default endpoint (`""` device id) on the recording host thread—never inside `IMMNotificationClient`. Device-catalog updates while recording call `RecordingHost::devices_changed` so a missing source can reopen after hot-plug. Free space and writability are rechecked about every 30 s (`SPACE_CHECK_INTERVAL`); below 200 MB the session finalizes with `NOTICE_LOW_DISK`. Write/folder failures release the partial, attempt a bounded rescue copy into `%LOCALAPPDATA%\Skrivli\rescue\` only when bytes are readable, and otherwise report `NOTICE_PARTIAL_KEPT` / `NOTICE_SAVE` without claiming recovery. Cleanup policy: never auto-delete unknown `*.wav.partial` files; `abandon` only after a failed start; startup recovery may rename safe partials to `*.recovered.wav`. Corrupt sidecars are left in place. Explicit Exit while recording opens a confirm dialog (`ConfirmExit`); close-to-tray still only hides. The tray window is a hidden top-level (not `HWND_MESSAGE`) so `WM_QUERYENDSESSION` / `WM_ENDSESSION` emit `SessionEnding` → stop/finalize without confirmation. Worker panic and a simple stall watchdog surface `NOTICE_WORKER` / `NOTICE_STALL`. Hardware unplug and live logoff were not exercised in this session; `cargo test` covers injected faults without devices.

Rescue copy and startup recovery were removed later, see D-23.

## D-11 — M5 library identity, sidecars, watch, and playback

**Outcome (M5):**

- **Sidecar home:** write+load live in `infrastructure/storage/sidecar.rs` (schema **v3**, still accepts v2). `infrastructure/files/sidecar.rs` re-exports write for the recording service. Transcription fields default empty until M6; a present `.txt` surfaces as Ready.
- **Identity:** `RecordingId` from sidecar `id` when valid; unpaired/malformed entries use a synthetic FNV-1a of the display stem. Selection is preserved by id across rescans and in-app renames that keep the sidecar. External rename of unpaired files may drop selection (no durable id without sidecar).
- **Catalog:** `LibraryHost` scans on a worker; `*.wav.partial` skipped; (Superseded by D-29: only `.flac` is listed; `.wav`, `.m4a` and `*.recovered.wav` are not.) Colliding sidecar ids keep both rows.
- **Watch:** `FindFirstChangeNotificationW` with ~350 ms debounce plus a 30 s poll fallback. Missing folders poll until they appear.
- **Delete:** `SHFileOperationW` with `FOF_ALLOWUNDO` (Recycle Bin) for audio + `.json` + `.txt` after confirm.
- **Playback:** superseded by D-22 (Rodio). Selection change, delete and rename stop playback.
- **UI:** Grouped list (Kit `v_virtual_list` of day headers and rows, 330 px wide) + detail pane (the `DataTable` was replaced in Recordings UI plan phase 1); Rename uses Kit dialog + `Input`; Delete uses confirm alert. Narrow stacked fallback not implemented (fixed split).

**Not exercised on hardware this session:** large-folder browse (hundreds of files), live Explorer edits while the app is open, and audible WASAPI playback of a real recording.

## D-12 — One platform tree; Windows is the only host

**Outcome:** Every Win32 / COM / `std::os::windows` call lives under `infrastructure/platform/windows/`. `platform/mod.rs` is `cfg(windows)` today and `compile_error!` on other targets. Traits (`DesktopShell`, `Trash`, `DirectoryWatch`, `SingleInstance`, `Tray`, recording clocks/capture) live in `services/ports.rs`. Free functions `replace_file`, `user_paths`, `spawn_device_watch`, `clock`, `disk`, and `performance_counter` are selected in `platform/mod.rs`. flexaudio capture stays in `infrastructure/audio/`; adapters take an injected `PerformanceCounter`. Future OS work follows **Adding a platform** in `skrivli-app-design.md` (§4.4). Product scope stays Windows until another host ships.

## D-07 — whisper.cpp binding pin (CPU, Windows MSVC)

**Outcome:** Pin **`whisper-rs` 0.16** (crates.io semver `0.16`, resolved in `Cargo.lock`) with **default features only** (CPU). No CUDA/Vulkan/Metal/OpenBLAS features. The crate’s `whisper-rs-sys` build script owns the whisper.cpp CMake build; Skrivli does not add a separate FFI crate.

**Adapter boundary:** `infrastructure/ai/whisper.rs` is the only module that holds `WhisperContext` / engine pointers. Services depend on `InferencePort` in `services/ports.rs` (plain `f32` samples, `Arc<AtomicBool>` cancel, `Arc<dyn Fn(u8)>` progress, plain `InferenceSegment` values). `domain/`, `views/`, and other `services/` modules stay free of whisper types and of engine `cfg`.

**Native build requirements (x86_64-pc-windows-msvc):** CMake on `PATH` (VS Build Tools CMake and/or Scoop), a working MSVC toolset (`VsDevCmd -arch=amd64`), and **libclang** for bindgen (`LIBCLANG_PATH` → directory with `libclang.dll`). Machine note: winget `LLVM.LLVM` MSI needed elevation and was cancelled (exit 1602). Successful compile used portable `clang+llvm-18.1.8` under gitignored `.tools/llvm/` (and/or Scoop LLVM). With `LIBCLANG_PATH` set, `whisper-rs` **0.16.0** / `whisper-rs-sys` **0.15.0** built cleanly on this host.

**Binding pitfalls (0.16.0):** Do not use `set_abort_callback_safe`: its trampoline casts the boxed closure to the wrong type, so it reads unrelated memory and usually aborts at the end of encoding (`whisper_full` error -6). The adapter installs its own `extern "C"` check over the cancel `AtomicBool`. Segment callback timestamps are in 10 ms units, not milliseconds; the adapter converts them. `real_model_decodes_a_chunk_without_aborting` (`#[ignore]`, set `SKRIVLI_TEST_MODEL`) covers the real engine.

**Licenses / notices:** whisper.cpp and ggml are MIT. Ship whisper.cpp NOTICE/license text with the Windows distribution (M9). Model files are separate downloads (D-08), not bundled.

**Runtime contract:** load one model on the inference worker thread; `FullParams` with `SamplingStrategy::Greedy { best_of: 1 }`; disable engine stdout (`set_print_*` false); progress and abort via safe callbacks; never log transcript text or audio samples. Default `cargo test` uses a fake `InferencePort` and does not download models or run real inference.

## D-08 — Checked-in model manifest (verified HF LFS digests)

**Outcome:** `resources/models/manifest.json` lists the only variants Skrivli offers for download in v1. Source base: `https://huggingface.co/ggerganov/whisper.cpp/resolve/main/`. License: **MIT**. Digests were supplied as verified Hugging Face LFS SHA-256 values for this pin:

| Variant id | Filename | Bytes | SHA-256 |
| --- | --- | ---: | --- |
| `tiny-q5_1` | `ggml-tiny-q5_1.bin` | 32152673 | `818710568da3ca15689e31a743197b520007872ff9576237bda97bd1b469c3d7` |
| `base-q5_1` (DEFAULT) | `ggml-base-q5_1.bin` | 59707625 | `422f1ae452ade6f30a004d7e5c6a43195e4433bc370bf23fac9cc591f01a8898` |
| `small-q5_1` | `ggml-small-q5_1.bin` | 190085487 | `ae85e4a935d7a567bd102fe55afc16bb595bdb618e11b2fc7591bc08120411bb` |
| `large-v3-turbo-q5_0` | `ggml-large-v3-turbo-q5_0.bin` | 574041195 | `394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2` |
| `tiny-en-q5_1` | `ggml-tiny.en-q5_1.bin` | 32166155 | `c77c5766f1cef09b6b7d47f21b546cbddd4157886b3b5d6d4f709e91e66c7c2b` |
| `base-en-q5_1` | `ggml-base.en-q5_1.bin` | 59721011 | `4baf70dd0d7c4247ba2b81fafd9c01005ac77c2f9ef064e00dcf195d0e2fdd2f` |
| `small-en-q5_1` | `ggml-small.en-q5_1.bin` | 190098681 | `bfdff4894dcb76bbf647d56263ea2a96645423f1669176f4844a1bf8e478ad30` |
| `medium-en-q5_0` | `ggml-medium.en-q5_0.bin` | 539225533 | `76733e26ad8fe1c7a5bf7531a9d41917b2adc0f20f2e4f5531688a8c6cd88eb0` |
| `kb-whisper-base-q5_0` | `ggml-kb-whisper-base-q5_0.bin` | 55295450 | `aead29b356bca8840e72a8dc2286e2d69e6702639751a1e60cb3c8eacefec546` |
| `kb-whisper-small-q5_0` | `ggml-kb-whisper-small-q5_0.bin` | 175209680 | `6768836a51abc902e420c613153e6d418c90ea2774e913274d02ab23170225b7` |
| `kb-whisper-medium-q5_0` | `ggml-kb-whisper-medium-q5_0.bin` | 539212484 | `7f8762e0ade9e0073674c0d5acae942a0b1ea98add9baa008ee89c94eaba43d0` |

Added models read from the Hugging Face file API on 2026-11-07. KB-Whisper is **Apache-2.0** from `KBLab/kb-whisper-*`. Upstream, every size is named `ggml-model-q5_0.bin`, so each entry has a unique local `filename` and a `url` pinned to the commit read (base `1499d2d`, small `3564d61`, medium `0abe10b`).

Display copy also lives in the manifest. Ratings were rescaled to fit the larger list: Tiny 3/1, Base 3/2, Small 2/2, Medium and Large Turbo 1/3 (speed/accuracy). They are general comparisons, not measurements. Swedish models depend on a language setting that does not exist yet: no code calls `set_language`.

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

## D-14 — Transcript editor widget (gpui-kit 0.9.0)

**Outcome:** The finished transcript is GPUI Kit `Editor` backed by `EditorState`, created once on the recordings view and rendered with `Editor::new(&editor).readonly(true)`. Confirmed against gpui-kit 0.9.0 via Context7 library `/longbridge/gpui-kit` (editor component docs: read-only still focuses, selects, copies, and searches; `set_value` still works) and the pinned crate `gpui-base` 0.9.0 (`EditorState::new` aims at display of large text up to about 50K lines). A meeting transcript stays under that limit, so no substitute widget is used. A plain `div` of the whole text is not acceptable.

**Scroll:** `InputBaseState::set_value` resets selection and scroll. A transcription progress snapshot must not call it and must not recreate the `EditorState` entity. Load `<basename>.txt` with `background_spawn` inside `spawn_in`, then `set_value` only when the selected recording's text actually changes.

## D-16 — Model dialog can be closed (decision 1 of the model/settings UI plan)

**Outcome:** Approved. The transcription-model dialog can be closed at any time except during a download. This reverses the earlier rule that it stayed open until a model was installed. Closing abandons the pending Transcribe and leaves the saved model unchanged. Phase 1 of `skrivli-model-settings-ui-plan.md` implements it; the hardware/manual checks in that plan's section 6 have not been run.

## D-15 — M7.2 manual pass not run

**Outcome:** Automated tests cover the completion-notification gate, balloon-click selection, and rename/delete identity safety with a fake inference port, a temp directory, and no toast. The hardware journey was **not run**. Do not treat any of the following as observed:

- Record, then transcribe from the detail pane
- Auto-transcribe left off, then turned on for one new recording
- Missing-model prompt satisfied by a verified local file
- Cancel
- Retry after Failed
- Restart with the `.txt` and sidecar Ready still shown
- Network disconnected after the model is installed
- Each failure leaves the WAV playable

No pass or fail result is recorded for those checks. The third M7.2 checkbox and the M7 acceptance checkbox stay open.

**Notification mechanism:** a balloon (`NIF_INFO`) on the existing `Shell_NotifyIconW` icon owned by `Skrivli.TrayMessage`. The notifier port only queues a `RecordingId`. The tray thread shows the balloon. WinRT toasts were not added. `notify_on_transcript` defaults on via `serde(default)` and does not bump settings schema v4.

## D-17 — Settings layout uses own row helpers, not Kit `setting` (Phase 5 of the model/settings UI plan)

**Outcome:** Settings uses `settings_section` and `settings_row` in `ui/components/settings_rows.rs` (uppercase label, bordered group, text left and control right). Kit's `setting` module is not used: its `Settings` component is built around a sidebar with pages, which is too much for the current two rows. Revisit it during M8.1 once the list is longer; if it is adopted, record that here. Settings' **Change model…** dispatches `AppCommand::OpenModelDialog`. The manual checks in the plan (dialog stacking, light/dark rendering) have not been run.

## D-18 — Transcription window is 10 minutes

**Outcome:** `WINDOW_DURATION` went from 30 to 10 minutes, with the overlap still 30 seconds, and the worker passes each window to the engine as a borrowed slice instead of copying it. By calculation, peak audio and spectrogram memory falls from about 290 MB to about 58 MB, excluding the model. About 5% of the audio is now decoded twice, up from about 2%.

**Not verified:** transcript quality at the extra seams was not compared on a long real recording, and memory was not measured.

## D-19 — Local time with `jiff` (Phase 0 of the Recordings UI plan)

**Outcome:** `jiff = "0.2.39"` with default features is used to turn Unix milliseconds into local civil time. The default features include the Windows system zone lookup and a bundled time zone database, so no extra setup is needed. A test that read the system zone gave the expected local offset on the dev PC. A test also confirmed the New York spring-forward change (01:59 to 03:01). `jiff` appears only in `views/recordings.rs`. `domain/` keeps its own `CivilTime`, and `recording_day` compares plain dates. `TimeZone::system()` is called only in `local_clock`, where the views build row data.

**Not verified:** behavior on a PC whose system zone cannot be detected. Jiff then falls back to UTC.

## D-20 — Recordings list is a virtual list; Refresh kept (Phase 1 of the Recordings UI plan)

**Outcome:** the Recordings table (`DataTable`) was replaced by Kit's `v_virtual_list` with fixed heights (32 px headers, 56 px rows), built from `LibraryItem` values in catalog order. A Windows directory watcher is wired in production (`FindFirstChangeNotificationW` on file and directory name changes, size and write time, plus a 30 s full rescan, feeding `LibraryHost`), and a test covers create. Add, rename and delete in Explorer were **not** checked in the running app, so the Refresh icon button stays beside Open folder. Remove it, and `AppCommand::RefreshLibrary`, after that manual check.

## D-21 — Transcript is a paragraph list; text selection dropped (Phase 3 of the Recordings UI plan)

**Outcome:** the read-only Kit `Editor` is gone. The finished transcript is a GPUI `list` element with one multiline item per paragraph (`domain::transcript::parse_transcript`), and a Kit `Clipboard` button copies the whole file text. Selecting part of the transcript is **no longer supported**; the `.txt` file stays available for anything finer. This supersedes D-14 (the editor widget and its `set_value` scroll reset). The scroll rule from D-14 still holds: the list resets only when the loaded text changes.

**Why GPUI's `list`, not Kit's `List`:** Kit's `ListState` measures one item and gives every row that size (`gpui-component-0.9.0/src/list/list.rs`, `prepare_items_if_needed`), so paragraphs of different lengths would be cut off or overlap. GPUI's `list` (`gpui-pre-0.3.11/src/elements/list.rs`) measures each item and draws only those in view. It is re-exported by `gpui_kit` (`pub use ::gpui::*`), so no second GPUI version is pulled in. Import it as `ListState as ParagraphListState`.

**Not verified:** the running app (scroll feel, wrapping on resize, Copy with Unicode, PageUp and PageDown focus handling, the Spinner and Progress look).

## D-22 — Playback engine is Rodio 0.21.1 (Phase 5 of the Recordings UI plan)

**Outcome:** playback uses `rodio = "=0.21.1"` with `default-features = false` and features `playback` and `hound` (`hound` removed by D-29; only `playback` remains), behind the `PlaybackOutput` and `PlaybackSession` ports. Rodio 0.21.1 depends on cpal `^0.16`, so `Cargo.lock` still has one cpal (0.16.0) and one hound (3.5.1), and no symphonia. Rodio 0.22 needs cpal 0.17, so do not upgrade without moving the capture code too. The cpal dependency stays for device code.

**Verified by reading the Rodio 0.21.1 source and by building:** the player type is `Sink` (`Sink::connect_new(stream.mixer())`, `append`, `pause`, `play`, `try_seek`, `get_pos`, `empty`). The stream comes from `OutputStreamBuilder::open_default_stream()` and holds a `cpal::Stream`, so it is kept with the sink inside one session object on the player worker (sink dropped before stream). `Decoder::new_wav` with the `hound` feature gives `total_duration` and frame-based seeking (`hound` `seek`, saturated at the end). `try_seek` blocks until the audio thread answers (a few ms, the sink polls every 5 ms), runs on the worker only, and silently does nothing if the sink is already empty. `get_pos` is refreshed every 5 ms by the sink. The mixer wraps every source in `UniformSourceIterator`, so a file rate or channel count different from the device is converted in Rodio. The hound seek converts the time to frames with `f32`, so on a 1 h 48 kHz file the target can be off by a few frames (under 1 ms); this is source reading, not a measurement. Unit tests use a fake `PlaybackOutput` and need no hardware.

**NOT verified on hardware:** seek accuracy and latency on a WAV longer than one hour; playback when the device sample rate differs from the file (conversion quality, no click); creating the `OutputStream` on the worker thread on Windows; audible playback to the end and the return to `Idle`; pause and seek with no click or hang; behavior when the default output device is removed during playback. Run these before Phase 6 is called done.

## D-23 — Rescue and crash recovery removed

**Removed:** the rescue copy to `%LOCALAPPDATA%\Skrivli\rescue\`, the startup repair that turned `*.wav.partial` into `*.recovered.wav`, and the Recovered flag, badge and note.

**New failure rule:** after a failure, always attempt finalize. If finalize fails, leave the `.wav.partial` where it is and report the original failure. A crash or kill leaves the partial untouched and the next launch says nothing.

**No backward compatibility:** old `.wav.partial` files are ignored (not listed, repaired, renamed or deleted). Old `.recovered.wav` files are listed as plain WAVs with no badge or note. The old rescue folder is ignored and never deleted.

**Why:** less surface for a path that is rarely hit, and no hidden copies of recordings outside the folder the user chose. The trade-off is accepted: a recording lost to a crash is not offered back.


## D-24 — Static CRT (release plan R2)

**Outcome:** `.cargo/config.toml` sets `crt-static` for `x86_64-pc-windows-msvc`. `dumpbin /dependents` on the release exe shows no `vcruntime140`, `msvcp140` or `api-ms-win-crt` imports. The remaining non-base DLLs are `icuuc.dll` (Windows 10 and later) and `mmdevapi.dll`. The whisper.cpp licence text in the ZIP is copied from the vendored source of `whisper-rs-sys` 0.15.0.

**whisper.cpp must match:** `.cargo/config.toml` `[env]` sets `CMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded` and `CMAKE_POLICY_DEFAULT_CMP0091=NEW` (whisper.cpp declares `cmake_minimum_required(3.5)`, so the policy is OFF otherwise). `whisper-rs-sys` forwards `CMAKE_*` env vars to CMake. Without both, a clean build compiles ggml with `/MD` and the link fails on `__imp_fgetc`, `__imp_realloc` and other CRT imports. A stale local `target/` can hide the failure; the build script does not rerun on env changes, so run `cargo clean -p whisper-rs-sys` after changing them.

**Not verified:** a clean VM without Visual Studio (M9.2 stays open).

## D-25 — Release tools pinned (release plan R4, R6)

**Outcome:** `cargo-release` 1.1.7: installed, `release.toml` parsed, branch guard confirmed. `cargo-about` 0.9.3: install needs `--features cli`; generation tested locally (about 510 crates); `about.toml` accepts the extra licences `CDLA-Permissive-2.0` and `Unlicense`.

**Not verified:** a full `cargo release patch` dry run on a clean `main`.

## D-26 — Signing deferred

**Outcome:** the exe is unsigned for now. The ZIP README tells users about the SmartScreen warning. Revisit when the distribution channel needs signing.

## D-27 — Release built in GitHub Actions

**Outcome:** a `v*` tag push runs `release.yml`, which builds, packages and publishes the GitHub release immediately (no draft).

**Not verified:** `release.yml` and `scripts/package.ps1` end to end on a fork (R6 check). R7, the first real release, is not done.

## D-28 — Version comes only from `Cargo.toml` (release plan R1)

**Outcome:** `build.rs` no longer sets a version. The release exe reports `ProductVersion` 0.1.0 and `FileVersionRaw` 0.1.0.0, matching `Cargo.toml`.

## D-29 — Release files live under `releases/`

**Date:** 2026-11-08. **Outcome:** Release-only files moved into one folder: `releases/package.ps1` (was `scripts/`), `releases/release-notes-header.md` (was `.github/`), `releases/about.toml` and `releases/about.hbs` (was root; CI passes `-c releases/about.toml`), `releases/dist/` and `releases/licenses/` (were root). `release.toml` was folded into `Cargo.toml` as `[package.metadata.release]`, since cargo-release only reads config from the root. `.github/workflows/release.yml` stays where GitHub requires it. Paths inside the ZIP are unchanged. `scripts/build_run.bat` stays in `scripts/` (dev tool, not release).

## D-30 — WAV replaced by FLAC (2026-11-08)

**Outcome:** FLAC is the only format Skrivli saves, plays and transcribes: 16-bit stereo 48 kHz, compression level 5, `Skrivli_YYYY-MM-DD_HH-MM-SS[-N].flac`, written as `.flac.partial` and renamed on finalize. Implemented in `infrastructure/files/flac.rs` (`FlacWriter`), `player.rs` (`FlacSource`) and `transcript_reader.rs`.

**Pinned:** `flac-codec = "1.3"` in `Cargo.toml`, locked at **1.3.3** in `Cargo.lock`. Rodio stays `=0.21.1` with only the `playback` feature. `hound` and rodio's `hound` feature are removed. `RecordingContainer` has only `Flac` (`Wav` and `M4a` removed).

**Durability:** blocks are flushed and `sync_all`ed about every 2 s. A crash leaves `.flac.partial` with a STREAMINFO that has no total samples or MD5; nothing repairs or lists it (D-23 still applies).

**Padding:** the encoder reserves an 80,000-byte PADDING block. At finalize the writer rewrites STREAMINFO and a seek table into that space in place, without moving audio. One seek point per 10 s at 18 bytes each is 77,760 bytes for 12 h, so the table fits. Playback seeks then do not decode from the start.

**Cap:** the 4 GB RIFF safe stop (D-05) is replaced by a **12 hour duration cap**. At the cap the writer keeps the frames that fit, the session finalizes, and the user sees "Recording stopped because it reached the 12 hour limit."

**No backward compatibility:** old `.wav`, `.m4a`, `.wav.partial` and `.recovered.wav` files are not listed, played or transcribed. They are never touched or deleted.

**Not verified on hardware:** seek accuracy and latency on a FLAC longer than one hour, and playback of a 12 h file. Unit and integration tests need no hardware.

## D-31 — Maximum meeting duration is a user setting (2026-11-08)

**Outcome:** the fixed 12 hour cap from D-29 becomes the **Maximum meeting duration** setting (Settings → Recording), default **3 hours**, choices 1, 2, 3, 4, 6, 8 and 12 hours. It protects against a forgotten recording writing a very long file. Stored as `max_meeting_hours` in settings JSON (no schema bump; missing or unlisted values load as 3). The value is read when a recording starts. 12 hours stays the hard ceiling because the 80,000-byte seek-table padding covers 12 h. The notice is now "Recording stopped because it reached the maximum meeting duration." (no hour count, so it stays a static string).

## D-32 — Disabled audio sources are a stored state (2026-11-09)

**Outcome:** turning off the microphone or system audio is `StoredEndpoint::Disabled` (`{"mode":"disabled"}`), not a magic device id. The settings schema version is unchanged (additive). An older build reading it falls back to defaults, which is accepted before v1. `EndpointChoice::Disabled` and `ResolveError::Disabled` keep "turned off" apart from "no device". An intentionally disabled source shows no degraded warning; the sidecar `microphone_id` or `system_audio_id` is `"disabled"`. The menu label is **Disabled**. Starting with both sources disabled is refused in `build_start_request`, so the Record button, the tray and the hotkey share one check.
