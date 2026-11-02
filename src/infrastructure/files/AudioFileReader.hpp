#pragma once

#include <cstdint>
#include <string>
#include <vector>

struct DecodedAudio {
    std::vector<float> interleavedStereo; // 48 kHz stereo float
    uint32_t sampleRate = 48000;
    uint16_t channels = 2;
    double durationSeconds = 0.0;
};

class AudioFileReader {
public:
    // Decodes WAV (miniaudio) or M4A (Media Foundation) to 48 kHz stereo float.
    // Does not resample to 16 kHz.
    static bool Read(const std::wstring& path, DecodedAudio& out, std::wstring& error);
};
