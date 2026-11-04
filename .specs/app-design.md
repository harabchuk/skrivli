# Skrivli — Implementation Plan

Windows desktop app (C++ / WTL) that records microphone + system audio output via WASAPI,
mixes them into a single file in a user-chosen folder, and optionally produces a plain-text
transcript using a local lightweight speech-to-text model. Lives in the system tray.

---

## 1. Goals and non-goals

### Goals
- Record **microphone** (WASAPI capture) and **audio output** (WASAPI loopback) at the same time.
- **Mix** both streams into one time-aligned audio file.
- Save recordings into a **user-defined folder** (persisted setting).
- After a recording finishes, **optionally** transcribe it with a **local** model (no cloud).
- Show the transcript as **plain text** in the Recordings detail pane (selectable/copyable).
- **Tray** integration: minimize/close to tray, tray menu, start/stop recording from tray.
- Main window with **tabs**:
  1. **Record** — record button and live status.
  2. **Recordings** — list of recordings and their transcripts.

### Non-goals (v1)
- Speaker diarization (who said what) — see "Future work".
- Video/screen recording.
- Cloud sync, accounts, telemetry.
- Real-time (live) transcription while recording.

---

## 2. Current state of the repo

- `CMakeLists.txt` — C++17, fetches WTL from `Win32-WTL/WTL@main`, builds `Skrivli.exe` (WIN32 subsystem, `main()` entry).
- `src/main.cpp`, `src/Application.*` — `CAppModule` + message loop, creates `MainWindow`.
- `src/MainWindow.*` — `CFrameWindowImpl` with a single centred Record/Stop toggle button (no logic).
- `src/Window.hpp` — leftover duplicate of `MainWindow` (unused) → **delete**.
- `src/Ids.hpp` — control/resource IDs. There is **no `.rc` file yet** (`IDR_MAINFRAME` refers to nothing).
- `scripts/start.bat` — finds CMake, kills a running instance, builds Release, launches.

Housekeeping before feature work:
- Pin WTL to a fixed tag/commit instead of `main`.
- Move to **C++20** (`std::jthread`, `std::span`, designated initializers).
- Add `C` to `project(... LANGUAGES C CXX)` (needed by whisper.cpp / ggml).
- Add a resource script + application manifest (Common Controls v6, PerMonitorV2 DPI awareness).

---

## 3. Technology choices

| Concern | Choice | Reason |
|---|---|---|
| UI | WTL 10 + ATL (Win32), Common Controls v6 | Native, tiny, already in use |
| Audio capture | WASAPI shared mode, event-driven (`IAudioClient` / `IAudioCaptureClient`) | Needed for both mic and loopback |
| System audio | `AUDCLNT_STREAMFLAGS_LOOPBACK` on the default render endpoint | Standard way to capture "what you hear" |
| Format conversion | `AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM \| SRC_DEFAULT_QUALITY` → 48 kHz float | Lets Windows resample both devices to one internal format; fallback: miniaudio data converter |
| Container | WAV (PCM 16-bit, 48 kHz, stereo) while recording; optional post-encode to M4A/AAC via Media Foundation `IMFSinkWriter` | WAV is simple and crash-tolerant; M4A is ~10× smaller |
| Transcription | **whisper.cpp** (ggml, CPU), statically linked via `FetchContent` | Local, MIT, fast on CPU, quantized models |
| Default model | `ggml-base-q5_1.bin` (~57 MB); offer `tiny`, `base`, `small` (q5) | Good speed/quality balance on laptops |
| Transcript view | Read-only multiline edit in the Recordings detail pane | Native, selectable/copyable plain text |
| Settings | `%APPDATA%\Skrivli\settings.json` (Glaze) | Simple, human-editable |
| Tests | GoogleTest (or Catch2) via `FetchContent`, `ctest` | Unit-test DSP, file, and formatting logic |

---

## 4. Architecture

