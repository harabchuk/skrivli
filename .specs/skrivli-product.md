# Skrivli — Product Specification

**Audience:** product, design, and anyone describing what Skrivli does for users.  
**Perspective:** user-facing behavior only. Implementation details live in `skrivli-app-design.md` and `skrivli-implementation-plan.md`.  
**Platform:** Windows 10 and 11 desktop.  
**Status:** v1 product definition derived from the implementation plan and app design.

When a feature is added, changed, deferred, or removed, update this document so it stays the accurate description of what users can do.

---

## 1. Product overview

Skrivli is a local Windows app for capturing meetings, calls, and other spoken sessions. It records your microphone and what plays through your computer at the same time, saves one continuous audio file on your machine, and can turn a finished recording into a timestamped plain-text transcript — entirely offline after you install a transcription model.

There is no account, no cloud sync, and no automatic upload of audio or transcripts. Files stay where you put them.

### Primary value

- Capture both sides of a conversation (your voice and system playback) in one file.
- Keep recording reliable when one side is quiet, a device drops, or the app is interrupted.
- Browse, play, rename, and delete recordings from the app.
- Create a selectable, copyable transcript without sending audio anywhere.

### Who it is for

People who take meetings or calls on Windows and want a private, local recording plus optional text notes — without a SaaS workflow.

---

## 2. Core user journey (v1)

1. Install by unzipping a portable package and launching Skrivli.
2. On first useful transcription, choose a transcription model from the list and download it. The dialog can be closed at any time except while a download is running; closing it abandons that transcription. When the download finishes, the model becomes the one in use, the dialog closes, and the transcription starts without a second click.
3. On the **Record** tab, check the microphone and system audio sources, and use Change on either one if needed.
4. Start recording (window, tray, or keyboard shortcut). Speak and/or play audio as usual.
5. Stop recording. A finished audio file appears in the chosen folder and in the **Recordings** tab.
6. Open a recording, play it back, and optionally create a transcript.
7. Select and copy transcript text for notes, email, or another tool.

After a model is installed, the record → save → transcribe → copy path works without a network connection.

---

## 3. Application shell

### Window

- Resizable main window with two tabs: **Record** and **Recordings**.
- Remembers size and position across launches (restored safely on the available display).
- Status area shows transcription progress on both tabs. On the Recordings tab it also shows recording state, elapsed time, and notices; the Record tab shows those itself.
- A Settings gear on the tab bar opens Settings; Settings shows the transcription model in use (with its description and size) and a Change model button that opens the model dialog on top (Choose model when none is installed), the close-to-tray switch with a one-line explanation, and the app version in its footer. Opening the recordings folder and Exit remain available from the Recordings tab and the tray.
- A theme button next to Settings cycles theme preference. Order is System → opposite of Windows → the other palette → System (light OS: System, Dark, Light; dark OS: System, Light, Dark). The icon and tooltip name the current choice; the preference is remembered across launches.

### System tray

- Skrivli can stay available from the notification area.
- Closing or minimizing the window can hide the app to the tray instead of quitting (configurable; first use explains this).
- Tray actions: show/hide the window, start/stop recording, open recordings folder, Settings, Exit.
- While recording, the tray indicates that capture is active and can show elapsed time.
- Exit is always an explicit choice. Hiding to the tray does **not** stop an in-progress recording.

### Single instance

- Only one Skrivli process runs at a time.
- Starting the app again brings the existing window forward instead of opening a second copy.

### Keyboard

- Major actions are usable with the keyboard.
- A global shortcut can start or stop recording (default **Ctrl+Alt+R**, configurable or disableable).

---

## 4. Recording

### What is captured

- **Microphone** — your voice (defaults to the Windows communications microphone). The microphone is recorded as mono: both channels of the file carry the same microphone signal. A true stereo microphone loses its stereo image. A signal that is only on one side is about 6 dB quieter.
- **System audio** — what plays through the selected output device (meetings, browser, apps). When system audio is captured, speaker echo is removed from the microphone automatically. That removal is always on. There is no setting to turn it off.
- Both sources are combined into **one** continuous recording. Quiet periods on either side do not shrink or skip the timeline; silence is preserved so timing stays natural. The saved file is that mix, with the microphone gain applied.

If only one source is available (for example, no microphone permission or no playback device), recording can continue with that source and the UI makes the limitation clear.

### Record tab

Users can:

