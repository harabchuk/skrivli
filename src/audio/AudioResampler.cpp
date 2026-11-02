#include "audio/AudioResampler.hpp"

#include "audio/AudioFormat.hpp"
#include "audio/MiniAudio.hpp"
#include "util/Log.hpp"

#include <mmreg.h>

#include <algorithm>
#include <cstddef>
#include <cstring>

namespace {

ma_format FormatFromWave(const WAVEFORMATEX* format) {
    if (format == nullptr) {
        return ma_format_unknown;
    }

    auto fromPcmBits = [](WORD bits) {
        switch (bits) {
        case 8:
            return ma_format_u8;
        case 16:
            return ma_format_s16;
        case 24:
            return ma_format_s24;
        case 32:
            return ma_format_s32;
        default:
            return ma_format_unknown;
        }
    };

    if (format->wFormatTag == WAVE_FORMAT_IEEE_FLOAT && format->wBitsPerSample == 32) {
        return ma_format_f32;
    }
    if (format->wFormatTag == WAVE_FORMAT_PCM) {
        return fromPcmBits(format->wBitsPerSample);
    }
    if (format->wFormatTag == WAVE_FORMAT_EXTENSIBLE
        && format->cbSize >= (sizeof(WAVEFORMATEXTENSIBLE) - sizeof(WAVEFORMATEX))) {
        const auto* extensible = reinterpret_cast<const WAVEFORMATEXTENSIBLE*>(format);
        const WORD bits = extensible->Samples.wValidBitsPerSample != 0
            ? extensible->Samples.wValidBitsPerSample
            : format->wBitsPerSample;
        if (extensible->SubFormat == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT && bits == 32) {
            return ma_format_f32;
        }
        if (extensible->SubFormat == KSDATAFORMAT_SUBTYPE_PCM) {
            return fromPcmBits(bits);
        }
    }
    return ma_format_unknown;
}

}  // namespace

struct AudioResampler::State {
    ma_data_converter converter{};
    uint32_t blockAlign = 0;
    bool live = false;
};

void AudioResampler::StateDeleter::operator()(State* state) const {
    if (state == nullptr) {
        return;
    }
    if (state->live) {
        ma_data_converter_uninit(&state->converter, nullptr);
        state->live = false;
    }
    delete state;
}

AudioResampler::~AudioResampler() {
    Reset();
}

void AudioResampler::Reset() {
    m_state.reset();
}

bool AudioResampler::Initialize(const void* inputFormatBytes) {
    Reset();
    const auto* inputFormat = static_cast<const WAVEFORMATEX*>(inputFormatBytes);
    if (inputFormat == nullptr || inputFormat->nChannels == 0 || inputFormat->nSamplesPerSec == 0) {
        return false;
    }

    const ma_format inputSampleFormat = FormatFromWave(inputFormat);
    if (inputSampleFormat == ma_format_unknown) {
        Log::Error(L"Mix format is not supported by the miniaudio resampler");
        return false;
    }

    const ma_uint32 bytesPerFrame = ma_get_bytes_per_frame(inputSampleFormat, inputFormat->nChannels);
    if (bytesPerFrame == 0 || inputFormat->nBlockAlign != bytesPerFrame) {
        Log::Error(L"Mix format block align does not match the miniaudio sample format");
        return false;
    }

    ma_channel inputMap[MA_MAX_CHANNELS]{};
    ma_channel outputMap[2]{};
    ma_channel_map_init_standard(
        ma_standard_channel_map_microsoft,
        inputMap,
        MA_MAX_CHANNELS,
        inputFormat->nChannels);
    ma_channel_map_init_standard(ma_standard_channel_map_microsoft, outputMap, 2, 2);

    ma_data_converter_config config = ma_data_converter_config_init(
        inputSampleFormat,
        ma_format_f32,
        inputFormat->nChannels,
        AudioFormat::kChannels,
        inputFormat->nSamplesPerSec,
        AudioFormat::kSampleRate);
    config.pChannelMapIn = inputMap;
    config.pChannelMapOut = outputMap;
    config.ditherMode = ma_dither_mode_none;
    config.resampling.linear.lpfOrder = 8;

    auto state = std::unique_ptr<State, StateDeleter>(new State());
    state->blockAlign = bytesPerFrame;
    const ma_result result = ma_data_converter_init(&config, nullptr, &state->converter);
    if (result != MA_SUCCESS) {
        Log::Error(L"miniaudio data converter init failed");
        return false;
    }
    state->live = true;
    m_state = std::move(state);
    Log::Info(L"Audio resampler ready (mix-format → 48 kHz stereo float)");
    return true;
}

bool AudioResampler::Process(
    const void* inputBytes,
    uint32_t inputByteCount,
    uint32_t inputFrames,
    std::vector<float>& outInterleavedStereo,
    uint32_t& outFrames) {
    outFrames = 0;
    outInterleavedStereo.clear();
    if (m_state == nullptr || !m_state->live || inputBytes == nullptr || inputByteCount == 0 || inputFrames == 0) {
        return false;
    }
    if (inputByteCount < inputFrames * m_state->blockAlign) {
        return false;
    }

    ma_uint64 expected = 0;
    if (ma_data_converter_get_expected_output_frame_count(&m_state->converter, inputFrames, &expected) != MA_SUCCESS
        || expected == 0) {
        expected = static_cast<ma_uint64>(inputFrames) + 256;
    }
    expected += 64;
    outInterleavedStereo.resize(static_cast<size_t>(expected) * AudioFormat::kChannels);

    const auto* cursor = static_cast<const std::byte*>(inputBytes);
    ma_uint64 framesLeft = inputFrames;
    size_t produced = 0;
    while (framesLeft > 0) {
        ma_uint64 inCount = framesLeft;
        ma_uint64 outSpace = (outInterleavedStereo.size() / AudioFormat::kChannels) - produced;
        if (outSpace == 0) {
            outInterleavedStereo.resize(outInterleavedStereo.size() + static_cast<size_t>(AudioFormat::kChannels) * 1024);
            outSpace = (outInterleavedStereo.size() / AudioFormat::kChannels) - produced;
        }

        const ma_result result = ma_data_converter_process_pcm_frames(
            &m_state->converter,
            cursor,
            &inCount,
            outInterleavedStereo.data() + produced * AudioFormat::kChannels,
            &outSpace);
        if (result != MA_SUCCESS) {
            outInterleavedStereo.clear();
            return false;
        }
        if (inCount == 0 && outSpace == 0) {
            break;
        }
        cursor += static_cast<size_t>(inCount) * m_state->blockAlign;
        framesLeft -= inCount;
        produced += static_cast<size_t>(outSpace);
    }

    outInterleavedStereo.resize(produced * AudioFormat::kChannels);
    outFrames = static_cast<uint32_t>(produced);
    return true;
}