```
                 ┌──────────────────────────── UI thread (WTL) ───────────────────────────┐
                 │ MainWindow (frame)                                                     │
                 │  ├─ CTabCtrl                                                           │
                 │  │   ├─ RecordView      (button, timer, level meters, device pickers)  │
                 │  │   └─ RecordingsView  (list + detail pane: player / transcript)     │
                 │  └─ TrayIcon            (Shell_NotifyIcon, context menu)               │
                 └───────▲───────────────────────────────▲────────────────────────────────┘
                         │ PostMessage(WM_APP_*)         │ PostMessage(WM_APP_TRANSCRIBE_*)
┌────────────────────────┴───────────────┐     ┌─────────┴─────────────────────────┐
│ RecordingSession                        │     │ TranscriptionService (worker)     │
│  ├─ CaptureStream (mic)     ── thread ─┐│     │  ├─ AudioFileReader → 16k mono    │
│  ├─ CaptureStream (loopback)── thread ─┤│     │  ├─ WhisperEngine (whisper.cpp)   │
│  ├─ SilentRenderKeepAlive              ││     │  └─ TranscriptText → .txt         │
│  ├─ Mixer + drift control  ── thread ◄─┘│     └───────────────────────────────────┘
│  └─ WavWriter (→ optional M4A encode)   │
└─────────────────────────────────────────┘
          ▲
          │ IMMNotificationClient (device add/remove/default changed)
     DeviceManager
```

### Threads
- **UI thread** — all windows; never blocks on audio or model work.
- **Capture threads (2)** — one per WASAPI stream, event-driven, registered with MMCSS (`AvSetMmThreadCharacteristicsW(L"Audio")`). Only copy packets into lock-free ring buffers.
- **Mixer/writer thread** — pulls 10 ms blocks from both ring buffers, mixes, computes peak levels, writes to disk.
- **Transcription worker** — one job at a time (queue), cancellable.

UI ↔ worker communication is via `PostMessage` with `WM_APP + n` messages carrying heap payloads (or IDs looked up in a thread-safe store). Level meters are read by a UI `WM_TIMER` (≈30 Hz) from atomics.

---

## 5. Source layout

Placement rules (what belongs in each folder, and which layers may include which) live in `AGENTS.md`.

```
src/
  main.cpp, pch.hpp, Ids.hpp, Version.hpp
  app/
    Application.{hpp,cpp}                  # module, message loop, single-instance mutex
  core/
    AudioFormat.hpp, RingBuffer.hpp        # format constants; SPSC lock-free float ring
    ChunkPlan.hpp
    TranscriptText.{hpp,cpp}
    Recording.hpp                          # RecordingEntry, RecordingSidecar fields
    Transcript.hpp                         # TranscriptSegment, TranscriptMeta, TranscriptStatus
  services/
    RecordingSession.{hpp,cpp}             # one take: start, mix, write, stop
    LevelMonitor.{hpp,cpp}                 # idle mic/system peak metering (no disk I/O)
    TranscriptionService.{hpp,cpp}         # FIFO queue, one job at a time
  infrastructure/
    audio/    DeviceManager, CaptureStream, SilentRenderKeepAlive, Mixer,
              AudioResampler, MaConverter, MiniAudio.hpp, WavHeader, Resampler
    ai/       WhisperEngine, ModelManager  # whisper.cpp, model catalog, download (no HWND)
    storage/  Settings, RecordingLibrary, RecordingSidecar, DirectoryWatcher
    files/    WavWriter, M4aEncoder, AudioFileReader, AudioPlayer
    system/   Autostart, Hotkey, Clipboard
    common/   wil::com_ptr_nothrow, Log, HResult, Paths, Text, FileIo, Scoped.hpp, PostOwned.hpp
  ui/                                      # reusable controls, no screen knowledge
    TabCtrl, DeviceCombo, LevelMeter, RecordButton, TrayIcon, ShellDialogs, Theme.hpp
  views/                                   # windows, tab pages, dialogs
    MainWindow, RecordView, RecordingsView, SettingsDialog
    ModelPrompt                            # progress window and model file picker
resources/
  Skrivli.rc, resource.h
  app.ico, tray_idle.ico, tray_recording.ico
  Skrivli.manifest
```

GoogleTest sources sit in a `tests/` folder beside the type they cover (`src/core/tests/`, `src/infrastructure/audio/tests/`, `src/infrastructure/files/tests/`, `src/infrastructure/storage/tests/`, `src/infrastructure/common/tests/`). They are compiled only into `SkrivliTests`.

---

## 6. Audio recording design

### 6.1 Device selection
- `DeviceManager` enumerates `eCapture` and `eRender` endpoints (`IMMDeviceEnumerator::EnumAudioEndpoints`, `DEVICE_STATE_ACTIVE`) and exposes friendly names (`PKEY_Device_FriendlyName`).
- Record tab has two combos: **Microphone** (default: "Default communications device", which follows the Windows default) and **System audio** (default: "Default output device").
- Selected device IDs persist in settings; if a saved device is missing, fall back to the default.
- `IMMNotificationClient` handles `OnDefaultDeviceChanged` / `OnDeviceStateChanged`. The callback only posts a message; the real work happens on the UI or session thread.

