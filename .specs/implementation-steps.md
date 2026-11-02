# MeetingBuddy — Implementation Steps

Ordered build plan derived from [app-design.md](app-design.md). That document is the design source of truth. If a step and the design disagree, follow the design and update this file.

Implement the milestones in order. Each one ends with a running app: `scripts\start.bat` builds Release warning-clean at `/W4` and launches `MeetingBuddy.exe`. Do not start a later milestone until the current one's checks pass.

Conventions, thread rules, and the verification checklist live in `AGENTS.md`. Read the cited spec section before starting a step.

v1 does not include speaker diarization, video, cloud sync, accounts, telemetry, or live transcription.

---

## Baseline

Already in the tree:

- `CMakeLists.txt` — C++17, `LANGUAGES CXX` only, WTL fetched from `Win32-WTL/WTL` tag `main`, `/W4`, `mainCRTStartup`.
- `src/main.cpp`, `src/Application.*` — `CAppModule`, message loop, `CoInitialize` (STA), common controls init.
- `src/MainWindow.*` — fixed-size `CFrameWindowImpl`, one Record/Stop button that only toggles its caption. Close destroys the window.
- `src/Window.hpp` — unused duplicate of that window. Delete it in M0.
- `src/Ids.hpp` — `IDC_RECORD`, `IDR_MAINFRAME`. No `.rc`, so the frame icon id points at nothing.
- `src/pch.hpp` — ATL/WTL includes.
- `scripts/start.bat` — configure on first run, kill a running instance, build Release, launch.

There is no audio, tray, settings, logging, resources, or tests yet.

---

## Milestone index

| | Milestone | Spec | Result |
|---|---|---|---|
| M0 | Foundation | §2, §8.6, §9, §10 | C++20, pinned WTL, resources, log, settings, single instance |
| M1 | Shell UI | §8.1, §8.5 | Resizable tabs, status bar, tray, hide to tray |
| M2 | Mic capture | §6.1–§6.2, §6.5–§6.6, §8.2 | Record/Stop writes a playable WAV |
| M3 | Mix | §6.3–§6.4, §8.2 | Mic + system audio, meters, mixer tests |
| M4 | Robustness | §6.2, §6.5–§6.6, §9 | Device loss, disk, crash recovery, exit while recording |
| M5 | Recordings | §6.5, §8.3 | Library, list, play/rename/delete, watcher |
| M6 | Transcription engine | §7, §10 | whisper.cpp, resample, models, Markdown, queue |
| M7 | Transcript UI | §7.5, §8.2, §8.4 | Viewer, status column, balloon |
| M8 | Settings and polish | §8.2, §8.5, §8.7, §6.5 | Dialog, hotkey, startup, DPI, optional M4A |
| M9 | Release | §10, §11 | Portable zip, README, manual test pass |

---

## Rules for every milestone

- UI thread does windows only. Capture threads only copy packets and QPC timestamps into the ring buffer (MMCSS `Audio`). No allocation, lock, log, or I/O on that hot path.
- Mixer/writer and transcription work run off the UI thread. Workers notify the UI only with `PostMessage(WM_APP_*)`.
- Heap payloads posted to the UI are owned by the handler that receives them. `wParam` / `lParam` never point at stack or ring-buffer memory.
- COM: `CoInitializeEx(COINIT_APARTMENTTHREADED)` on the UI thread, `COINIT_MULTITHREADED` on workers. Check every `HRESULT`. No exceptions across COM, window-proc, or thread boundaries.
- New control, menu, resource, and `WM_APP` ids go in `Ids.hpp` / `resources/resource.h`. No magic numbers.
- Every new `.cpp` is added to `add_executable` in `CMakeLists.txt`. DSP and file code stay free of WTL so tests can link them without the UI.
- `#include "pch.hpp"` first in any header or source that uses ATL/WTL.
- App data paths, and nowhere else: settings `%APPDATA%\MeetingBuddy\settings.ini`; models `%LOCALAPPDATA%\MeetingBuddy\models\`; logs `%LOCALAPPDATA%\MeetingBuddy\logs\`; rescue `%LOCALAPPDATA%\MeetingBuddy\rescue\`. Default recordings folder: `%USERPROFILE%\Documents\MeetingBuddy`.
- The only network access in the whole app is the user-initiated model download (M6). Do not add telemetry.

---

## M0 — Foundation

### M0.1 Toolchain

**Spec:** §2, §10.

- Set `project(MeetingBuddy LANGUAGES C CXX)` and `CMAKE_CXX_STANDARD 20`.
- MSVC options: `/W4 /permissive- /utf-8`. Keep `UNICODE`, `_UNICODE`, `WIN32_LEAN_AND_MEAN`, `_WTL_USE_CSTRING`, and `/ENTRY:mainCRTStartup`.
- Pin WTL `GIT_TAG` to a WTL 10 release tag. Do not track `main`. Record the tag in `CMakeLists.txt`.
- Delete `build/CMakeCache.txt` (or reconfigure) so the pin and the language change are picked up.

**Done when:** a clean configure + Release build succeeds via `scripts\start.bat`.

### M0.2 Entry, COM, and dead code

**Spec:** §2, §4.

- Delete `src/Window.hpp`.
- Replace `CoInitialize` with `CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED)`.
- Keep module init and shutdown around the message loop, as they are now.

**Done when:** the app still opens the current window, and `Window.hpp` is gone from the build.

### M0.3 Resources and manifest

**Spec:** §2, §5, §10.

- Add `resources/MeetingBuddy.rc`, `resources/resource.h`, and `resources/MeetingBuddy.manifest`.
- Manifest: Common Controls v6 dependency, Per-Monitor V2 DPI awareness.
- Icons: `app.ico` (window), `tray_idle.ico`, `tray_recording.ico` (recording icon has a red dot). `IDR_MAINFRAME` loads the window icon.
- `resource.h` owns resource ids. `Ids.hpp` includes it and is where `WM_APP_*` messages will go.
- Add the `.rc` to the executable target.

**Done when:** the window shows the app icon, and the resource compiler is part of the Release build.

### M0.4 Paths, logging, and HRESULT helpers

**Spec:** §5, §9.

- `src/util/Paths` — resolve the settings, models, logs, rescue, and default recordings directories, and create them when needed.
- `src/util/Log` — append to `%LOCALAPPDATA%\MeetingBuddy\logs\meetingbuddy.log`, rotate at about 1 MB, keep three files. `Info` / `Warn` / `Error`, including an `HRESULT` overload. Logging is for UI and session threads, never for the capture hot path.
- `src/util/HResult` — small helper to log and return a failed `HRESULT`. No naked `Release()`.

**Done when:** a startup line appears in the log, and the helpers have no WTL dependency.

### M0.5 Settings

**Spec:** §3, §8.7.

`src/storage/Settings` reads and writes `%APPDATA%\MeetingBuddy\settings.ini` with `GetPrivateProfileString` / `WritePrivateProfileString`. Load on startup, save on change. Missing file means defaults.

Use these keys so later milestones do not invent a second schema:

```
[General]
CloseToTray=1
StartWithWindows=0
StartMinimized=0
HotkeyEnabled=1
Hotkey=Ctrl+Alt+R
FirstRunTrayHintShown=0
WindowX, WindowY, WindowW, WindowH   (absent until the user moves the window)

