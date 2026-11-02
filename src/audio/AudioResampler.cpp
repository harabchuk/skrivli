#include "audio/AudioResampler.hpp"

#include "audio/AudioFormat.hpp"
#include "audio/MaConverter.hpp"
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
    MaConverter converter;
    uint32_t blockAlign = 0;
};

void AudioResampler::StateDeleter::operator()(State* state) const {
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
    if (!state->converter.Init(config)) {
        Log::Error(L"miniaudio data converter init failed");
        return false;
    }
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
    if (m_state == nullptr || inputBytes == nullptr || inputByteCount == 0 || inputFrames == 0) {
        return false;
    }
    if (inputByteCount < inputFrames * m_state->blockAlign) {
        return false;
    }

    ma_uint64 expected = m_state->converter.ExpectedOutputFrames(inputFrames, 0);
    if (expected == 0) {
        expected = static_cast<ma_uint64>(inputFrames) + 256;
    }
    expected += 64;
    outInterleavedStereo.resize(static_cast<size_t>(expected) * AudioFormat::kChannels);

    size_t produced = 0;
    if (!m_state->converter.Push(inputBytes, inputFrames, outInterleavedStereo, produced)) {
        outInterleavedStereo.clear();
        return false;
    }

    outInterleavedStereo.resize(produced * AudioFormat::kChannels);
    outFrames = static_cast<uint32_t>(produced);
    return true;
}