### 6.2 CaptureStream
- `IAudioClient::Initialize(AUDCLNT_SHAREMODE_SHARED, EVENTCALLBACK | AUTOCONVERTPCM | SRC_DEFAULT_QUALITY [| LOOPBACK], 200 ms buffer, ..., &internalFormat)`, where `internalFormat` = **48 kHz, 2 ch, 32-bit float** (`WAVE_FORMAT_EXTENSIBLE`, `KSDATAFORMAT_SUBTYPE_IEEE_FLOAT`).
  - If `AUTOCONVERTPCM` is rejected (older drivers/OS), initialize with `GetMixFormat()` and convert to 48 kHz stereo float with a miniaudio data converter inside the capture thread.
- The loop waits on the event (with a 100 ms timeout), drains all packets via `GetBuffer` / `ReleaseBuffer`, and pushes frames + `u64QPCPosition` into the ring buffer.
- `AUDCLNT_BUFFERFLAGS_SILENT` → push zeros. `AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY` → log it and let the mixer resync on the QPC timestamp.
- `AUDCLNT_E_DEVICE_INVALIDATED` → stop the stream, notify the session, and reopen on the new default device. The gap is filled with silence, so the recording keeps going.
- Mic stream: optionally set `AudioClientProperties.eCategory = AudioCategory_Communications` so drivers that support it apply system echo cancellation / noise suppression (a user setting).

### 6.3 Loopback "no data during silence" problem
WASAPI loopback delivers **no packets** while nothing is playing, which would make the streams drift out of sync.
- **SilentRenderKeepAlive**: open a shared-mode render stream on the same output device and continuously feed it zeros. This keeps the engine clock running, so loopback always produces packets.
- Belt and braces: the mixer also fills gaps based on QPC timestamps (see 6.4).

### 6.4 Mixer and synchronization
- Internal timeline: frames at 48 kHz from `t0` = the QPC at recording start.
- Each source ring buffer stores `(qpcOfFirstFrame, frames)`. The mixer converts QPC to a timeline frame index. A gap larger than about 5 ms becomes inserted silence. Overlap from a duplicate or late packet is dropped.
- The mixer emits 10 ms blocks (480 frames) once both sources have covered the block, **or** once the block is more than 150 ms behind "now" (the missing source is treated as silence). That bounds latency and survives a stalled device.
- **Drift control**: mic and output devices run on different crystals. Track each buffer's fill level with a moving average. If it diverges by more than 20 ms, drop or duplicate one frame per correction step, spread out so it's inaudible for speech. A tiny-ratio resampler is a later improvement.
- **Mixdown**: `out = clamp(micGain * mic + sysGain * sys)`. Mic is mono-summed (miniaudio channel converter) and duplicated to both channels; the sum uses `ma_mix_pcm_frames_f32`. Default gains: mic 1.0, system 0.8. A simple soft limiter (tanh above -1 dBFS) prevents clipping.
- **Mic AGC** (optional, off by default): when enabled, replace the fixed mic gain with app-side automatic gain control in the mixer. Measure mic RMS over ~0.75 s, target speech around −18 dBFS, raise gain slowly and cut quickly, clamp to 0.5×–4×, ignore levels below −45 dBFS (noise gate), and interpolate gain across each 10 ms block to avoid pumping. This adjusts recording volume only — not the Windows mic input level. Driver “communications processing” remains a separate, inconsistent option.
- Peak levels per source (and mixed) go into atomics for the UI meters. The effective mic gain (manual or AGC) is also published for the Record-tab readout.
- Optional setting (off by default): also save `*.mic.wav` / `*.system.wav` stems. Useful later for diarization ("You" vs. "Others").