[Recording]
Folder=                          (empty → Documents\MeetingBuddy)
MicDeviceId=
SystemDeviceId=
MicGain=1.0
SystemGain=0.8
SaveStems=0
CompressToM4a=0
KeepWav=1
CommunicationsProcessing=0

[Transcription]
Model=ggml-base-q5_1.bin
Language=auto
AutoTranscribe=0
Threads=0                        (0 → hardware_concurrency - 1)
UseVad=0
```

The settings dialog is M8. M0 only needs load, save, and defaults.

**Done when:** a round-trip of load → mutate → save → load preserves every key, and a missing file yields the defaults above.

### M0.6 Single instance

**Spec:** §8.6.

- Named mutex `Local\MeetingBuddy.SingleInstance`, created before the window.
- If it already exists, `FindWindow` the class `MeetingBuddy.MainWindow`, send a registered "show yourself" message, and exit without starting a second message loop.
- The existing window handles that message by restoring and activating itself.

**Done when:** a second launch brings the running window forward and does not open another frame.

### M0.7 M0 check

- `scripts\start.bat` builds and launches.
- Build is warning-clean at `/W4`.
- No files written outside the app-data paths above.

---

## M1 — Shell UI

Move UI types under `src/ui/` and update `CMakeLists.txt` includes. `MainWindow` moves with them.

### M1.1 Resizable frame

**Spec:** §8.1.

- Create the frame with a normal resizable overlapped style (the current style strips `WS_THICKFRAME` and `WS_MAXIMIZEBOX`).
- Minimum client size about 560×420.
- Restore and save `WindowX/Y/W/H` from settings. Ignore saved coordinates that are fully off every monitor.

**Done when:** the window resizes and maximizes, and reopens at the last size and position.

### M1.2 Menu and status bar

**Spec:** §8.1.

- Menu bar from the resource script.
  - **File:** Open recordings folder, Settings…, Exit.
  - **Help:** About.
- Settings… and About are enabled stubs (a short message) until M8.
- Open recordings folder opens the settings folder in Explorer, creating the default folder if needed.
- `CMultiPaneStatusBarCtrl` with three panes: recording state, elapsed time, transcription progress. Idle text is enough in this milestone.

**Done when:** every menu item is wired, and the status bar tracks the frame's client width.

### M1.3 Tabs

**Spec:** §8.1, §5.

- Stock `CTabCtrl` centered above the pages: **Record** and **Recordings**. Pages fill the width below the strip.
- Each page is a child `CDialogImpl` with `CDialogResize`: `DS_CONTROL | WS_CHILD` dialog resources `IDD_RECORD_PAGE` and `IDD_RECORDINGS_PAGE`.
- Switching tabs shows one page and hides the other.
- Placeholder copy only. Record's real controls arrive in M2; the recordings list arrives in M5.

**Done when:** both tabs show their page, layout survives resize, and the old centred toggle button is gone.

### M1.4 Tray icon

**Spec:** §8.5.

`src/ui/TrayIcon` wraps `Shell_NotifyIconW` with `NOTIFYICON_VERSION_4`, a GUID-identified icon, and `NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP`.

- Idle icon and tooltip "MeetingBuddy". The recording icon is swapped in M2.
- Left click toggles main-window visibility.
- Right-click menu: **Show MeetingBuddy**, **Start recording** / **Stop recording**, **Open recordings folder**, **Settings…**, **Exit**.
- Start/Stop stays disabled until M2. Settings… uses the same stub as the menu bar.

**Done when:** the icon is visible, left click hides and shows the window, and Exit quits.

### M1.5 Hide to tray, and Explorer restarts

**Spec:** §8.5.

- `SC_MINIMIZE` hides the window instead of minimizing to the taskbar.
- The close button hides to the tray when `CloseToTray` is on (default). Exit from the menu or tray still quits.
- The first hide shows one balloon: "MeetingBuddy is still running in the tray." Persist that with `FirstRunTrayHintShown`.
- On `RegisterWindowMessage(L"TaskbarCreated")`, add the icon again.

**Done when:** minimize and close leave the process running with a tray icon; Exit ends it; restarting Explorer brings the icon back.

### M1.6 M1 check

- `scripts\start.bat` launches the tabbed window.
- The UI thread does no disk or audio work beyond settings load/save.
- DPI: handle `WM_DPICHANGED` by applying the suggested rect. Finer control scaling is M8.

---

## M2 — Microphone capture to WAV

Capture threads must not write files. Land `RingBuffer` here, even though the milestone list names it under M3, and drain it on a writer thread. M3 adds the second source, the mixer, and alignment. Until then the writer converts mic float frames to PCM and writes them in order.

### M2.1 Messages and session states

**Spec:** §4, §6.6.

Add to `Ids.hpp`:

- `WM_APP_RECORDING_STARTED`
- `WM_APP_RECORDING_FINISHED` (heap `std::wstring*` path; UI deletes it)
- `WM_APP_RECORDING_ERROR` (heap message; UI deletes it)
- `WM_APP_DEVICES_CHANGED`

`RecordingSession` states: `Idle → Starting → Recording → Stopping → Idle`, plus `Error`. Invalid transitions are ignored and logged.

### M2.2 DeviceManager (capture)

**Spec:** §6.1.

`src/audio/DeviceManager`:

- Enumerate active `eCapture` endpoints and read `PKEY_Device_FriendlyName`.
- Expose the default communications capture device.
- Persist the selected endpoint id in `MicDeviceId`. If that id is missing, fall back to the default.
- `IMMNotificationClient` only posts `WM_APP_DEVICES_CHANGED`. Enumeration runs on the UI thread when that message arrives.

**Done when:** the Record tab combo lists real microphones, including a "Default communications device" entry, and the choice survives a restart.

### M2.3 Ring buffer

**Spec:** §4, §6.4, §11.

`src/audio/RingBuffer.hpp` — single-producer, single-consumer, lock-free, float frames.

- The capture thread never blocks. A failed push drops that packet and records a discontinuity the consumer can observe.
- Store enough timing to support M3: QPC of the first frame in each pushed packet, plus frame count. Stereo interleaved `float` samples.
- No allocation inside `Push` / `Pop`.

**Tests:** `tests/RingBufferTests.cpp` — SPSC correctness, wraparound, overrun drops without blocking. GoogleTest via `FetchContent`, pinned tag, `MEETINGBUDDY_BUILD_TESTS` default ON, `ctest` in the Release config. Add `scripts/test.bat` (`ctest --test-dir build -C Release`) so later milestones can run it.

### M2.4 CaptureStream (microphone)

**Spec:** §6.2.

`src/audio/CaptureStream` — one WASAPI shared-mode capture.

- `IAudioClient::Initialize` with `AUDCLNT_SHAREMODE_SHARED`, `AUDCLNT_STREAMFLAGS_EVENTCALLBACK | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY`, a 200 ms buffer, and the internal format: **48 kHz, stereo, 32-bit float** (`WAVE_FORMAT_EXTENSIBLE`, `KSDATAFORMAT_SUBTYPE_IEEE_FLOAT`).
- If `AUTOCONVERTPCM` is rejected, fail this session with a clear error. The Media Foundation resampler fallback is M4. Do not write a file in the device's native format and pretend it is 48 kHz float.
- Wait on the event with a 100 ms timeout. Drain every packet with `GetBuffer` / `ReleaseBuffer`. Push frames and `u64QPCPosition`.
- `AUDCLNT_BUFFERFLAGS_SILENT` pushes zeros. `DATA_DISCONTINUITY` is flagged for the consumer.
- Thread proc: `AvSetMmThreadCharacteristicsW(L"Audio")`, MTA COM, no logging and no heap traffic on the steady path.
- `AUDCLNT_E_DEVICE_INVALIDATED` stops the stream and posts an error. Reopen-and-continue is M4.

When `CommunicationsProcessing` is on, set `AudioClientProperties.eCategory = AudioCategory_Communications` before initialize. The setting stays off; the dialog that toggles it is M8.

### M2.5 WavWriter

**Spec:** §6.5, §11.

`src/audio/WavWriter`:

- PCM 16-bit, 48 kHz, stereo. Convert float to int16 in the writer thread (clamp to [-1, 1] before scaling).
- Filename `<folder>\MeetingBuddy_YYYY-MM-DD_HH-MM-SS.wav` in local time. Collision suffix `-2`, `-3`, …
- Write `*.wav.partial` with a RIFF header up front. Patch the RIFF and data sizes every 2 s and on close. On success, flush, close, and rename to `.wav`.
- On failure, close and leave the partial file for M4 recovery. Do not delete the user's other files.

**Tests:** `tests/WavWriterTests.cpp` — header layout, periodic size patch, a truncated partial still has a consistent header after a repair helper. That helper is what M4's startup recovery will call.

### M2.6 RecordingSession (mic only)

**Spec:** §6.6.

`src/audio/RecordingSession` owns the mic `CaptureStream`, the ring buffer, `WavWriter`, and the writer thread for one take.

- **Start:** folder exists and is writable (create the default if needed), open the mic, start capture, start the writer, then post `WM_APP_RECORDING_STARTED`. Any failure rolls back to `Idle` or `Error` and posts `WM_APP_RECORDING_ERROR`.
- **Stop:** signal capture, drain the ring, finalize and rename, post `WM_APP_RECORDING_FINISHED`.
- One session at a time.

### M2.7 Record tab

**Spec:** §8.2.

Replace the placeholder page:

- Microphone combo (M2.2).
- **Save to** path and a browse button. `IFileOpenDialog` with `FOS_PICKFOLDERS`. Reject a folder that is not writable. Default `%USERPROFILE%\Documents\MeetingBuddy`.
- A large Record button. While `Recording` it reads Stop.
- Elapsed time from a UI `WM_TIMER` (about 4 Hz is enough for the clock). The timer reads a start QPC stored when `WM_APP_RECORDING_STARTED` arrives. It does not query the capture thread.
- Status-bar panes show "Recording" and the elapsed time.
- Disable the combo and folder controls while `Starting`, `Recording`, or `Stopping`.
- Tray Start/Stop calls the same session commands. Swap in `tray_recording.ico` and set the tooltip to "Recording hh:mm:ss" while recording.
- System-audio combo and meters are not in this milestone. A missing microphone shows the error from the session and does not create a file.

### M2.8 M2 check

- `scripts\test.bat` passes.
- `scripts\start.bat` launches.
- Record about 10 seconds of microphone audio. The result is `<folder>\MeetingBuddy_….wav` (no `.partial` left behind), it plays in the default player, and the elapsed timer matches the file length within about a second.
- The window stays responsive while recording (moving and switching tabs still work).

---

## M3 — Loopback, mixer, meters

### M3.1 Loopback capture

**Spec:** §6.1, §6.2.

- Enumerate active `eRender` endpoints. Record-tab combo **System audio**, default entry "Default output device", persisted as `SystemDeviceId`, with the same fallback rule as the mic.
- Second `CaptureStream` on the selected render device with `AUDCLNT_STREAMFLAGS_LOOPBACK` added to the M2 flags. Same internal format and the same thread rules.
- Its own `RingBuffer`.

### M3.2 Silent render keep-alive

**Spec:** §6.3.

`src/audio/SilentRenderKeepAlive` opens a shared-mode render client on the **same** device as loopback and continuously submits silence, so the engine clock keeps running when nothing is playing.

- Start it before loopback capture. Stop it after capture stops.
- Zeros only. It must not change what the user hears.

### M3.3 Mixer

**Spec:** §6.4, §11.

`src/audio/Mixer` runs on the writer thread and replaces the M2 "write mic frames in order" path.

- Timeline zero is the QPC at recording start. Convert each packet's QPC to a frame index at 48 kHz.
- A gap larger than 5 ms becomes inserted silence. Overlap from a late or duplicate packet is dropped.
- Emit a 10 ms block (480 stereo frames) when both sources have covered it, or when the block is more than 150 ms behind now. The missing side is silence.
- Drift: moving average of each source's fill. If the two fills diverge by more than 20 ms, drop or duplicate a single frame per correction step.
- Mix: mic channels are summed to mono and duplicated to stereo, then `out = clamp(micGain * mic + sysGain * sys)` with defaults 1.0 and 0.8 (read from settings). Soft limiter: tanh-style attenuation above -1 dBFS.
- Publish peak levels for mic, system, and the mix in atomics.
- A source that failed to open is silence for the whole take (see M4 for the user-visible warning). M3 still requires both devices present for the happy path.

**Tests:** `tests/MixerTests.cpp` with synthetic QPC timestamps — alignment, gap fill, overlap drop, drift under a simulated 100 ppm offset, limiter not exceeding full scale. No audio hardware.

### M3.4 Session wiring

**Spec:** §6.6.

`RecordingSession` starts keep-alive, both captures, and the mixer. Stop drains both rings before the WAV is finalized. `SaveStems` stays off; when M8 turns it on, the mixer can also write `*.mic.wav` and `*.system.wav` beside the mix. Do not write stems yet.

### M3.5 Level meters

**Spec:** §8.2, §4.

`src/ui/LevelMeter` — owner-drawn peak meter on the Record page, one for Mic and one for System.
`src/audio/LevelMonitor` — idle mic + loopback capture that publishes peaks without writing to disk.

- A UI timer at about 30 Hz reads the atomics (monitor while idle, mixer while recording). The capture and mixer threads do not touch the control.
- Peaks decay smoothly so speech is visible.
- The monitor stops before recording starts and restarts when recording ends or the device selection changes.

### M3.6 M3 check

- Mixer and ring-buffer tests pass.
- Record about one minute while talking and toggling system playback on and off. Both sources are audible, speech and playback stay in sync, and the file plays in the default player.
- Meters move for the source that is actually making sound, including during a stretch of silence (loopback must not freeze the timeline).
- Inputs stay disabled while recording. The window stays responsive.

---

## M4 — Robustness

### M4.1 Device loss during a take

**Spec:** §6.1, §6.2, §9.

- On `AUDCLNT_E_DEVICE_INVALIDATED`, or a default-device change for a source that was set to "default", reopen that source on the new default. Keep the other source and the file going.
- The mixer fills the hole with silence (the 150 ms stale rule plus an explicit gap at the reopen boundary).
- Post a UI message that names the new device. Tray balloon: "Device lost — recording continues on \<new device\>".
- Remember the switch so M5 can store it in the sidecar. If the sidecar writer does not exist yet, keep a `std::vector` of notes on the session and give them to `RecordingLibrary` in M5.

### M4.2 Buffer flags and format fallback

**Spec:** §6.2, §13.

- Confirm silent packets and discontinuities already take the M2/M3 paths; add a test or a logged session note where the code cannot be unit-tested without hardware.
- When `AUTOCONVERTPCM` fails, initialize with `GetMixFormat()` and convert to 48 kHz stereo float with the Media Foundation Audio Resampler DSP (`CLSID_CResamplerMediaObject`) **before** the ring push. The ring format does not change. Link `mfplat` / `mfuuid` when this lands.
- Resampler setup happens at stream start, not per packet on a fresh allocation.

### M4.3 Missing devices, exclusive mode, privacy

**Spec:** §9, §8.2.

- No capture device: allow a system-only take and show a warning on the Record tab. No render device: mic-only, with a warning. Refuse to start only when both are unavailable.
- Exclusive-mode failure on one device: report which device, and continue with the other if it opened.
- `E_ACCESSDENIED` from mic `Activate` or `Initialize`: tell the user the microphone privacy setting is off, with a button that opens `ms-settings:privacy-microphone`. Do not start the file.

### M4.4 Disk space and a vanished folder

**Spec:** §6.5, §9.

- `GetDiskFreeSpaceExW` before start and every 30 s from the writer thread (not the capture thread). Below 200 MB, stop gracefully, finalize the WAV, and balloon "Recording stopped: low disk space".
- If the folder disappears or the write fails mid-take, stop, write what was captured under `%LOCALAPPDATA%\MeetingBuddy\rescue\`, and tell the user the path.
- Start already fails with a clear message when the chosen folder is missing or not writable.

### M4.5 Crash recovery

**Spec:** §6.5.

- On startup, scan the recordings folder and the rescue folder for `*.wav.partial`.
- Repair the RIFF sizes with the M2 helper and rename to `*.recovered.wav`.
- A partial that is too short to contain a header is logged and left alone.

**Done when:** killing the process during a recording, then launching again, produces a playable `*.recovered.wav`.

### M4.6 Exit and session end

**Spec:** §6.6.

- Quit from the menu, tray, or a close that is actually exiting, while a take is active: "Stop and save recording?" Yes / No / Cancel. Yes stops and saves, then exits. No exits without saving (delete the partial). Cancel returns to the app and keeps recording.
- `WM_QUERYENDSESSION` / `WM_ENDSESSION` always stop and save. Do not show a dialog on the end-session path.

### M4.7 M4 check

- Unplug the mic, or switch the Windows default device, during a recording: the file continues, the gap is silence, and the balloon names the new device.
- Fill or fake the low-disk condition and confirm a clean stop.
- Privacy-denied path opens the Settings URI (the button can be exercised with the privacy toggle off).
- Exit during recording follows Yes / No / Cancel as specified.

---

## M5 — Recordings tab

### M5.1 Sidecar JSON

**Spec:** §6.5.

Write `<name>.json` next to the WAV when a take finalizes (including recovered files, as far as the facts are known).

```json
{
  "startLocal": "2026-11-01T13:18:00",
  "endLocal": "2026-11-01T14:00:17",
  "durationSeconds": 2537,
  "sampleRate": 48000,
  "channels": 2,
  "micDevice": "…",
  "systemDevice": "…",
  "deviceNotes": [],
  "transcriptStatus": "none",
  "transcriptError": "",
  "model": "",
  "language": ""
}
```

`transcriptStatus` is one of `none`, `queued`, `preparing`, `transcribing`, `ready`, `failed`, `cancelled`. UTF-8, no BOM. Update the file in place when transcription state changes (M6/M7).

### M5.2 RecordingLibrary

**Spec:** §8.3, §11.

`src/storage/RecordingLibrary`:

- Scan the recordings folder for `*.wav` and `*.m4a`. Pair each with the `.md` and `.json` that share its stem.
- Sort newest first by the file's local start time (sidecar, then filename timestamp, then file time).
- Duration and size come from the sidecar when present, otherwise from the WAV header and `GetFileSizeEx`.
- Rename and delete treat audio + `.md` + `.json` as one unit. Delete uses `IFileOperation` to send all of them to the Recycle Bin.
- Run the M4 partial-file repair before the scan.

**Tests:** `tests/RecordingLibraryTests.cpp` on a temp directory — pairing, sort order, missing sidecar, partial recovery. No UI.

### M5.3 List view

**Spec:** §8.3.

Replace the Recordings placeholder with a report-mode `CListViewCtrl`: full-row select, columns **Name | Date | Duration | Size | Transcript**.

- Transcript cell shows `—` until M7 drives live states. `ready` may already display Ready if a `.md` exists.
- Default sort is newest first. Clicking a column header sorts by that column.
- Empty folder shows an empty list, not an error.

### M5.4 Actions

**Spec:** §8.3.

Toolbar (or a button row) and a right-click menu, both calling the same handlers:

| Action | Behaviour in M5 |
|---|---|
| Play | `ShellExecute` the audio with the default player |
| Open folder | Explorer with the file selected |
| Rename | Prompt for a new stem; rename audio, `.md`, and `.json` together |
| Delete | Confirm, then Recycle Bin for all three |
| Refresh | Rescan |
| Transcribe | Disabled until M6 |
| View transcript | Disabled until a `.md` exists; M7 does the window |

Double-click plays. (M7 changes double-click to open the transcript when a `.md` exists.)

### M5.5 Directory watcher

**Spec:** §8.3.

A background thread blocks in `ReadDirectoryChangesW` on the recordings folder. Coalesce bursts (about 200 ms) and post one "library changed" message. The UI thread rescans. Handle overflow by doing a full rescan. Do not touch the list view from the watcher thread.

Restart the watch when the user changes the recordings folder (the folder setting already exists; the UI that edits it is still M8, but the library must follow `Settings` whenever it changes).

### M5.6 M5 check

- A new recording appears at the top of the list without a manual refresh, including when the file is copied in from Explorer.
- Play, rename, and delete keep the three sidecar names in lockstep. Delete can be restored from the Recycle Bin.
- Library tests pass. `scripts\start.bat` still launches.

---

## M6 — Transcription engine

No UI polish in this milestone beyond enough of a control to enqueue a job and see progress. The transcript window is M7.

### M6.1 whisper.cpp in the build

**Spec:** §7.1, §10.

- `FetchContent` a pinned whisper.cpp tag (record the tag in `CMakeLists.txt`). Reconfigure so the pin is picked up.
- Options: `WHISPER_BUILD_EXAMPLES=OFF`, `WHISPER_BUILD_TESTS=OFF`, `BUILD_SHARED_LIBS=OFF`, CPU only, `GGML_NATIVE=OFF`, AVX2 on so the binary stays portable. Do not enable Vulkan or CUDA.
- Link the static library into `MeetingBuddy`. Confirm `LANGUAGES C` is what ggml compiles as.

**Done when:** Release links and the exe still starts. A few extra MB in the binary is expected.

### M6.2 AudioFileReader

**Spec:** §7.2.

`src/transcription/AudioFileReader` decodes a recording to 48 kHz stereo float.

- WAV: read the file the app writes (and a standard PCM WAV).
- M4A: `IMFSourceReader` (`mfreadwrite`). M4A files are optional until M8, but the reader can land now.
- Report duration. Do not resample to 16 kHz here.

### M6.3 Resampler

**Spec:** §7.2, §11.

`src/transcription/Resampler`: average stereo to mono, then 48 kHz → 16 kHz with a windowed-sinc FIR low-pass and decimation by 3. Output `std::vector<float>`.

**Tests:** a 1 kHz sine keeps frequency and amplitude; a 10 kHz tone is attenuated. No whisper dependency.

### M6.4 MarkdownFormatter

**Spec:** §7.4, §11.

`src/transcription/MarkdownFormatter` takes segments `{t0, t1, text}` in whisper's 10 ms units plus the recording metadata, and writes UTF-8 **without** BOM:

- Title `# Meeting — <local date time>`.
- Bullets for duration, recorded range, and model (language: configured value, and the detected language when whisper reports one).
- `## Transcript`, then paragraphs prefixed with `**[hh:mm:ss]**`.
- Merge adjacent short segments. Start a new paragraph after a pause over 1.5 s, or at about 60 s of continuous speech.
- Trim whitespace. Drop a line repeated 3 or more times in a row.

