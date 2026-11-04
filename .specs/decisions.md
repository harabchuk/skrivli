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

## Single instance

Mutex name: `Local\Skrivli.SingleInstance` (`infrastructure/windows/single_instance.rs`). The name is an argument so tests use a unique `Local\...` name. The guard is `!Send`; `ReleaseMutex` runs on the acquiring thread in `Drop`.

If the mutex already exists, the second process logs and exits 0. It does not open a window and does not read or write recordings. Startup order is logs, then mutex, then local `models`/`rescue` folders and settings. A second process stops before those.

**M1 handshake, not implemented:** the loser should `RegisterWindowMessageW` (or a small authenticated local IPC) to ask the winner to show and focus, then exit. If the winner has not created its window yet, it must store a pending show and apply it when the window exists. Doing the signal before the window exists is the startup race.

## COM, foreground ownership, commands, shutdown, errors

**COM (design only, no WASAPI yet):** each capture or other Windows worker that touches COM calls `CoInitializeEx` / `CoUninitialize` on that thread. COM pointers stay in that apartment. Do not mark those wrappers `Send` or `Sync`. The GPUI foreground thread does not initialize a capture apartment and does not join workers inline.

**Foreground ownership:** `app::run` loads settings and starts logging before `application().run`. The GPUI callback only initializes Kit, applies the theme, and opens the window. Disk and inference do not run inside `render`.

**Command / event bridge (M1 sketch):** one `AppCommand` path for toolbar, tray, and hotkey. Workers send discrete events on a bounded channel. A foreground `cx.spawn` drains that channel and updates entities. High-rate meters stay in atomics and are sampled by the timer above.

**Shutdown order:** last window closed → `cx.quit()` → `application().run` returns → drop the mutex guard → drop the log worker guard (declared first so it flushes last). No capture threads exist yet.

**Errors:** `AppError` `Display` includes context and the source. `user_message()` is a fixed sentence and does not include paths, audio, or transcript text. Panic hook logs `panic at file:line:column` and does not format the panic payload. Logs are a non-blocking `tracing` subscriber writing `skrivli.log` under `%LOCALAPPDATA%\Skrivli\logs`, rotating at about 1 MB with two archives (three files). Settings unknown `schema_version` values above 1 fall back to defaults. Saves write a temp sibling, `sync_all`, then `ReplaceFileW` when the destination exists (`std::fs::rename` fails on Windows in that case).
