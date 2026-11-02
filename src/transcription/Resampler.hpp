#pragma once

#include <cstdint>
#include <vector>

class Resampler {
public:
    // Average stereo → mono, then 48 kHz → 16 kHz (miniaudio low-pass + linear resampler).
    static std::vector<float> Stereo48kToMono16k(const float* interleavedStereo, size_t frameCount);
};