- See the current microphone and system audio device by name, and use a small Change button on either to pick from the available Windows devices.
- See live level meters for microphone and system audio (with clear numeric levels, not color alone). Peak hold is a tick on the meter, with its value in a tooltip on the level.
- Adjust microphone gain with a manual slider.
- Start and stop recording; see a clear recording indicator and elapsed timer.
- Read inline warnings for missing devices, permission problems, or storage issues.

Device choices are locked while a recording is active; a lock icon replaces Change. The gain slider can stay adjustable during capture and is disabled while saving. Recordings save to the configured directory (default: a Skrivli folder under Documents when writable); the save location is chosen in Settings.

### Starting and stopping

- Start and stop from the Record tab, the tray, or the global hotkey — all do the same thing.
- After stop, the recording is saved as a playable file with a dated name (for example `Skrivli_YYYY-MM-DD_HH-MM-SS`).
- Starting is refused or clearly blocked when the save location is not usable.

### Durability and recovery (user-visible)

Users should trust that Skrivli tries to keep usable audio when things go wrong:

- Interrupted recordings are recovered on next launch when possible (presented as recovered files, not silently discarded).
- Low disk space is warned; recording stops in a controlled way before the disk is exhausted when the app can detect it.
- If a device is unplugged or becomes unavailable mid-session, the other source can keep going; gaps are reflected as silence rather than a broken timeline.
- Closing the window to the tray leaves recording running. Quitting while recording asks to stop and save.
- Windows logoff/shutdown triggers a best-effort save; there is no guarantee of unlimited time, but the app attempts to finalize what it can.

Skrivli does not silently delete usable partial recordings.

---

## 5. Recordings library

### Browsing

- The **Recordings** tab lists saved recordings, newest first, with name, date, duration, size, and transcript status.
- The list updates when files change in the folder (including changes made outside the app).
- Empty and error states are clear when there is nothing to show or the folder cannot be read.
- In-progress captures are not treated as finished library entries until saved (except when presenting recovered files).

### Detail and playback

- Selecting a recording shows details and playback controls (play / stop).
- Changing selection stops playback.
- Users can open the containing folder in Explorer, refresh the list, rename, or delete.
- Rename keeps related files together (audio, transcript, and metadata) when possible; partial failures are reported.
- Delete asks for confirmation and sends files to the Recycle Bin rather than permanent erasure by default.

### Transcript status in the list

| Status | Meaning |
| --- | --- |
| — | No transcript yet |
| Queued | Waiting to run |
| *n*% | In progress |
| Ready | Transcript available |
| Failed | Last attempt failed; audio is still intact |

---

## 6. Transcription

Transcription runs **after** a recording is finished. It does not produce live captions while recording.

### Models

- Transcription models are **not** bundled with the app.
- On first transcription without a model, the user picks a model from the always-visible list. The dialog says "Runs on this PC. Your audio never leaves it." Each model is a bordered card with a plain name (Tiny, Base, Small), a one-line description, and general Speed and Accuracy ratings out of three; the highlighted card has a stronger border and a radio dot. A card shows whether the model is in use (green "In use" tag) or already installed ("Installed" tag). Every card shows the model size, installed or not. During a download the other cards are dimmed and disabled, and the downloading card shows a progress bar with the percentage and an approximate "About 76.1 MB of 181.3 MB"; a failure appears inside the failed model's card. The footer shows only the buttons that apply to the highlighted model, at their natural width and aligned to the right: Close and Download with its size, Close and Use selected model, Close for the model in use, Cancel download while downloading, or Close and Retry download for the model that failed. A failed download is explained inside the dialog, with no second popup. Clicking a model only highlights it; the model in use changes only when the user presses Use selected model on an installed model or a download of it finishes. Download starts only from the Download button. The dialog can be closed at any time except during a download (Close, the X, Escape, or the backdrop), so a user with no network is never trapped. Cancel and Retry apply to the download itself, and Retry is offered only for the model that failed. If Transcribe opened the dialog, a finished download or Use closes it and starts that transcription; if the user opened it themselves it stays open.
- The status bar always shows the current transcription model on the right (for example "Model: Base"), "Downloading Small 42%" during a download, or "No model installed · Choose…" when none is available; it shows nothing until the installed models are known. Clicking it opens the model dialog. The left of the status bar reads "Ready", "Recording 00:12:41", "Saving" or "Transcribing 42%", with any notice, and gives way first when the window is narrow.
- Downloads happen only when the user starts them, with progress, cancel, and verification feedback. Corrupt or unsupported models are reported distinctly.
- After installation, transcription works fully offline.
- Users can choose among offered model sizes/variants and language settings when available.

