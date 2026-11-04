# services

One user-facing workflow per type. Owns threads and session state. Talks to the UI only with `PostMessage`.

Update this file when a file here is added, removed, renamed, or its responsibility changes.

| File | Purpose | Role | Domain | Features |
|---|---|---|---|---|
| `RecordingSession.hpp`, `RecordingSession.cpp` | One take: start mic and loopback, mix, write WAV (and optional stems), stop or discard. Reopens devices and watches free disk. | Session state machine | Audio | Recording, Loopback mix, Device change, Low disk, Crash recovery, M4A export |
| `LevelMonitor.hpp`, `LevelMonitor.cpp` | Idle mic and system peaks with no disk I/O. Stop it before a recording starts. | Meter drain thread | Audio | Level meters |
| `TranscriptionService.hpp`, `TranscriptionService.cpp` | FIFO of one cancellable job at a time: decode, chunk, whisper, write Markdown, update the sidecar. | Worker queue | Transcription | Transcription, Auto-transcribe |
