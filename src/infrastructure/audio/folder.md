# infrastructure/audio

WASAPI and miniaudio. No UI. `tests/` is not indexed.

Update this file when a file here is added, removed, renamed, or its responsibility changes.

| File | Purpose | Role | Domain | Features |
|---|---|---|---|---|
| `DeviceManager.hpp`, `DeviceManager.cpp` | Enumerates capture and render devices, resolves a saved id, and posts device-change notifications. | WASAPI enumerator | Audio | Device selection, Device change |
| `CaptureStream.hpp`, `CaptureStream.cpp` | Event-driven mic (`eCapture`) or loopback (`eRender`) thread. Copies packets and QPC into a `RingBuffer`. | Capture thread | Audio | Recording, Loopback mix, Level meters |
| `SilentRenderKeepAlive.hpp`, `SilentRenderKeepAlive.cpp` | Plays silence on the loopback render device so the engine clock keeps moving. | Render client | Audio | Loopback mix |
| `Mixer.hpp`, `Mixer.cpp` | Aligns mic and system by QPC, fills gaps, applies gain and optional mic AGC, and emits 10 ms blocks. | DSP | Audio | Recording, Loopback mix, Level meters |
| `AudioResampler.hpp`, `AudioResampler.cpp` | Converts one device mix-format packet to 48 kHz stereo float. | DSP | Audio | Recording, Loopback mix |
| `MaConverter.hpp`, `MaConverter.cpp` | RAII `ma_data_converter` that appends float frames to a buffer. | miniaudio wrapper | Audio | Recording, Transcription |
| `MiniAudio.hpp` | Includes `miniaudio.h` with the same `MA_NO_*` switches as the CMake static library. | SDK header | Audio | Recording, Transcription |
| `Resampler.hpp`, `Resampler.cpp` | Stereo 48 kHz float to mono 16 kHz for whisper. | DSP | Audio | Transcription |
| `WavHeader.hpp`, `WavHeader.cpp` | Reads the 44-byte PCM WAV header and reports duration. | Parser | Audio | Recordings library, Crash recovery |
