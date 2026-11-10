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
- Create a transcript, read it in the app, and copy it, without sending audio anywhere.

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
7. Copy the transcript for notes, email, or another tool.

After a model is installed, the record → save → transcribe → copy path works without a network connection.

---

## 3. Application shell

### Window

- Resizable main window with two tabs: **Record** and **Recordings**.
- Remembers size and position across launches (restored safely on the available display).
- The status bar is shared by both tabs and reads the same on each: recording state, elapsed time, notices (including "Saved." after a recording), transcription progress, and the selected model. Warnings also appear as a banner on the Record tab.
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
- On the Recordings tab, **Up** and **Down** move through the list, **Space** plays or pauses, **F2** renames and **Delete** asks to delete the selected recording. These keys do nothing while a dialog is open, and Space works only when the page itself has focus (not when a button or the seek bar does). **Escape** closes menus and dialogs.

---

## 4. Recording

### What is captured

- **Microphone** — your voice (defaults to the Windows communications microphone). The microphone is recorded as mono: both channels of the file carry the same microphone signal. A true stereo microphone loses its stereo image. A signal that is only on one side is about 6 dB quieter.
- **System audio** — what plays through the selected output device (meetings, browser, apps). When system audio is captured, speaker echo is removed from the microphone automatically. That removal is always on. There is no setting to turn it off.
- Both sources are combined into **one** continuous recording. Quiet periods on either side do not shrink or skip the timeline; silence is preserved so timing stays natural. The saved file is that mix, with the microphone gain applied.

If only one source is available (for example, no microphone permission or no playback device), recording can continue with that source and the UI makes the limitation clear.

Either source can also be turned off on purpose by choosing **Disabled** in its Change menu. The recording then contains only the other source, the turned-off source's meter stays empty, and no warning is shown because nothing is missing. Echo removal is only active while system audio is on.

### Record tab

Users can:

- See the current microphone and system audio device by name, and use a small Change button on either to pick from the available Windows devices. The menu ends with **Disabled**, which turns that source off.
- See live level meters for microphone and system audio (with clear numeric levels, not color alone). Peak hold is a tick on the meter, with its value in a tooltip on the level.
- Adjust microphone gain with a manual slider.
- Start and stop recording; see a clear recording indicator and elapsed timer.
- Read inline warnings for missing devices, permission problems, or storage issues.

Device choices are locked while a recording is active; a lock icon replaces Change. The gain slider can stay adjustable during capture and is disabled while saving. Recordings save to the configured directory (default: a Skrivli folder under Documents when writable); the save location is chosen in Settings.

### Starting and stopping

- Start and stop from the Record tab, the tray, or the global hotkey — all do the same thing.
- After stop, the recording is saved as a playable file with a dated name (for example `Skrivli_YYYY-MM-DD_HH-MM-SS`).
- Starting is refused or clearly blocked when the save location is not usable.
- Starting is refused with "Turn on the microphone or system audio to record." when both sources are set to Disabled, from the Record tab, the tray, or the hotkey. No file is created.

### Durability (user-visible)

Users should trust that Skrivli tries to keep usable audio when things go wrong:

- If a recording fails part way, Skrivli saves what it can as a normal recording and says what went wrong. A recording interrupted by a crash or power loss is not recovered.
- Low disk space is warned; recording stops in a controlled way before the disk is exhausted when the app can detect it.
- If a device is unplugged or becomes unavailable mid-session, the other source can keep going; gaps are reflected as silence rather than a broken timeline.
- Closing the window to the tray leaves recording running. Quitting while recording asks to stop and save.
- A single recording stops by itself when it reaches the **Maximum meeting duration** set in Settings (default 3 hours). The file is finalized and the app shows "Recording stopped because it reached the maximum meeting duration."
- Recordings are saved as `.flac` files (written as `.flac.partial` until you stop). Audio files in other formats in the folder are not listed, played or transcribed; they are left untouched on disk.
- Windows logoff/shutdown triggers a best-effort save; there is no guarantee of unlimited time, but the app attempts to finalize what it can.

Skrivli does not silently delete usable partial recordings. Leftover unfinished files are not shown in the app.

---

## 5. Recordings library

### Browsing

- The **Recordings** tab lists saved recordings, newest first, in a fixed-width list grouped under **Today**, **Yesterday** and **Earlier** headings. A heading appears only for a day that has recordings. Times are shown in the PC's local time zone.
- Each row shows the recording name, a muted line with when, how long and how large it is (`14:32 · 12:41 · 38.0 MB` for today and yesterday, `Nov 3 · 31:04 · 93.0 MB` for earlier days), and a transcript badge on the right. The selected row has a tinted background and an accent bar on its left edge.
- The list header shows the word "Recordings", the number of recordings, a Refresh icon button and an Open folder icon button. The list is not sortable and has no search.
- The list updates when files change in the folder (including changes made outside the app).
- With no recordings, the tab shows an empty state ("No recordings yet", with a **Start recording** button that switches to the Record tab; it does not start recording). When the folder cannot be read, the list says "Recordings unavailable" with the reason.
- Opening the tab with nothing selected selects the newest recording. "Select a recording" shows only if there is nothing to select.
- In a window narrower than 680 px the list sits above the detail instead of beside it.
- In-progress captures are not treated as finished library entries until saved.

