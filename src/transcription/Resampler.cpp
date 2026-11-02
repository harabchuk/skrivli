#include "transcription/Resampler.hpp"

#include "audio/MiniAudio.hpp"

#include <cstddef>
#include <vector>

namespace {

constexpr uint32_t kInputRate = 48000;
constexpr uint32_t kOutputRate = 16000;
constexpr uint32_t kLpfOrder = 8;

bool PushFrames(
    ma_data_converter& converter,
    const float* input,
    ma_uint64 inputChannels,
    ma_uint64& framesLeft,
    std::vector<float>& out,
    size_t& produced) {
    const float* cursor = input;
    while (framesLeft > 0) {
        ma_uint64 inCount = framesLeft;
        ma_uint64 outSpace = out.size() - produced;
        if (outSpace == 0) {
            out.resize(produced + static_cast<size_t>(framesLeft) + 64);
            outSpace = out.size() - produced;
        }
        const ma_result result = ma_data_converter_process_pcm_frames(
            &converter,
            cursor,
            &inCount,
            out.data() + produced,
            &outSpace);
        if (result != MA_SUCCESS) {
            return false;
        }
        if (inCount == 0 && outSpace == 0) {
            break;
        }
        cursor += static_cast<size_t>(inCount) * static_cast<size_t>(inputChannels);
        framesLeft -= inCount;
        produced += static_cast<size_t>(outSpace);
    }
    return true;
}

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

    ma_data_converter converter{};
    if (ma_data_converter_init(&config, nullptr, &converter) != MA_SUCCESS) {
        return {};
    }

    const size_t target = frameCount / 3;
    ma_uint64 expected = 0;
    if (ma_data_converter_get_expected_output_frame_count(&converter, frameCount, &expected) != MA_SUCCESS) {
        expected = static_cast<ma_uint64>(target);
    }
    const ma_uint64 latency = ma_data_converter_get_input_latency(&converter);

    std::vector<float> out(static_cast<size_t>(expected + latency + 64));
    ma_uint64 framesLeft = frameCount;
    size_t produced = 0;
    if (!PushFrames(converter, interleavedStereo, 2, framesLeft, out, produced)) {
        ma_data_converter_uninit(&converter, nullptr);
        return {};
    }

    // Pull the filter tail so a short clip is not missing its last output frames.
    if (latency > 0) {
        std::vector<float> silence(static_cast<size_t>(latency) * 2, 0.0f);
        ma_uint64 flushFrames = latency;
        if (!PushFrames(converter, silence.data(), 2, flushFrames, out, produced)) {
            ma_data_converter_uninit(&converter, nullptr);
            return {};
        }
    }

    ma_data_converter_uninit(&converter, nullptr);

    if (produced > target) {
        const size_t lead = produced - target;
        out.erase(out.begin(), out.begin() + static_cast<std::ptrdiff_t>(lead));
        produced = target;
    }
    out.resize(produced);
    return out;
}
