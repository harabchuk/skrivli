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

**Outcome:** M1 uses three separate Windows objects. The GPUI window stays on the GPUI thread. A dedicated worker thread owns a message-only window of class `Skrivli.TrayMessage` and pumps `GetMessageW` there. That procedure is not attached to the GPUI HWND (`Zed::Window`). Tray commands are `TrayCommand` values sent on an `std::sync::mpsc` channel. A foreground task started with `Context::spawn_in` awaits `BackgroundExecutor::timer` (150 ms) and drains the channel on the GPUI thread, coalescing `ShellPulse` events into one presentation update. The same path serves menus (`cx.on_action` → `dispatch` → `MainWindow::handle`).

**Notify icon:** `Shell_NotifyIconW` with GUID `{7C3E8A21-4B6D-4E91-9F2A-1D5C8B0E6A44}`, tooltip `Skrivli`, and `NOTIFYICON_VERSION_4`. `NIM_DELETE` runs on `WM_CLOSE` before the thread exits. `RegisterWindowMessageW("TaskbarCreated")` calls `NIM_ADD` again. The icon is the system application icon, not a recording-state icon.

**Show and hide:** `Platform::hide` on gpui-pre 0.3.11 Windows is a no-op, and `Window` has no hide method. The shell finds `Zed::Window` titled `Skrivli` and calls `ShowWindow` (`SW_SHOW` plus `Window::activate_window`, or `SW_HIDE`). It does not subclass that HWND. `window.on_window_should_close` returns false when `close_to_tray` is set (default true) and hides instead; the first hide opens a Kit alert and stores `tray_explanation_seen`. Explicit Exit calls `window.remove_window`, which does not go through that callback, then `cx.quit`. Minimize is not a GPUI callback: the same foreground timer sees `IsIconic` and hides when close-to-tray is on. `WindowOptions.window_min_size` is 560×420, and `domain::window_frame` clamps saved bounds against injected work-area rectangles. Restored bounds use `PlatformDisplay::visible_bounds`. Saves are debounced about 400 ms and written by a background settings thread, not from `render`.

**Single instance:** mutex `Local\Skrivli.SingleInstance`. The winner creates auto-reset event `Local\Skrivli.Activate` on a listener thread before `application().run`. The loser opens that event, `SetEvent`, and exits 0 without a window and without reading or writing recordings. The listener posts `AppCommand::Show` on the same channel. If the GPUI window is not up yet, the message stays queued and is applied when the shell drains it; if `FindWindowW` still misses, `pending_show` retries on the timer. This replaces the earlier `RegisterWindowMessageW` sketch: a window message cannot be delivered before the window exists.

**Shutdown:** `ShutdownController::request` is idempotent and runs stop hooks once (tray `WM_CLOSE`, settings-writer stop, shared `AtomicBool`). Workers call `acknowledge` and are `join`ed after `application().run` returns, not on the GPUI thread. Dropping `SingleInstance` then releases the mutex.

**Not verified on this machine:** Explorer restart (`TaskbarCreated` is implemented and was not exercised; Explorer was not restarted). A live second process started during window creation. 100% / 150% DPI and a second monitor. Close, minimize, show, and hide were not clicked through a full manual pass. `cargo run` was used only to see whether the process stayed up, then `skrivli.exe` was stopped.

## COM, foreground ownership, commands, shutdown, errors

**COM (design only, no WASAPI yet):** each capture or other Windows worker that touches COM calls `CoInitializeEx` / `CoUninitialize` on that thread. COM pointers stay in that apartment. Do not mark those wrappers `Send` or `Sync`. The GPUI foreground thread does not initialize a capture apartment and does not join workers inline.

**Foreground ownership:** `app::run` loads settings and starts logging before `application().run`. The GPUI callback only initializes Kit, applies the theme, and opens the window. Disk and inference do not run inside `render`.

**Command / event bridge:** implemented as D-02. One `AppCommand` path. A foreground `spawn_in` task awaits `BackgroundExecutor::timer` and drains the channel. High-rate meters are still out of scope.

