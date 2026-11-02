#pragma once

#include <windows.h>

#include <cstdint>
#include <optional>

// Fields of the canonical 44-byte PCM WAV header written by WavWriter ("fmt " then "data" at
// fixed offsets).
struct WavHeader {
    static constexpr uint32_t kHeaderBytes = 44;

    uint16_t format = 0;
    uint16_t channels = 0;
    uint32_t sampleRate = 0;
    uint16_t bits = 0;
    uint32_t dataSize = 0;

    // Seeks to the start of `file` and reads the header. On success the file pointer is at
    // `kHeaderBytes`, i.e. the first PCM byte. Fails when fewer than 44 bytes are available or
    // the RIFF/WAVE tags are missing.
    static auto Read(HANDLE file) -> std::optional<WavHeader>;

    // Zero when the header describes no audio (missing channels, rate, or bit depth).
    double DurationSeconds() const;
};