### 6.5 Writing files
- Filename: `<folder>\Skrivli_2026-11-01_13-18-00.wav` (local time; collisions get a `-2` suffix).
- While recording, write to `*.wav.partial`. `WavWriter` uses the miniaudio WAV encoder (float in, PCM 16-bit out; MiniAudio owns the WAV header). Size fields are rewritten every 2 s so partials stay playable; `ma_encoder_uninit` patches them again on close. On stop, flush, close, and rename to `.wav`.
- On startup, `RecordingLibrary` finds leftover `.partial` files (from a crash), patches RIFF/`data` sizes from file length, and renames them to `*.recovered.wav`.
- Check free disk space before start and every 30 s (`GetDiskFreeSpaceExW`); stop gracefully below 200 MB and notify.
- Optional post-step: "Compress to M4A (AAC 128 kbps)" via `IMFSinkWriter` on a worker, then delete the WAV if the user opts in.
- Metadata sidecar `<name>.json`: start/end time, duration, devices used, sample rate, transcript status, model used.

### 6.6 Session lifecycle (state machine)
`Idle → Starting → Recording → Stopping → Idle` (plus `Error`).
- Start: validate folder, open devices, start keep-alive, start captures, start the mixer, then post `WM_APP_RECORDING_STARTED`.
- Stop: signal captures to stop, drain the ring buffers, finalize the file, then post `WM_APP_RECORDING_FINISHED(path)`.
- Exit while recording: ask "Stop and save recording?" (Yes / No / Cancel). Ending the Windows session (`WM_QUERYENDSESSION`/`WM_ENDSESSION`) always stops and saves.

---

## 7. Transcription design

### 7.1 Engine
- whisper.cpp via `FetchContent` (pinned tag). Options: `WHISPER_BUILD_EXAMPLES=OFF`, `WHISPER_BUILD_TESTS=OFF`, `BUILD_SHARED_LIBS=OFF`, CPU backend only by default (`GGML_NATIVE=OFF` + AVX2 for portable binaries). Vulkan/CUDA could be added later as opt-in build flags.
- `WhisperEngine` wraps:
  - `whisper_init_from_file_with_params` (load the model once and keep it cached while the app runs; unload after 5 min idle).
  - `whisper_full_default_params(WHISPER_SAMPLING_GREEDY)`: `language = "auto"` (or user setting), `n_threads = max(1, hw_concurrency - 1)`, `translate = false`, `print_* = false`.
  - `progress_callback` → post a percentage to the UI; `abort_callback` → checks a cancel flag.
  - Read segments: `whisper_full_n_segments`, `whisper_full_get_segment_text`, `whisper_full_get_segment_t0/t1` (in 10 ms units).
  - Optionally enable whisper.cpp's built-in VAD (Silero model) to skip silence and reduce hallucinations on long quiet stretches.

### 7.2 Input preparation
- `AudioFileReader` decodes WAV (and FLAC/MP3) with miniaudio to 48 kHz stereo float. M4A/AAC still goes through `IMFSourceReader`, then a miniaudio data converter when the decoded rate or channel count is not already 48 kHz stereo.
- `Resampler`: stereo to mono (average), then 48 kHz to **16 kHz** with miniaudio's Butterworth low-pass (order 8) and linear resampler. Output is `std::vector<float>`.
- For long recordings (over 1 h), stream in 30-minute chunks with a small overlap to keep memory bounded, and offset the timestamps.

### 7.3 Models
- `ModelManager` looks in `%LOCALAPPDATA%\Skrivli\models\` for `ggml-*.bin`.
- If none is present when the user first transcribes, a dialog offers to download the default model from `https://huggingface.co/ggerganov/whisper.cpp/resolve/main/<file>` (WinHTTP, progress, resumable, SHA-256 check against a built-in table), **or** to browse for a local `.bin` file.
- Settings let the user pick a model (tiny / base / small, multilingual or `.en`) and a language.
- The app makes no network calls except this explicit, user-initiated download.

### 7.4 Plain-text transcript
Saved next to the audio as `<name>.txt`:

```text
[00:00:03] Okay, let's get started with the weekly sync.

[00:00:09] First item is the release schedule...
```

- Adjacent short segments get merged into paragraphs (a new paragraph after a pause over 1.5 s or every ~60 s) for readability.
- Whitespace is trimmed, and repeated hallucinated lines (e.g. identical text repeated 3+ times) are removed.
- The file is UTF-8 without BOM.
- Legacy `<name>.md` files are still detected and shown with Markdown markers stripped for display.

### 7.5 TranscriptionService
- FIFO job queue; one job runs at a time. Job states: `Queued → Preparing → Transcribing (n%) → Done | Failed | Cancelled`.
- The state is reflected in the Recordings list and in the sidecar JSON.
- When a job finishes, show a tray balloon ("Transcript ready — click to open"). Clicking the balloon (or finishing while the main window is visible) switches to the Recordings tab and selects that recording.
- **Optional by design**: nothing transcribes automatically unless the user enables "Transcribe automatically after recording" in settings (off by default). Otherwise the user starts transcription from the Recordings tab.

