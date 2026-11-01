#pragma once

#include <cstdint>
#include <vector>

class Resampler {
public:
    // Average stereo → mono, then 48 kHz → 16 kHz (windowed-sinc FIR + decimate by 3).
    static std::vector<float> Stereo48kToMono16k(const float* interleavedStereo, size_t frameCount);
};