**Tests:** merging, timestamp format, de-duplication, no BOM.

### M6.5 WhisperEngine

**Spec:** §7.1.

`src/transcription/WhisperEngine` is the only file that includes whisper headers.

- `whisper_init_from_file_with_params`. Cache the loaded model. Unload it after 5 minutes with no job.
- `whisper_full_default_params(WHISPER_SAMPLING_GREEDY)`: `translate = false`, `print_* = false`, `language` from settings (`auto` or a code), `n_threads = max(1, hw_concurrency - 1)` unless `Threads` is non-zero.
- `progress_callback` and `abort_callback` (the abort callback only reads an atomic cancel flag).
- Collect `whisper_full_n_segments`, text, and `t0`/`t1`.
- If `UseVad` is on and the Silero model file is present, enable whisper's VAD. If the file is absent, log and run without VAD. Do not download it silently.

### M6.6 ModelManager

**Spec:** §7.3.

`src/transcription/ModelManager` looks in `%LOCALAPPDATA%\MeetingBuddy\models\` for `ggml-*.bin`.

- The offered set is multilingual and `.en` variants of tiny, base, and small, including `ggml-base-q5_1.bin` (the default) and `ggml-small-q5_1.bin`.
- When the user asks to transcribe and the chosen file is missing, a dialog offers **Download** or **Browse…**. Browse copies or records a path to a local `.bin` under the models directory.
- Download with WinHTTP from `https://huggingface.co/ggerganov/whisper.cpp/resolve/main/<file>`. Show progress, support resume, and check SHA-256 against a table of official digests recorded at implementation time (do not invent hashes). A mismatch deletes the partial download and reports failure.
- This is the only network call in the app. Do not check for updates, and do not phone home.