---

## 8. UI design

### 8.1 Main window
- Resizable `CFrameWindowImpl` (allow resizing and maximizing, unlike the current fixed style). Minimum size about 560×420; position and size are saved.
- Menu bar: **File** (Open recordings folder, Settings…, Exit), **Help** (About).
- Client area: a `CTabCtrl` (Record / Recordings) centered in the strip above the pages. The selected tab is white and open along its bottom. A hairline runs the width of the strip and breaks under the selected tab so that tab joins the page. Each tab page is a child `CDialogImpl` (resource dialog, `DS_CONTROL | WS_CHILD`) so layout uses `CDialogResize`. Switching tabs shows/hides the pages, which fill the width below the strip.
- Status bar (`CMultiPaneStatusBarCtrl`): recording state, elapsed time, transcription progress.
- Per-monitor DPI: handle `WM_DPICHANGED`, and scale fonts and the meter control.

### 8.2 Tab 1 — Record
```
┌──────────────────────────────────────────────┐
│  Input  ─────────────────────────────────    │
│  Microphone   [ Default communications ▼ ]   │
│  System audio [ Default output device  ▼ ]   │
│  Output ─────────────────────────────────    │
│  Save to      [ C:\Users\…\Skrivli ] [Browse…] │
│                                              │
│              (  ● Start recording  )         │   owner-drawn red pill; becomes ■ Stop
│                    00:12:34                  │   large semibold timer (red while recording)
│                     ● REC                    │   blinks while recording
│  Mic     ▕██████████░░░░░░░░▏  -12 dB        │   flat segmented meters, dB scale
│  Gain    ──●────  1.0×  [Auto]               │   short slider + AGC; live under mic meter
│  System  ▕████░░░░░░░░░░░░░░▏  -24 dB        │   with peak-hold tick
│                                              │
│  ! warning / device status (inline)          │
└──────────────────────────────────────────────┘
```
- The page uses a solid `COLOR_WINDOW` background. Input and Output are owner-drawn section headers: accent semibold text with a light separator line. Device combos use the flat file-dialog (`CFD`) theme with a taller selection field and drop-down rows; the Save to edit and Browse button match that height.
- The folder picker uses `IFileOpenDialog` with `FOS_PICKFOLDERS`. The path edit is read-only; Browse… validates writability. Default folder: `%USERPROFILE%\Documents\Skrivli`.
- Device combos and folder are disabled while recording. The mic gain slider and Auto checkbox stay enabled and apply immediately (including mid-take). Auto disables the slider and runs mixer AGC; the value label shows `Auto` while idle or the live effective gain while recording.
- Level meters map linear peak to a −60…0 dB scale, use a green→amber→red segmented fill, double-buffer painting, and hold a peak tick for about 1 s. The mic meter displays post-gain level so the gain controls under it are meaningful. They run continuously: while idle, `LevelMonitor` opens the selected mic + loopback (with keep-alive) and publishes peaks to atomics; while recording, peaks come from the mixer. The monitor stops before a recording starts and restarts when it ends or when the device selection changes.
- Inline warning label covers missing devices / unwritable folder; hard failures use `TaskDialogIndirect`.
- Hotkey: global `Ctrl+Alt+R` (`RegisterHotKey`, configurable or disableable) toggles recording.

### 8.3 Tab 2 — Recordings
- Split pane laid out in `OnSize`.
- **Left:** `CListViewCtrl` in report view with full-row select, columns **Name | Date | Duration | Size | Transcript**. The Transcript column shows `—`, `Queued`, `42 %`, `Ready`, or `Failed`.
- **Right:** empty state ("Select a recording") until a row is selected. Then **Play** / **Stop** (in-app miniaudio via `AudioPlayer`: `ma_decoder` + `ma_device`; WAV/FLAC/MP3), and below either a read-only multiline edit with the transcript text, or a **Create transcript** button (status text while a job is active; error + button again on failure).
- Context menu: **Play**, **Transcribe**, **Open folder**, **Rename**, **Delete** (to the Recycle Bin via `IFileOperation`, deleting audio + `.txt` + `.json` together, and a leftover `.md` if present), **Refresh**.
- Double-click plays the selected recording. Selecting another row stops playback.
- Data comes from `RecordingLibrary`, which scans the folder for `*.wav`/`*.m4a` and pairs them with `.txt`/`.json` (or legacy `.md`). `ReadDirectoryChangesW` on a background thread (debounced) keeps the list in sync with external changes.
- Sorted newest first by default; clicking a column header sorts by it.
- After each refresh, the previously selected audio path is reselected so the detail pane stays put while a job runs.

