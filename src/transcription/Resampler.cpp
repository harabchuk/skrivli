#include "transcription/Resampler.hpp"

#include <cmath>
#include <cstddef>

namespace {

constexpr int kTaps = 63; // odd length
constexpr double kPi = 3.14159265358979323846;

// Low-pass cutoff ≈ 0.9 * 8 kHz / 48 kHz so the 16 kHz Nyquist is preserved.
double Sinc(double x) {
    if (std::abs(x) < 1e-8) {
        return 1.0;
    }
    return std::sin(kPi * x) / (kPi * x);
}

void BuildKernel(float kernel[kTaps]) {
    constexpr double fc = 0.15; // cycles per input sample
    const int mid = kTaps / 2;
    double sum = 0.0;
    for (int i = 0; i < kTaps; ++i) {
        const double n = static_cast<double>(i - mid);
        const double sinc = 2.0 * fc * Sinc(2.0 * fc * n);
        const double window = 0.54 - 0.46 * std::cos(2.0 * kPi * static_cast<double>(i) / static_cast<double>(kTaps - 1));
        const double v = sinc * window;
        kernel[i] = static_cast<float>(v);
        sum += v;
    }
    for (int i = 0; i < kTaps; ++i) {
        kernel[i] = static_cast<float>(kernel[i] / sum);
    }
}

}  // namespace

std::vector<float> Resampler::Stereo48kToMono16k(const float* interleavedStereo, size_t frameCount) {
    if (interleavedStereo == nullptr || frameCount == 0) {
        return {};
    }

    std::vector<float> mono(frameCount);
    for (size_t i = 0; i < frameCount; ++i) {
        mono[i] = 0.5f * (interleavedStereo[i * 2] + interleavedStereo[i * 2 + 1]);
    }

    float kernel[kTaps]{};
    BuildKernel(kernel);
    const int mid = kTaps / 2;

    const size_t outCount = frameCount / 3;
    std::vector<float> out(outCount);
    for (size_t o = 0; o < outCount; ++o) {
        const size_t center = o * 3;
        double acc = 0.0;
        for (int t = 0; t < kTaps; ++t) {
            const int64_t idx = static_cast<int64_t>(center) + (t - mid);
            float sample = 0.0f;
            if (idx >= 0 && static_cast<size_t>(idx) < frameCount) {
                sample = mono[static_cast<size_t>(idx)];
            }
            acc += static_cast<double>(sample) * static_cast<double>(kernel[t]);
        }
        out[o] = static_cast<float>(acc);
    }
    return out;
}