### M6.7 TranscriptionService

**Spec:** §7.2, §7.5.

`src/transcription/TranscriptionService` — one worker thread, FIFO queue, one job at a time. States: `Queued → Preparing → Transcribing (n%) → Done | Failed | Cancelled`.

- Preparing: decode, resample, and for audio longer than 1 hour split into 30-minute chunks with a short overlap. Offset segment timestamps by the chunk start.
- Done: write `<name>.md` via the formatter and set the sidecar to `ready` with the model and language. The audio file is never modified or deleted here.
- Failed: sidecar `failed` plus `transcriptError`. Leave any existing `.md` untouched if the new run did not finish.
- Cancel: abort flag, sidecar `cancelled`.
- Post `WM_APP_TRANSCRIBE_QUEUED`, `WM_APP_TRANSCRIBE_PROGRESS` (percent), `WM_APP_TRANSCRIBE_FINISHED`, `WM_APP_TRANSCRIBE_FAILED`. Add the ids in `Ids.hpp`.
- Status-bar transcription pane shows the percent for the active job.
- Enable **Transcribe** on the Recordings tab. It enqueues the selected file. A second click while that file is queued or running requests cancel.
- Do **not** auto-start a job when recording stops. `AutoTranscribe` stays off and is honored once M7/M8 wire the setting (and M8 the checkbox). If the setting is already on, M6 may enqueue at stop so the worker is testable.