**Shutdown order:** D-02. `request` on the last window or Exit, `cx.quit()`, `application().run` returns, `ShutdownController::join`, drop the mutex, then drop the log guard. No capture threads exist yet.

**Errors:** `AppError` `Display` includes context and the source. `user_message()` is a fixed sentence and does not include paths, audio, or transcript text. Panic hook logs `panic at file:line:column` and does not format the panic payload. Logs are a non-blocking `tracing` subscriber writing `skrivli.log` under `%LOCALAPPDATA%\Skrivli\logs`, rotating at about 1 MB with two archives (three files). Settings unknown `schema_version` values above the supported version fall back to defaults. Saves write a temp sibling, `sync_all`, then `ReplaceFileW` when the destination exists (`std::fs::rename` fails on Windows in that case).

## D-03 — WASAPI through the `windows` crate

**Outcome:** microphone capture calls WASAPI directly through `windows` 0.58 (`IAudioClient`, `IAudioCaptureClient`, `IMMDeviceEnumerator`). No `wasapi` / `cpal` wrapper. The capture thread calls `CoInitializeEx(COINIT_MULTITHREADED)` and releases that apartment, the audio client, and the event handle with `Drop`. Those COM pointers are not `Send` or `Sync`.

**Not spiked on hardware:** opening a real capture endpoint was not run in this session. `cargo test` never calls `WasapiCapture::open`. The shared-mode event-driven path (format probe, ~200 ms buffer when the engine period allows it, packet drain, silence and discontinuity flags, QPC timestamps) is compiled and covered by pure helpers plus a fake port.

## D-04 — Bounded capture queue (microphone and loopback)

**Outcome:** each source sends timestamped blocks on its own `std::sync::mpsc::sync_channel` with capacity 64. On `TrySendError::Full` the **new** packet is rejected, an overrun is counted, and the capture loop does not wait. The mixer keeps the timeline by inserting silence (capped at 5 s per hole) when the next accepted block's sample index jumps ahead, and records `overrun`, `gap`, `discontinuity`, and `drift` on the sidecar. There is no custom lock-free queue.

The second source did not change the policy. Microphone and loopback each have one queue, both reject-new. Capacity is 64 blocks. At a 10 ms period that is about 640 ms of audio, which sits past the mixer's 150 ms lateness limit. A longer device period holds fewer milliseconds in the same 64 blocks. Overflow, dropped packets, and the silence that replaces them are counted as overruns and gaps.

## D-05 — RIFF size limit

**Outcome:** recordings are canonical PCM16 stereo 48 kHz RIFF, not RF64. The header is 44 bytes. The RIFF size field stores `36 + data_bytes` and must fit in `u32`, so the largest safe payload is `u32::MAX - 36` (`4_294_967_259` bytes, about 6 h 13 min). Before a write that would pass that limit, the writer emits only the frames that still fit, repairs the header, and stops. It does not publish a header whose length fields disagree with the bytes on disk. Lengths are rewritten about every 2 s of audio and again on finalize. A crash leaves `*.wav.partial`; launch repairs a canonical header and renames it to `*.recovered.wav`, or leaves the partial in place when the header is not safe to trust.

## D-06 — Sparse clock correction, not resampling

**Outcome:** the first mix correction is sparse. Each source appends packets on the shared QPC timeline. A sudden hole of at least about 1 ms (including the 5 ms case) becomes silence. A sudden overlap is dropped from the front of the late packet. Gradual drift is left alone until it passes about 20 ms (`960` frames at 48 kHz). Past that threshold the mixer drops or duplicates frames until the error is back at the threshold. Tests cover ±100 ppm.

Adaptive resampling is deferred. Use it only if a real two-source recording makes these splices audible.

A silent-render keep-alive on the output endpoint is also deferred. WASAPI loopback may deliver no packets during silence; that is treated as a normal gap and filled from the session clock. The keep-alive stays out until a real device shows it is inaudible and does not change power behavior.
