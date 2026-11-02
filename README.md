# MeetingBuddy

Native Windows tray app that records your microphone and system audio together,
saves a mixed WAV (optional M4A) to a folder you choose, and can optionally
transcribe it locally with whisper.cpp to Markdown.

## Requirements

- 64-bit Windows 10 version 1903 or later
- Visual Studio with the **Desktop development with C++** workload and the **C++ ATL** component (WTL needs ATL)
- CMake 3.24 or later
- Visual C++ redistributable matching the MSVC toolchain used to build (the portable zip does not bundle the CRT)

## Build and run

```powershell
scripts\start.bat
```

This configures `build/` on first run, builds Release, stops any running instance, and launches `build\Release\MeetingBuddy.exe`.

Unit tests (no audio hardware):

```powershell
scripts\test.bat
```

Portable zip (Release exe + this README; models are not included):

```powershell
scripts\package.bat
```

Output lands in `dist\MeetingBuddy-<version>.zip`.

## Privacy

- Transcription runs entirely on your machine.
- There is no telemetry and no cloud transcription.
- The only network access is a model download that you start from Settings.

## Models

Whisper models are **not** bundled. Download one from Settings (or browse to a local `ggml-*.bin`) before transcribing. The default is `ggml-base-q5_1.bin` under `%LOCALAPPDATA%\MeetingBuddy\models\`.

After each transcription, MeetingBuddy logs the real-time factor (wall time ÷ audio duration) to `%LOCALAPPDATA%\MeetingBuddy\logs\meetingbuddy.log`.

## Audio tips

Headphones are the reliable way to avoid speaker/mic echo. The communications-processing option only asks the driver for AEC; it is not a substitute for headphones.

## App data

| What | Where |
|---|---|
| Settings | `%APPDATA%\MeetingBuddy\settings.json` |
| Models | `%LOCALAPPDATA%\MeetingBuddy\models\` |
| Logs | `%LOCALAPPDATA%\MeetingBuddy\logs\` |
| Default recordings | `%USERPROFILE%\Documents\MeetingBuddy` |

## Release notes (v1.0.0)

- A full two-hour recording was not captured for the M9 manual pass. Transcription chunking for audio longer than one hour is covered by the `ChunkPlan` unit tests with a synthetic long buffer.
- Measure real-time factor from the log after a local English and non-English transcription with tiny or base; it is not reported over the network.
