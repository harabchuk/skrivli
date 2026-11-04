# core

Domain types and pure logic. No windows, devices, files, network, or third-party SDKs. `tests/` is not indexed.

Update this file when a file here is added, removed, renamed, or its responsibility changes.

| File | Purpose | Role | Domain | Features |
|---|---|---|---|---|
| `AudioFormat.hpp` | Internal 48 kHz stereo float layout, 10 ms block size, and QPC-to-frame conversion. | Format constants | Audio | Recording, Loopback mix |
| `RingBuffer.hpp` | Lock-free SPSC ring of stereo float packets with QPC stamps. Push drops on overrun. | Queue | Audio | Recording, Level meters |
| `ChunkPlan.hpp` | Splits recordings longer than 1 h into 30 min pieces and offsets whisper timestamps. | Planner | Transcription | Transcription |
| `MarkdownFormatter.hpp`, `MarkdownFormatter.cpp` | Turns segments into the meeting Markdown and writes UTF-8 without a BOM. | Formatter | Transcription | Transcription |
| `Recording.hpp` | `RecordingSidecar` (JSON fields) and `RecordingEntry` (one library row: audio, json, md). | Value types | Recordings | Recordings library, Transcription |
| `Transcript.hpp` | `TranscriptStatus`, `TranscriptSegment`, `TranscriptMeta`, and the default whisper model name. | Value types | Transcription | Transcription |