### M6.8 M6 check

- Formatter, resampler, and existing tests pass.
- With a downloaded or browsed model, transcribing a short recording produces a `.md` beside the WAV, the sidecar moves through the states, and cancelling a longer job stops the worker.
- Airplane mode (or no network) still transcribes when the model file is already local. No other HTTP requests appear.
- The UI stays responsive during a transcription (record tab and window move still work).

---

## M7 — Transcript UI

### M7.1 Transcript window

**Spec:** §8.4.

`src/ui/TranscriptWindow` — top-level frame, one per open file. If that path is already open, bring the existing window forward.

- Read-only RichEdit 4.1 (`MSFTEDIT_CLASS`): `ES_MULTILINE | ES_READONLY | WS_VSCROLL`, `EM_AUTOURLDETECT`. Load `Msftedit.dll`.
- Show the Markdown **source**.
- Light `CHARFORMAT2` styling: `#` headings larger and bold, `**[hh:mm:ss]**` timestamps grey.
- Copy stays plain Markdown. `WM_COPY`, Ctrl+C, and Copy all put `CF_UNICODETEXT` on the clipboard and do not leave RTF as the text the user pastes. `src/util/Clipboard` can own that helper.

### M7.2 Toolbar

**Spec:** §8.4.

- **Copy all** — entire file as `CF_UNICODETEXT`.
- **Save as…** — another `.md` path, UTF-8 without BOM.
- **Open in editor** — `ShellExecute` on the `.md`.
- **Find** — `Ctrl+F`, `FINDMSGSTRING` dialog, search inside the control.
- Ctrl+A / Ctrl+C work. Status bar shows the word count.