### 8.5 Tray
- `TrayIcon` wraps `Shell_NotifyIconW` (`NOTIFYICON_VERSION_4`, a GUID-identified icon, `NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP`).
- Icons: idle and recording (red dot). The tooltip shows "Recording 00:12:34" while recording.
- Left click toggles main window visibility. Right click opens a menu: **Show Skrivli**, **Start/Stop recording**, **Open recordings folder**, **Settings…**, **Exit**.
- Minimize goes to the tray (`SC_MINIMIZE`, then hide). The close button (X) hides to the tray, configurable in settings ("Close button minimizes to tray": default on). The first time, a balloon says "Skrivli is still running in the tray."
- Re-add the icon on `RegisterWindowMessage(L"TaskbarCreated")` so it survives an Explorer restart.
- Balloons for "Transcript ready", "Recording stopped: low disk space", and "Device lost — recording continues on <new device>".
- Optional setting: start with Windows (HKCU `Run` key) and start minimized (`--tray` argument).

### 8.6 Single instance
- Named mutex `Local\Skrivli.SingleInstance`. A second launch finds the existing window (`FindWindow` on the class name) and sends it a registered "show yourself" message, then exits.

### 8.7 Settings dialog
- Recording folder, default devices, mic/system gains, mic Auto (AGC), save separate stems, compress to M4A after recording, keep WAV after compression.
- Transcription: model, language, auto-transcribe after recording, threads, use VAD.
- General: close to tray, start with Windows, start minimized, global hotkey.

---

