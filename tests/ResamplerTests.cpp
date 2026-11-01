#include "transcription/Resampler.hpp"

#include <gtest/gtest.h>

#include <cmath>
#include <vector>

namespace {

constexpr double kPi = 3.14159265358979323846;

std::vector<float> MakeStereoSine(double freqHz, double seconds, float amplitude) {
    const size_t frames = static_cast<size_t>(seconds * 48000.0);
    std::vector<float> stereo(frames * 2);
    for (size_t i = 0; i < frames; ++i) {
        const float s = amplitude * static_cast<float>(std::sin(2.0 * kPi * freqHz * static_cast<double>(i) / 48000.0));
        stereo[i * 2] = s;
        stereo[i * 2 + 1] = s;
    }
    return stereo;
}

double Rms(const std::vector<float>& samples) {
    if (samples.empty()) {
        return 0.0;
    }
    double sum = 0.0;
    for (float s : samples) {
        sum += static_cast<double>(s) * static_cast<double>(s);
    }
    return std::sqrt(sum / static_cast<double>(samples.size()));
}

}  // namespace

TEST(Resampler, OneKHzSineKeepsEnergy) {
    const auto stereo = MakeStereoSine(1000.0, 0.25, 0.5f);
    const auto mono = Resampler::Stereo48kToMono16k(stereo.data(), stereo.size() / 2);
    ASSERT_FALSE(mono.empty());
    EXPECT_NEAR(static_cast<double>(mono.size()), 0.25 * 16000.0, 2.0);

    const double inRms = 0.5 / std::sqrt(2.0);
    const double outRms = Rms(mono);
    EXPECT_NEAR(outRms, inRms, 0.08);
}

TEST(Resampler, TenKHzToneIsAttenuated) {
    const auto stereo = MakeStereoSine(10000.0, 0.25, 0.5f);
    const auto mono = Resampler::Stereo48kToMono16k(stereo.data(), stereo.size() / 2);
    ASSERT_FALSE(mono.empty());
    const double outRms = Rms(mono);
    EXPECT_LT(outRms, 0.08);
}