### M7.3 Auto-transcribe after recording

**Spec:** §7.5, §8.2.

When `WM_APP_RECORDING_FINISHED` arrives and `AutoTranscribe` is on, enqueue the new file immediately. Otherwise leave transcription to the Recordings tab (or a later manual action). There is no post-stop prompt on the Record tab.

### M7.4 List, balloon, and double-click

**Spec:** §7.5, §8.3, §8.5.

- Transcript column follows sidecar state: `—`, `Queued`, `42 %`, `Ready`, `Failed`. Failed shows `transcriptError` as the item tooltip.
- **View transcript** opens `TranscriptWindow`. Double-click opens it when a `.md` exists; otherwise it plays, as in M5.
- On success, if the main window is visible, open the transcript window. Always show a tray balloon "Transcript ready — click to open" that activates that window.
- Device-switch notes from M4 are written into `deviceNotes` when the sidecar is saved.

### M7.5 M7 check

- The Recordings action and auto-transcribe (flip the ini key by hand until M8) produce the same Markdown and the same column states.
- Copy all and a selection both paste as Markdown source, including the `#` heading and `**[timestamp]**` markers.
- Find, Save as, and Open in editor work. A completion balloon opens the transcript.
- A failed job (bad model path) sets Failed, shows the reason, and does not alter the WAV.