## 9. Error handling and edge cases
- No microphone present: allow system-only recording, with a warning label in the Record tab (and vice versa if there's no output device).
- Mic privacy setting disabled (`E_ACCESSDENIED` from `Activate`/`Initialize`): show a message with a button that opens `ms-settings:privacy-microphone`.
- Exclusive-mode apps holding a device: report a clear error and continue with the other source.
- Device unplugged mid-recording: switch to the new default, insert silence for the gap, add a note to the sidecar JSON.
- Folder deleted or offline (network share/USB): fail at start with a clear message. If it happens mid-recording, stop gracefully, then save to `%LOCALAPPDATA%\Skrivli\rescue\` and notify.
- Echo (speakers + mic): document that headphones give the best results. The communications category may enable driver AEC.
- Transcription errors (model missing or corrupt, out of memory): set the job to Failed with the reason shown in a tooltip or message; the audio is never touched.
- All HRESULTs get logged to `%LOCALAPPDATA%\Skrivli\logs\skrivli.log` (rotating, about 1 MB × 3).

---

## 10. Build and packaging
- CMake:
  - `project(Skrivli LANGUAGES C CXX)`, `CMAKE_CXX_STANDARD 20`, `/W4 /permissive- /utf-8`, static CRT optional (`CMAKE_MSVC_RUNTIME_LIBRARY`).
  - FetchContent: WTL (pinned), whisper.cpp (pinned), GoogleTest (tests only, `SKRIVLI_BUILD_TESTS` option).
  - Link: `ole32 uuid avrt mfplat mfreadwrite mfuuid shlwapi comctl32 winhttp` (WASAPI objects come from `CoCreateInstance`, so no extra import lib is needed).
  - Add `resources/Skrivli.rc` and the manifest to the executable.
- `scripts/start.bat` keeps working; add `scripts/test.bat` (`ctest -C Release`).
- Requirements: Visual Studio with the **C++ ATL** component (WTL depends on ATL). Windows 10 1903+ (for `AUTOCONVERTPCM` capture and modern tray APIs).
- Distribution (later milestone): portable zip first, then an MSIX or WiX installer. Models are not bundled; they're downloaded on demand.

---

## 11. Testing strategy
- **Unit tests** (no audio hardware):
  - `RingBuffer`: SPSC correctness, wraparound, overrun behaviour.
  - `Mixer`: alignment with synthetic QPC timestamps, gap filling, drift correction (simulate a 100 ppm clock difference), soft limiter.
  - `WavWriter`: header correctness, periodic header patching, recovery of truncated files.
  - `Resampler`: a 1 kHz sine at 48k → 16k keeps frequency and amplitude; aliasing of a 10 kHz tone is attenuated.
  - `TranscriptText`: paragraph merging, timestamps, de-duplication of repeated lines.
  - `RecordingLibrary`: pairing of audio/`.txt`/`.json` (and legacy `.md`), sorting, partial-file recovery.
- **Integration tests** (manual or optional CI on a machine with audio):
  - Record 10 min with intermittent playback (YouTube on/off) and talking; check sync by clapping near the mic while a click track plays.
  - Unplug a USB headset mid-recording; change the default device mid-recording.
  - Record 2 h and check file integrity, memory stability, and transcription chunking.
  - Hide to tray, restart Explorer, and confirm the icon comes back.
  - Transcribe English and one non-English sample with tiny/base models; measure the real-time factor.

---

## 12. Milestones

Each milestone ends with a working, runnable app.

1. **M0 — Foundation** (≈1 day)
   Remove `Window.hpp`, C++20, pin WTL, add `.rc` + manifest + icons, logging, `Settings`, single-instance mutex, COM init (`COINIT_APARTMENTTHREADED` on UI, MTA on workers).
2. **M1 — Shell UI** (≈2 days)
   Resizable main window, `CTabCtrl` with Record and Recordings pages (placeholder content), status bar, tray icon with menu, minimize/close to tray, re-add on Explorer restart.
3. **M2 — Single-source capture** (≈2 days)
   `DeviceManager`, `CaptureStream` (mic), `WavWriter` with `.partial` handling, Record/Stop working end to end, elapsed timer, folder picker.
4. **M3 — Loopback + mixing** (≈3–4 days)
   Loopback stream, `SilentRenderKeepAlive`, `RingBuffer`, `Mixer` with QPC alignment, drift control, gains, limiter, level meters. Unit tests for mixer/ring buffer.
5. **M4 — Robustness** (≈2 days)
   Device-change handling, low-disk handling, crash recovery, privacy-denied flow, exit/end-session while recording.
6. **M5 — Recordings tab** (≈2 days)
   `RecordingLibrary`, list view with columns/sorting/context menu, play/open/rename/delete, directory watcher, sidecar JSON.
7. **M6 — Transcription engine** (≈3 days)
   whisper.cpp integration in CMake, `AudioFileReader`, `Resampler`, `WhisperEngine`, `ModelManager` (download + browse), `TranscriptionService` queue with progress/cancel, `TranscriptText`.
8. **M7 — Transcript UI** (≈2 days)
   Recordings detail pane with Play/Stop and transcript text (or Create transcript). Transcript column states, tray balloon on completion.
9. **M8 — Settings and polish** (≈2 days)
   Settings dialog, global hotkey, start with Windows, DPI polish, optional M4A compression, keyboard accessibility (tab order, mnemonics), About dialog.
10. **M9 — Release** (≈1–2 days)
    Release build config, portable zip, README with requirements and privacy notes, a manual test pass using the checklist in §11.

---

## 13. Risks and mitigations

| Risk | Mitigation |
|---|---|
| Loopback produces no data during silence, so streams desync | Silent render keep-alive + QPC-based gap filling |
| Clock drift between mic and output devices | Fill-level-based drop/duplicate correction; tested with a simulated ppm offset |
| `AUTOCONVERTPCM` unavailable on some drivers | Fallback to MF Audio Resampler DSP |
| Echo when using speakers | Communications category (driver AEC), recommend headphones, optional separate stems |
| Whisper hallucinations in silence | VAD, de-duplication of repeated lines, `no_speech_thold` |
| Slow transcription on low-end CPUs | Default to the quantized base model, offer tiny, show the real-time factor, run in the background, allow cancel |
| Build time / size from whisper.cpp | Static CPU-only build, examples/tests disabled; binary grows by a few MB, models are external |
| WTL tracking `main` breaks the build | Pin to a tag/commit |

---

## 14. Future work
- Speaker separation using the mic/system stems ("You" vs. "Remote") and/or a diarization model.
- Live transcription while recording (streaming whisper on 5–10 s windows).
- Summaries and action items with a local LLM (llama.cpp) appended to the transcript.

- Search across all transcripts in the Recordings tab.
- GPU acceleration (Vulkan/CUDA backends of ggml) as an optional build.
