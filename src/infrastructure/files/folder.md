# infrastructure/files

Audio encode and decode. `tests/` is not indexed.

Update this file when a file here is added, removed, renamed, or its responsibility changes.

| File | Purpose | Role | Domain | Features |
|---|---|---|---|---|
| `WavWriter.hpp`, `WavWriter.cpp` | Writes PCM 16-bit 48 kHz stereo to `*.wav.partial`, patches the header, and renames on stop. | Encoder | Files | Recording, Crash recovery, Low disk |
| `M4aEncoder.hpp`, `M4aEncoder.cpp` | Encodes that WAV to AAC 128 kbps M4A with Media Foundation. Does not delete the WAV. | Encoder | Files | M4A export |
| `AudioFileReader.hpp`, `AudioFileReader.cpp` | Decodes WAV or M4A to 48 kHz stereo float for transcription. | Decoder | Files | Transcription |
| `AudioPlayer.hpp`, `AudioPlayer.cpp` | Plays a recording via MFPlay and posts `WM_APP_PLAYBACK` when play state changes. | Player | Files | Recordings library |