---

## M8 — Settings and polish

### M8.1 Settings dialog

**Spec:** §8.7.

`src/ui/SettingsDialog` replaces the File → Settings and tray stubs.

- **Recording:** folder, default devices, mic gain, system gain, save separate stems, compress to M4A, keep WAV after compression.
- **Transcription:** model (from `ModelManager`, plus Download / Browse), language, auto-transcribe, thread count, use VAD.
- **General:** close to tray, start with Windows, start minimized, hotkey enable and the chord (default Ctrl+Alt+R).
- OK writes `settings.ini`. Cancel leaves the previous values. Applying a new recordings folder restarts the M5 watcher and refreshes the list.

Wire the gains into the mixer at the start of the next take (a take already in progress keeps the gains it started with).

### M8.2 Stems and communications processing

**Spec:** §6.2, §6.4.

- `SaveStems` writes `*.mic.wav` and `*.system.wav` next to the mix, same partial-and-rename protocol. Delete, rename, and the library treat them as part of the recording's set only if that stays simple; at minimum do not orphan them on delete of the mix. Prefer deleting and renaming them with the main file.
- `CommunicationsProcessing` was hooked in M2. The checkbox now toggles it. Document in the dialog that headphones avoid echo and that this flag only asks the driver for communications processing.

### M8.3 Global hotkey