### Detail and playback

- Selecting a recording shows a title row with its name in large text and a detail line below it (`Today, 14:32 · 12:41 · 38.0 MB · FLAC`), then a player card.
- A pencil icon next to the name (dimmed until the title row is hovered) renames the recording. A trash icon and a `⋯` menu sit at the right of the title row. Delete asks for confirmation. The `⋯` menu has **Transcribe again…** (enabled only when the recording has a transcript and no job is queued or running) and **Show in folder**, which opens the recordings folder in Explorer. Every icon button has a tooltip and an accessibility label.
- A warning note appears under the title when its details file is missing or unreadable, or when problems were logged while recording. A clean recording shows no note.
- The player card has a round Play/Pause button, the current time, a seek bar and the total time. Play starts from the seek bar position, Pause keeps the position, and Play again resumes. Dragging the seek bar moves nothing until it is released. When a recording ends the player returns to the start. Every recording in the list can be played.
- While a recording plays or is paused, the transcript paragraph being spoken is highlighted and the list scrolls to it when it changes. If you scroll the transcript yourself, it is not scrolled again for 5 seconds.
- Changing selection, renaming or deleting stops playback.
- Users can also open the recordings folder (list header icon button) and refresh the list (list header icon button).
- Rename keeps related files together (audio, transcript, and metadata) when possible; partial failures are reported.
- Delete asks for confirmation and sends files to the Recycle Bin rather than permanent erasure by default.

### Transcript status in the list

| Status | Meaning |
| --- | --- |
| (no badge) | No transcript yet |
| Queued | Waiting to run |
| *n*% | In progress |
| Transcript (green) | Transcript available |
| Failed | Last attempt failed and there is no transcript; audio is still intact. A failed or cancelled attempt on a recording that already has a transcript shows the transcript again, with no badge change |

---

## 6. Transcription

Transcription runs **after** a recording is finished. It does not produce live captions while recording.

### Models

- Transcription models are **not** bundled with the app.
- On first transcription without a model, the user picks a model from the always-visible list. The dialog says "Runs on this PC. Your audio never leaves it." Each model is a bordered card with a plain name that states its language (for example "Nemotron 3.5 (Multilingual)" and "Parakeet TDT v3 (European)"), a one-line description, and general Speed and Accuracy ratings out of three; the highlighted card has a stronger border and a radio dot. A card shows whether the model is in use (green "In use" tag) or already installed ("Installed" tag). Every card shows the model size, installed or not. During a download the other cards are dimmed and disabled, and the downloading card shows a progress bar with the percentage and an approximate "About 469.0 MB of 938.1 MB"; a failure appears inside the failed model's card. The footer shows only the buttons that apply to the highlighted model, at their natural width and aligned to the right: Close and Download with its size, Close and Use selected model, Close for the model in use, Cancel download while downloading, or Close and Retry download for the model that failed. A failed download is explained inside the dialog, with no second popup. Clicking a model only highlights it; the model in use changes only when the user presses Use selected model on an installed model or a download of it finishes. Download starts only from the Download button. The dialog can be closed at any time except during a download (Close, the X, Escape, or the backdrop), so a user with no network is never trapped. Cancel and Retry apply to the download itself, and Retry is offered only for the model that failed. If Transcribe opened the dialog, a finished download or Use closes it and starts that transcription; if the user opened it themselves it stays open.
- The status bar always shows the current transcription model on the right (for example "Model: Nemotron 3.5 (Multilingual)"), "Downloading Nemotron 3.5 (Multilingual) 42%" during a download, or "No model installed · Choose…" when none is available; it shows nothing until the installed models are known. Clicking it opens the model dialog. The left of the status bar reads "Ready", "Recording 00:12:41", "Saving" or "Transcribing 42%", with any notice, and gives way first when the window is narrow.
- Downloads happen only when the user starts them, with progress, cancel, and verification feedback. Corrupt or unsupported models are reported distinctly.
- After installation, transcription works fully offline.
- Six models are offered in three families, and all detect the language automatically, so there is no language setting; a language choice is a possible later addition:
  - **Nemotron 3.5 (Multilingual)**, NVIDIA's Nemotron 3.5 ASR 0.6B, quantized to Q8_0, the default. A download of about 984 MB (938.1 MB as the app shows it). It covers 40 languages, including English and Swedish, and adds punctuation and capitalization itself.
  - **Parakeet TDT v3 (European)**, NVIDIA's Parakeet TDT 0.6B v3. It covers 25 European languages, including English and Swedish, and is offered as Q8_0 (about 941 MB), Compact Q6_K (about 813 MB) and Smallest Q4_K (about 675 MB); the smaller ones trade some accuracy for download size. It is licensed CC BY 4.0, and its notice ships with the app.
  - **Parakeet TDT v2 (English)**, NVIDIA's Parakeet TDT 0.6B v2, English only, offered as Q8_0 (about 904 MB) and Compact Q6_K (about 776 MB). It is licensed CC BY 4.0.
