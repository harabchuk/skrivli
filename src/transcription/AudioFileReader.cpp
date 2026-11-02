#include "transcription/AudioFileReader.hpp"

#include "audio/MiniAudio.hpp"
#include "util/Text.hpp"

#include <mfapi.h>
#include <mfidl.h>
#include <mfreadwrite.h>
#include <wrl/client.h>

#include <algorithm>
#include <cstring>
#include <filesystem>
#include <vector>

namespace {

bool ConvertToStereo48k(
    const float* interleaved,
    size_t frameCount,
    uint32_t sampleRate,
    uint32_t channels,
    std::vector<float>& outStereo,
    std::wstring& error) {
    outStereo.clear();
    if (frameCount == 0 || channels == 0 || sampleRate == 0) {
        return true;
    }
    if (channels > MA_MAX_CHANNELS) {
        error = L"Audio file has too many channels.";
        return false;
    }
    if (sampleRate == 48000 && channels == 2) {
        outStereo.assign(interleaved, interleaved + frameCount * 2);
        return true;
    }

    ma_data_converter_config config = ma_data_converter_config_init(
        ma_format_f32,
        ma_format_f32,
        channels,
        2,
        sampleRate,
        48000);
    config.ditherMode = ma_dither_mode_none;
    config.resampling.linear.lpfOrder = 8;

    ma_data_converter converter{};
    if (ma_data_converter_init(&config, nullptr, &converter) != MA_SUCCESS) {
        error = L"Could not resample decoded audio.";
        return false;
    }

    ma_uint64 expected = 0;
    if (ma_data_converter_get_expected_output_frame_count(&converter, frameCount, &expected) != MA_SUCCESS) {
        expected = frameCount;
    }
    const ma_uint64 latency = ma_data_converter_get_input_latency(&converter);
    outStereo.resize(static_cast<size_t>(expected + latency + 64) * 2);

    ma_uint64 framesLeft = frameCount;
    size_t produced = 0;
    const float* cursor = interleaved;
    while (framesLeft > 0) {
        ma_uint64 inCount = framesLeft;
        ma_uint64 outSpace = (outStereo.size() / 2) - produced;
        if (outSpace == 0) {
            outStereo.resize(outStereo.size() + 4096);
            outSpace = (outStereo.size() / 2) - produced;
        }
        const ma_result result = ma_data_converter_process_pcm_frames(
            &converter,
            cursor,
            &inCount,
            outStereo.data() + produced * 2,
            &outSpace);
        if (result != MA_SUCCESS) {
            ma_data_converter_uninit(&converter, nullptr);
            error = L"Could not resample decoded audio.";
            outStereo.clear();
            return false;
        }
        if (inCount == 0 && outSpace == 0) {
            break;
        }
        cursor += static_cast<size_t>(inCount) * channels;
        framesLeft -= inCount;
        produced += static_cast<size_t>(outSpace);
    }

    if (latency > 0) {
        std::vector<float> silence(static_cast<size_t>(latency) * channels, 0.0f);
        ma_uint64 flushFrames = latency;
        const float* silenceCursor = silence.data();
        while (flushFrames > 0) {
            ma_uint64 inCount = flushFrames;
            ma_uint64 outSpace = (outStereo.size() / 2) - produced;
            if (outSpace == 0) {
                outStereo.resize(outStereo.size() + 4096);
                outSpace = (outStereo.size() / 2) - produced;
            }
            const ma_result result = ma_data_converter_process_pcm_frames(
                &converter,
                silenceCursor,
                &inCount,
                outStereo.data() + produced * 2,
                &outSpace);
            if (result != MA_SUCCESS || (inCount == 0 && outSpace == 0)) {
                break;
            }
            silenceCursor += static_cast<size_t>(inCount) * channels;
            flushFrames -= inCount;
            produced += static_cast<size_t>(outSpace);
        }
    }

    ma_data_converter_uninit(&converter, nullptr);
    outStereo.resize(produced * 2);
    return true;
}

void FinishDecoded(DecodedAudio& out) {
    out.sampleRate = 48000;
    out.channels = 2;
    out.durationSeconds = static_cast<double>(out.interleavedStereo.size() / 2) / 48000.0;
}

bool ReadWithMiniaudio(const std::wstring& path, DecodedAudio& out, std::wstring& error) {
    ma_decoder_config config = ma_decoder_config_init(ma_format_f32, 2, 48000);
    config.ditherMode = ma_dither_mode_none;
    config.resampling.linear.lpfOrder = 8;

    ma_decoder decoder{};
    if (ma_decoder_init_file_w(path.c_str(), &config, &decoder) != MA_SUCCESS) {
        error = L"Could not open audio file.";
        return false;
    }

    constexpr ma_uint64 kChunkFrames = 48000;
    std::vector<float> chunk(static_cast<size_t>(kChunkFrames) * 2);
    ma_uint64 total = 0;
    for (;;) {
        ma_uint64 framesRead = 0;
        const ma_result result = ma_decoder_read_pcm_frames(&decoder, chunk.data(), kChunkFrames, &framesRead);
        if (framesRead > 0) {
            const size_t samples = static_cast<size_t>(framesRead) * 2;
            out.interleavedStereo.resize(static_cast<size_t>(total) * 2 + samples);
            std::memcpy(out.interleavedStereo.data() + static_cast<size_t>(total) * 2, chunk.data(), samples * sizeof(float));
            total += framesRead;
        }
        if (framesRead == 0 || framesRead < kChunkFrames || result == MA_AT_END) {
            if (result != MA_SUCCESS && result != MA_AT_END && framesRead == 0 && total == 0) {
                ma_decoder_uninit(&decoder);
                error = L"Failed to decode audio.";
                return false;
            }
            break;
        }
    }

    ma_decoder_uninit(&decoder);
    FinishDecoded(out);
    return true;
}

bool ReadWithMediaFoundation(const std::wstring& path, DecodedAudio& out, std::wstring& error) {
    HRESULT hr = ::MFStartup(MF_VERSION, MFSTARTUP_LITE);
    if (FAILED(hr)) {
        error = L"MFStartup failed.";
        return false;
    }

    bool ok = false;
    Microsoft::WRL::ComPtr<IMFSourceReader> reader;
    hr = ::MFCreateSourceReaderFromURL(path.c_str(), nullptr, &reader);
    if (FAILED(hr) || !reader) {
        error = L"Could not open media file.";
        ::MFShutdown();
        return false;
    }

    Microsoft::WRL::ComPtr<IMFMediaType> partial;
    hr = ::MFCreateMediaType(&partial);
    if (FAILED(hr)) {
        error = L"MFCreateMediaType failed.";
        ::MFShutdown();
        return false;
    }
    partial->SetGUID(MF_MT_MAJOR_TYPE, MFMediaType_Audio);
    partial->SetGUID(MF_MT_SUBTYPE, MFAudioFormat_Float);
    hr = reader->SetCurrentMediaType(static_cast<DWORD>(MF_SOURCE_READER_FIRST_AUDIO_STREAM), nullptr, partial.Get());
    if (FAILED(hr)) {
        error = L"Could not configure float audio decode.";
        ::MFShutdown();
        return false;
    }

    Microsoft::WRL::ComPtr<IMFMediaType> current;
    hr = reader->GetCurrentMediaType(static_cast<DWORD>(MF_SOURCE_READER_FIRST_AUDIO_STREAM), &current);
    if (FAILED(hr) || !current) {
        error = L"Could not read decoded audio format.";
        ::MFShutdown();
        return false;
    }

    UINT32 channels = 0;
    UINT32 sampleRate = 0;
    current->GetUINT32(MF_MT_AUDIO_NUM_CHANNELS, &channels);
    current->GetUINT32(MF_MT_AUDIO_SAMPLES_PER_SECOND, &sampleRate);
    if (channels == 0 || sampleRate == 0) {
        error = L"Invalid decoded audio format.";
        ::MFShutdown();
        return false;
    }

    std::vector<float> interleaved;
    for (;;) {
        DWORD streamIndex = 0;
        DWORD flags = 0;
        LONGLONG timestamp = 0;
        Microsoft::WRL::ComPtr<IMFSample> sample;
        hr = reader->ReadSample(
            static_cast<DWORD>(MF_SOURCE_READER_FIRST_AUDIO_STREAM),
            0,
            &streamIndex,
            &flags,
            &timestamp,
            &sample);
        if (FAILED(hr)) {
            error = L"ReadSample failed.";
            break;
        }
        if (flags & MF_SOURCE_READERF_ENDOFSTREAM) {
            ok = true;
            break;
        }
        if (!sample) {
            continue;
        }

        Microsoft::WRL::ComPtr<IMFMediaBuffer> buffer;
        hr = sample->ConvertToContiguousBuffer(&buffer);
        if (FAILED(hr) || !buffer) {
            continue;
        }
        BYTE* data = nullptr;
        DWORD maxLen = 0;
        DWORD curLen = 0;
        hr = buffer->Lock(&data, &maxLen, &curLen);
        if (FAILED(hr) || data == nullptr || curLen == 0) {
            continue;
        }
        const size_t floatCount = curLen / sizeof(float);
        const size_t old = interleaved.size();
        interleaved.resize(old + floatCount);
        std::memcpy(interleaved.data() + old, data, floatCount * sizeof(float));
        buffer->Unlock();
    }

    ::MFShutdown();
    if (!ok) {
        return false;
    }

    const size_t frames = interleaved.size() / channels;
    if (!ConvertToStereo48k(interleaved.data(), frames, sampleRate, channels, out.interleavedStereo, error)) {
        return false;
    }
    FinishDecoded(out);
    return true;
}

}  // namespace

bool AudioFileReader::Read(const std::wstring& path, DecodedAudio& out, std::wstring& error) {
    out = {};
    if (path.empty() || !std::filesystem::exists(path)) {
        error = L"Audio file not found.";
        return false;
    }

    if (Text::EndsWithIgnoreCase(path, L".wav") || Text::EndsWithIgnoreCase(path, L".flac") || Text::EndsWithIgnoreCase(path, L".mp3")) {
        return ReadWithMiniaudio(path, out, error);
    }
    if (Text::EndsWithIgnoreCase(path, L".m4a") || Text::EndsWithIgnoreCase(path, L".mp4") || Text::EndsWithIgnoreCase(path, L".aac")) {
        return ReadWithMediaFoundation(path, out, error);
    }

    error = L"Unsupported audio format.";
    return false;
}
