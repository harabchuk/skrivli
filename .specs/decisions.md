# Skrivli decisions

## D-01 — GPUI Kit revision and Windows window APIs

**Outcome:** pin `gpui-kit` **0.9.0** (crates.io, checksum in `Cargo.lock`). It re-exports GPUI from the package **`gpui-pre` 0.3.11** (library name `gpui`). Do not depend on `gpui` or `gpui-component` separately. Optional `gpui-fast` is not enabled.

**Toolchain observed:** rustc/cargo 1.99.1 via rustup, host `x86_64-pc-windows-msvc`, MSVC 14.51 (Visual Studio 18 Build Tools). Debug `cargo run` opened a window titled Skrivli with the Foundation status line, a Kit **Ready** button, and a Kit select showing Foundation. Tab moved the focus ring onto that select. The window was then closed. 100% / 150% DPI and multi-monitor placement were **not** executed.

**Bootstrap (from the v0.9.0 hello-world sample, not from memory):**

1. `gpui_kit::application().with_assets(gpui_kit::assets::Assets)`
2. `.run(|cx| { ... })`
3. `gpui_kit::init(cx)` before any styled widget
4. `Theme::change(ThemeMode, None, cx)` (`gpui_kit::component`)
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

**Outcome:** microphone and system capture go through **flexaudio 0.8** behind `CapturePort`. Mic uses flexaudio-mic (cpal/WASAPI). System uses flexaudio-os-windows classic WASAPI loopback with silent-render keepalive enabled by default. Device ids for selection come from `flexaudio::devices()` (cpal names / FriendlyName), not MMDevice `GetId`. Hot-plug wake still uses `IMMNotificationClient` on a COM-owning watch thread; re-enumeration calls flexaudio. Skrivli keeps its own mixer, gains, meters, Rubato drift nudge, and WAV writer. `SourceKind::Mix` is not used (product needs separate meters and single-source degrade).

**Not spiked on hardware:** opening a real flexaudio endpoint was not run in this session. `cargo test` never calls `FlexMicCapture::open` / `FlexSystemCapture::open` against hardware. Mapping helpers and fake recording ports cover the port contract.

## D-04 — Bounded capture queue (microphone and loopback)

**Outcome:** each source sends timestamped blocks on its own `std::sync::mpsc::sync_channel` with capacity 64. On `TrySendError::Full` the **new** packet is rejected, an overrun is counted, and the capture loop does not wait. The mixer keeps the timeline by inserting silence (capped at 5 s per hole) when the next accepted block's sample index jumps ahead, and records `overrun`, `gap`, `discontinuity`, and `drift` on the sidecar. There is no custom lock-free queue.

The second source did not change the policy. Microphone and loopback each have one queue, both reject-new. Capacity is 64 blocks. At a 10 ms period that is about 640 ms of audio, which sits past the mixer's 150 ms lateness limit. A longer device period holds fewer milliseconds in the same 64 blocks. Overflow, dropped packets, and the silence that replaces them are counted as overruns and gaps.

## D-05 — RIFF size limit

**Outcome:** recordings are canonical PCM16 stereo 48 kHz RIFF, not RF64. Encoding uses the `hound` crate (`WavWriter` + periodic `flush` for header repair, `finalize` on close). The header is 44 bytes. The RIFF size field stores `36 + data_bytes` and must fit in `u32`, so the largest safe payload is `u32::MAX - 36` (`4_294_967_259` bytes, about 6 h 13 min). Before a write that would pass that limit, the writer emits only the frames that still fit, repairs the header, and stops. It does not publish a header whose length fields disagree with the bytes on disk. Lengths are rewritten about every 2 s of audio and again on finalize. A crash leaves `*.wav.partial`; launch repairs a canonical header and renames it to `*.recovered.wav`, or leaves the partial in place when the header is not safe to trust.

## D-06 — Adaptive Rubato resampling for gradual drift

**Outcome:** each capture stream owns a Rubato `Async` polynomial (Cubic) resampler that converts the device rate to internal 48 kHz stereo `f32`, including when the device already reports 48 kHz. Sudden holes of at least about 1 ms become silence on the shared QPC timeline; sudden overlaps are dropped from the front of the late packet. Gradual clock error is tracked as `AlignState.last_error` and published to an `AtomicI64` that the capture thread reads into `set_resample_ratio` (ramped, ±1% max relative). The mixer no longer drop/duplicates frames for gradual drift. Diagnostics count when absolute error passes about 20 ms (`960` frames). Tests cover ±100 ppm error publication and that a positive drift error increases resampler output length.

Classic WASAPI silent-render keepalive is **enabled** via flexaudio’s default `windows_classic_loopback_keepalive`. Idle system capture therefore keeps delivering device-clocked silence instead of stalling; the mixer still fills any remaining timeline gaps.

## D-10 — M4 durability: reopen, rescue, exit, session end

**Outcome (M4):** Mid-session capture errors clear the failed source, keep the other side, fill silence via the mixer, annotate sidecar `gap` / `capture` / `loopback` / `source_change`, and attempt reopen of the Windows default endpoint (`""` device id) on the recording host thread—never inside `IMMNotificationClient`. Device-catalog updates while recording call `RecordingHost::devices_changed` so a missing source can reopen after hot-plug. Free space and writability are rechecked about every 30 s (`SPACE_CHECK_INTERVAL`); below 200 MB the session finalizes with `NOTICE_LOW_DISK`. Write/folder failures release the partial, attempt a bounded rescue copy into `%LOCALAPPDATA%\Skrivli\rescue\` only when bytes are readable, and otherwise report `NOTICE_PARTIAL_KEPT` / `NOTICE_SAVE` without claiming recovery. Cleanup policy: never auto-delete unknown `*.wav.partial` files; `abandon` only after a failed start; startup recovery may rename safe partials to `*.recovered.wav`. Corrupt sidecars are left in place. Explicit Exit while recording opens a confirm dialog (`ConfirmExit`); close-to-tray still only hides. The tray window is a hidden top-level (not `HWND_MESSAGE`) so `WM_QUERYENDSESSION` / `WM_ENDSESSION` emit `SessionEnding` → stop/finalize without confirmation. Worker panic and a simple stall watchdog surface `NOTICE_WORKER` / `NOTICE_STALL`. Hardware unplug and live logoff were not exercised in this session; `cargo test` covers injected faults without devices.

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
