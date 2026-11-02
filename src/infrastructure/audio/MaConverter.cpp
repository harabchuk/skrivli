#include "infrastructure/audio/MaConverter.hpp"

namespace {

constexpr size_t kGrowthFrames = 64;

}  // namespace

MaConverter::~MaConverter() {
    Uninit();
}

void MaConverter::Uninit() {
    if (m_live) {
        ma_data_converter_uninit(&m_converter, nullptr);
        m_live = false;
    }
}

bool MaConverter::Init(const ma_data_converter_config& config) {
    Uninit();
    if (config.formatOut != ma_format_f32) {
        return false;
    }
    if (ma_data_converter_init(&config, nullptr, &m_converter) != MA_SUCCESS) {
        return false;
    }
    m_live = true;
    m_bytesPerInputFrame = ma_get_bytes_per_frame(config.formatIn, config.channelsIn);
    m_channelsOut = config.channelsOut;
    return true;
}

ma_uint64 MaConverter::InputLatency() const {
    return m_live ? ma_data_converter_get_input_latency(&m_converter) : 0;
}

ma_uint64 MaConverter::ExpectedOutputFrames(ma_uint64 inputFrames, ma_uint64 fallback) const {
    ma_uint64 expected = 0;
    if (!m_live
        || ma_data_converter_get_expected_output_frame_count(&m_converter, inputFrames, &expected) != MA_SUCCESS) {
        return fallback;
    }
    return expected;
}

bool MaConverter::Push(const void* input, ma_uint64 frames, std::vector<float>& out, size_t& produced) {
    if (!m_live) {
        return false;
    }

    const auto* cursor = static_cast<const std::byte*>(input);
    ma_uint64 framesLeft = frames;
    while (framesLeft > 0) {
        ma_uint64 inCount = framesLeft;
        ma_uint64 outSpace = (out.size() / m_channelsOut) - produced;
        if (outSpace == 0) {
            out.resize((produced + static_cast<size_t>(framesLeft) + kGrowthFrames) * m_channelsOut);
            outSpace = (out.size() / m_channelsOut) - produced;
        }
        const ma_result result = ma_data_converter_process_pcm_frames(
            &m_converter,
            cursor,
            &inCount,
            out.data() + produced * m_channelsOut,
            &outSpace);
        if (result != MA_SUCCESS) {
            return false;
        }
        if (inCount == 0 && outSpace == 0) {
            break;
        }
        cursor += static_cast<size_t>(inCount) * m_bytesPerInputFrame;
        framesLeft -= inCount;
        produced += static_cast<size_t>(outSpace);
    }
    return true;
}

bool MaConverter::FlushLatency(std::vector<float>& out, size_t& produced) {
    const ma_uint64 latency = InputLatency();
    if (latency == 0) {
        return true;
    }
    const std::vector<std::byte> silence(static_cast<size_t>(latency) * m_bytesPerInputFrame, std::byte{0});
    return Push(silence.data(), latency, out, produced);
}
