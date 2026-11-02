#include "transcription/Resampler.hpp"

#include "audio/MaConverter.hpp"
#include "audio/MiniAudio.hpp"

#include <cstddef>
#include <vector>

namespace {

constexpr uint32_t kInputRate = 48000;
constexpr uint32_t kOutputRate = 16000;
constexpr uint32_t kLpfOrder = 8;

}  // namespace

std::vector<float> Resampler::Stereo48kToMono16k(const float* interleavedStereo, size_t frameCount) {
    if (interleavedStereo == nullptr || frameCount == 0) {
        return {};
    }

    ma_data_converter_config config = ma_data_converter_config_init(
        ma_format_f32,
        ma_format_f32,
        2,
        1,
        kInputRate,
        kOutputRate);
    config.channelMixMode = ma_channel_mix_mode_rectangular;
    config.ditherMode = ma_dither_mode_none;
    // Order 8 is the highest Butterworth miniaudio will build. It is what keeps a
    // 10 kHz tone under the 16 kHz Nyquist instead of aliasing through linear interpolation.
    config.resampling.linear.lpfOrder = kLpfOrder;

    MaConverter converter;
    if (!converter.Init(config)) {
        return {};
    }

    const size_t target = frameCount / 3;
    const ma_uint64 expected = converter.ExpectedOutputFrames(frameCount, static_cast<ma_uint64>(target));
    const ma_uint64 latency = converter.InputLatency();

    std::vector<float> out(static_cast<size_t>(expected + latency + 64));
    size_t produced = 0;
    if (!converter.Push(interleavedStereo, frameCount, out, produced)) {
        return {};
    }

    // Pull the filter tail so a short clip is not missing its last output frames.
    if (!converter.FlushLatency(out, produced)) {
        return {};
    }

    if (produced > target) {
        const size_t lead = produced - target;
        out.erase(out.begin(), out.begin() + static_cast<std::ptrdiff_t>(lead));
        produced = target;
    }
    out.resize(produced);
    return out;
}