### Creating a transcript

- From a selected recording: create transcript, cancel, or retry after failure.
- Jobs run one at a time in a queue; progress is visible in the UI and status strip.
- Auto-transcribe after each new recording is available but **off by default**.
- Completion can show a Windows/tray notification (when allowed); opening it focuses the matching recording when possible.
- Failed transcription never damages the original audio.

### Transcript content

- Plain text with timestamps like `[HH:MM:SS]`, including sessions longer than one hour.
- Shown in a read-only view that supports selection, select-all, copy, scrolling, and keyboard use.
- Associated with the recording as a sibling text file the user owns and can open outside the app.

### Identity safety

If a recording is renamed or deleted while transcription is queued or running, Skrivli cancels or fails safely rather than writing text to the wrong place.

---

## 7. Settings and Windows integration

Preferences persist across restarts. Hazardous options are disabled while recording when changing them would be unsafe.

### Recording

- Default save directory (folder picker); the choice is remembered
- Default microphone and system audio devices
- Microphone gain, set with the manual slider
- Optional separate source files (mic / system) — available when enabled; off by default
- Optional lossless FLAC retention alongside or instead of WAV after a successful conversion — optional; WAV is never discarded on a failed conversion

### Transcription

- Selected model, language, and processing preferences (for example CPU thread count)
- Optional voice-activity filtering when offered — off by default
- Auto-transcribe after save — off by default

### App behavior

- Close window to tray (default on, with first-use explanation)
- Start hidden in the tray
- Optional Windows startup (opt-in only; never enabled by surprise)
- Global recording hotkey (enable/disable and rebind; conflicts are surfaced)

### Appearance and accessibility

- Light, Dark, or System theme from the tab-bar control (System follows Windows)
- Usable at common display scaling levels and on multiple monitors
- Keyboard-focused operation for main flows
- Clear error messages for permission, device, storage, and model problems

---

## 8. Privacy and data ownership

- No account required.
- No telemetry or analytics product surface in v1.
- No cloud transcription; audio and text are not uploaded for speech-to-text.
- Model download is explicit and user-initiated.
- Recordings, transcripts, and models live in user-controlled locations and can be deleted by the user.
- Diagnostic logs exclude audio samples and transcript text.

---

## 9. Distribution and first run

- Delivered as a portable ZIP: unpack, run, no mandatory installer for v1.
- Works from paths with spaces or Unicode characters when Windows allows normal app writes.
- Does not require a developer machine or hidden manually installed runtimes beyond what the package documents.
- Transcription models are separate: size and network use are disclosed when the user chooses to download.
- Privacy notes and licensing notices ship with the package.

### Happy-path acceptance (clean PC)

A new user can: unzip → launch → record microphone and system audio → stop → find and play the file → explicitly install a model → transcribe offline → copy the text.

---

## 10. Scope

### In v1 (mandatory product surface)

- Combined microphone + system recording with continuous timeline
- Local save to a chosen folder
- Durable capture with recovery for common failures
- Recordings library with playback, rename, delete, open folder
- Offline post-recording transcription with model install, progress, cancel, and copyable text
- Tray lifecycle, single instance, essential settings, hotkey, and keyboard-accessible main flows
- Portable Windows distribution without bundled models

### Optional / deferrable without blocking core v1

- Lossless FLAC post-processing
- Separate mic/system stem files
- Voice activity detection for transcription
- Expanded theming and a full installer

### Out of v1

- Live transcription while recording
- Cloud sync, accounts, or remote storage
- Speaker diarization (who spoke when)
- Summaries or action-item generation
- Video or screen capture
- GPU-accelerated transcription as a required path
- Non-Windows platforms

### Later directions (not commitments)

- Better separation of local vs remote voices
- Live transcription once offline recording is solid
- Local summaries without cloud dependency
- Search across transcripts
- Optional GPU acceleration when packaging is proven

---

## 11. Document map

| Document | Role |
| --- | --- |
| `skrivli-product.md` (this file) | What users can do; product behavior and scope |
| `skrivli-app-design.md` | Technical design and architecture |
| `skrivli-implementation-plan.md` | Milestone backlog and acceptance gates |
| `decisions.md` | Recorded design decisions and spikes |
| `AGENTS.md` | Coding conventions and when to update specs |