- Whisper models are no longer offered. Models downloaded before this change stay on disk, are not listed, and are never deleted. A user who had a Whisper model selected sees "No model installed · Choose…" and picks the new model the next time they press Transcribe.
- After recording stops, the recording is cut at pauses in the speech into short pieces of about 8 to 30 seconds, and each piece is transcribed on its own. This recovers phrases that one long continuous pass can drop, for example right after one speaker hands over to another. Transcription still starts only after the recording is finished, and cancel takes effect within about 10 seconds (the piece being transcribed finishes first).

### Creating a transcript

- From a selected recording: create transcript, cancel, or retry after failure. A recording that already has a transcript has no Transcribe button, so it cannot be overwritten by accident. Cancelling a first transcription returns the recording to "No transcript yet", not Failed.
- **Transcribe again…** in the `⋯` menu asks "Replace this transcript?", naming the model in use, with **Replace** and **Cancel**. After Replace the area shows the same Queued and Running states as a first run, and the old transcript is hidden while the job runs. The old transcript is replaced only when the new one is ready. If the user cancels the job, the old transcript returns with no error. If the job fails, the old transcript returns and the status bar says "Transcription failed. Your previous transcript is unchanged." If the app closes during the job, the old transcript is still there after restart. With no model installed, the model dialog opens first and the job starts when a model is ready.
- Jobs run one at a time in a queue; progress is visible in the UI and status strip.
- Auto-transcribe after each new recording is available but **off by default**.
- Completion can show a Windows/tray notification (when allowed); opening it focuses the matching recording when possible.
- Failed transcription never damages the original audio.

### What the transcript area shows

Under the player the Recordings detail pane shows exactly one of these states:

| State | What the user sees |
| --- | --- |
| No transcript | "No transcript yet", "Runs on this PC. Your audio never leaves it.", and a **Transcribe** button |
| Queued | "Queued", "Starts when the current transcription finishes.", and **Cancel** |
| Running | "Transcribing", the percentage, a progress bar, and **Cancel** |
| Failed | "Transcription failed", the reason, "The recording itself is intact.", and a **Retry** button |
| Ready | A "Transcript" heading with a Copy button, and a scrolling list with one item per paragraph |
| Unreadable | "Could not open the transcript" and the reason |

**Cancel** is shown only while the selected recording has a queued or running job.

### Transcript content

- Plain text with timestamps like `[HH:MM:SS]`, including sessions longer than one hour.
- Shown as a scrolling list with one item per paragraph, with the timestamp in a column to the left of the text. A scrollbar on the right shows where you are in the transcript and can be dragged. Page Up, Page Down, Home and End scroll the list while it has focus. The **Copy** button copies the whole transcript, including timestamps. Selecting part of the text is not supported; open the `.txt` file for anything finer.
- Associated with the recording as a sibling text file the user owns and can open outside the app.

### Identity safety

If a recording is renamed or deleted while transcription is queued or running, Skrivli cancels or fails safely rather than writing text to the wrong place.

---

## 7. Settings and Windows integration

Preferences persist across restarts. Hazardous options are disabled while recording when changing them would be unsafe.

### Recording

- Default save directory (folder picker); the choice is remembered
- Default microphone and system audio devices; either can be set to Disabled, and the choice is remembered across restarts
- Microphone gain, set with the manual slider
- Optional separate source files (mic / system) — available when enabled; off by default
- Recordings are always saved as lossless FLAC (16-bit stereo 48 kHz). There is no format choice and no conversion step.
- **Maximum meeting duration** (Settings → Recording): 1, 2, 3, 4, 6, 8 or 12 hours, default 3 hours. It stops a forgotten recording so it does not write a very long file. A change applies to the next recording.

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
- Releases are downloaded from the GitHub Releases page of `harabchuk/skrivli`. Each release has the portable ZIP and a SHA-256 checksum file for it. The executable is not code-signed, so Windows SmartScreen may warn on first launch; the README in the ZIP explains this.
- Settings shows the app version in its footer, matching the version of the release.

### Happy-path acceptance (clean PC)

A new user can: unzip → launch → record microphone and system audio → stop → find and play the file → explicitly install a model → transcribe offline → copy the text.

---

## 10. Scope

### In v1 (mandatory product surface)

- Combined microphone + system recording with continuous timeline
- Local save to a chosen folder
- Durable capture that keeps usable audio after common failures (device loss, low disk, write errors)
- Recordings library with playback, rename, delete, open folder
- Offline post-recording transcription with model install, progress, cancel, and copyable text
- Tray lifecycle, single instance, essential settings, hotkey, and keyboard-accessible main flows
- Portable Windows distribution without bundled models

### Optional / deferrable without blocking core v1

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