**Spec:** §8.2.

`RegisterHotKey` the configured chord (default Ctrl+Alt+R) while the app runs. It toggles Record/Stop through the same path as the button. If registration fails, report it once and leave recording available from the window and tray. `HotkeyEnabled` off unregisters it.

### M8.4 Start with Windows

**Spec:** §8.5.

- `StartWithWindows` creates or removes the HKCU `Run` value for this exe.
- `StartMinimized`, or a `--tray` argument, starts hidden with only the tray icon. A second instance still activates the existing window instead of starting another process.

### M8.5 DPI, About, and access keys

**Spec:** §8.1, §8, §10.

- On `WM_DPICHANGED`, apply the suggested rect and recreate or scale fonts and the level-meter layout so controls are not clipped at 150% and 200%.
- **Help → About** shows the app name, version, and a one-line privacy note: transcription is local, and the only network use is a model download the user starts.
- Tab order and mnemonics on the Record page, Recordings actions, settings dialog, and transcript toolbar.

### M8.6 Optional M4A

**Spec:** §6.5.

`src/audio/M4aEncoder` — after the WAV is finalized, if `CompressToM4a` is on, encode AAC 128 kbps with `IMFSinkWriter` on a worker. On success, if `KeepWav` is off, delete the WAV and point the library at the `.m4a`. On failure, keep the WAV and surface the error. The sidecar stays beside whichever audio file remains. Do not block the UI thread on the encode.

### M8.7 M8 check

- Every settings field round-trips through the dialog and affects the next recording or transcription as specified.
- Ctrl+Alt+R starts and stops a take while another app has focus.
- With "start with Windows" on, the Run key points at this exe; turning it off removes the value.
- At 150% DPI the Record tab is usable.
- An M4A take plays in the default player, and the recordings list shows it.

---

## M9 — Release

### M9.1 README

**Spec:** §10, §11.

A short `README.md` at the repo root:

- What the app does.
- Requirements: 64-bit Windows 10 1903+, Visual Studio with Desktop C++ and C++ ATL, CMake ≥ 3.24.
- Build and run: `scripts\start.bat`. Tests: `scripts\test.bat`.
- Privacy: local transcription, no telemetry, network only for a model download the user requests.
- Models are not bundled.
- Headphones are the reliable way to avoid speaker/mic echo.

### M9.2 Portable zip

**Spec:** §10.

A script (for example `scripts/package.bat`) builds Release and zips `MeetingBuddy.exe` plus any non-system DLLs that are actually required, and the README. Do not put models in the zip. An MSIX or WiX installer is out of scope unless packaging turns out to need it; the zip is the v1 artifact.

### M9.3 Manual test pass

**Spec:** §11.

Run the integration list and fix defects before calling M9 done:

- Record about 10 minutes with playback toggling on and off and with speech. Check sync (a clap near the mic against a click track is enough).
- Unplug a USB headset mid-take, and change the Windows default device mid-take.
- Hide to tray, restart Explorer, confirm the icon returns.
- Transcribe an English sample and one non-English sample with tiny or base. Note the real-time factor in the README or the log, whichever is less invasive. Do not add a new network call to record it.
- A recording longer than one hour is optional if time is short; if skipped, say so. The chunking path still needs a unit or integration check with a synthetic long buffer if no two-hour take is recorded.
- Low disk, privacy denied, exit-while-recording, and crash recovery already have M4 checks; repeat any that the later milestones could have regressed.

### M9.4 M9 check

- `scripts\test.bat` passes.
- `scripts\start.bat` produces a warning-clean Release build and a running app.
- The portable zip launches on a machine that has the Visual C++ runtime the build was linked against, and it does not write outside the recordings folder and the app-data paths.

---

## Spec coverage

| Design | Where it is implemented |
|---|---|
| §6.1 Device selection | M2 mic, M3 system, M4 notifications |
| §6.2 CaptureStream | M2, M4 invalidation and resampler fallback |
| §6.3 Silent keep-alive | M3 |
| §6.4 Mixer, drift, limiter, meters | M3; stems flag honored in M8 |
| §6.5 Files | M2 WAV, M4 disk and recovery, M5 JSON, M8 M4A |
| §6.6 Session lifecycle | M2, M4 exit and end-session |
| §7.1–§7.3 Engine, resample, models | M6 |
| §7.4 Markdown | M6 formatter, M7 viewer |
| §7.5 Queue, balloon | M6 queue, M7 balloon |
| §8.1 Main window | M1, M8 DPI and About |
| §8.2 Record tab | M2, M3 meters, M8 hotkey |
| §8.3 Recordings tab | M5, M7 transcript column and double-click |
| §8.4 Transcript window | M7 |
| §8.5 Tray | M1, M4 balloons, M7 transcript balloon, M8 startup |
| §8.6 Single instance | M0 |
| §8.7 Settings dialog | M8 (storage in M0) |
| §9 Edge cases | M4 |
| §10 Build and packaging | M0, M6, M9 |
| §11 Tests | With the module they cover; full manual pass in M9 |
