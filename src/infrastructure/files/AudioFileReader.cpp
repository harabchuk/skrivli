#include "infrastructure/files/AudioFileReader.hpp"

#include "infrastructure/audio/MaConverter.hpp"
#include "infrastructure/audio/MiniAudio.hpp"
#include "infrastructure/common/Text.hpp"

#include <algorithm>
#include <cstring>
#include <filesystem>
#include <mfapi.h>
#include <mfidl.h>
#include <mfreadwrite.h>
#include <vector>
#include <wil/com.h>

namespace {

bool ConvertToStereo48k(const float* interleaved, size_t frameCount, uint32_t sampleRate,
                        uint32_t channels, std::vector<float>& outStereo, std::wstring& error) {
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

    ma_data_converter_config config =
        ma_data_converter_config_init(ma_format_f32, ma_format_f32, channels, 2, sampleRate, 48000);
    config.ditherMode = ma_dither_mode_none;
    config.resampling.linear.lpfOrder = 8;

    MaConverter converter;
    if (!converter.Init(config)) {
        error = L"Could not resample decoded audio.";
        return false;
    }

    const ma_uint64 expected = converter.ExpectedOutputFrames(frameCount, frameCount);
    const ma_uint64 latency = converter.InputLatency();
    outStereo.resize(static_cast<size_t>(expected + latency + 64) * 2);

    size_t produced = 0;
    if (!converter.Push(interleaved, frameCount, outStereo, produced)) {
        error = L"Could not resample decoded audio.";
        outStereo.clear();
        return false;
    }

    // A failed tail flush only drops the last few output frames, so keep what was converted.
    (void)converter.FlushLatency(outStereo, produced);

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
        const ma_result result =
            ma_decoder_read_pcm_frames(&decoder, chunk.data(), kChunkFrames, &framesRead);
        if (framesRead > 0) {
            const size_t samples = static_cast<size_t>(framesRead) * 2;
            out.interleavedStereo.resize(static_cast<size_t>(total) * 2 + samples);
            std::memcpy(out.interleavedStereo.data() + static_cast<size_t>(total) * 2, chunk.data(),
                        samples * sizeof(float));
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
    wil::com_ptr_nothrow<IMFSourceReader> reader;
    hr = ::MFCreateSourceReaderFromURL(path.c_str(), nullptr, reader.put());
    if (FAILED(hr) || !reader) {
        error = L"Could not open media file.";
        ::MFShutdown();
        return false;
    }

    wil::com_ptr_nothrow<IMFMediaType> partial;
    hr = ::MFCreateMediaType(partial.put());
    if (FAILED(hr)) {
        error = L"MFCreateMediaType failed.";
        ::MFShutdown();
        return false;
    }
    partial->SetGUID(MF_MT_MAJOR_TYPE, MFMediaType_Audio);
    partial->SetGUID(MF_MT_SUBTYPE, MFAudioFormat_Float);
    hr = reader->SetCurrentMediaType(static_cast<DWORD>(MF_SOURCE_READER_FIRST_AUDIO_STREAM),
                                     nullptr, partial.get());
    if (FAILED(hr)) {
        error = L"Could not configure float audio decode.";
        ::MFShutdown();
        return false;
    }

    wil::com_ptr_nothrow<IMFMediaType> current;
    hr = reader->GetCurrentMediaType(static_cast<DWORD>(MF_SOURCE_READER_FIRST_AUDIO_STREAM),
                                     current.put());
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
        wil::com_ptr_nothrow<IMFSample> sample;
        hr = reader->ReadSample(static_cast<DWORD>(MF_SOURCE_READER_FIRST_AUDIO_STREAM), 0,
                                &streamIndex, &flags, &timestamp, sample.put());
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

        wil::com_ptr_nothrow<IMFMediaBuffer> buffer;
        hr = sample->ConvertToContiguousBuffer(buffer.put());
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
    if (!ConvertToStereo48k(interleaved.data(), frames, sampleRate, channels, out.interleavedStereo,
                            error)) {
        return false;
    }
    FinishDecoded(out);
    return true;
}

} // namespace

bool AudioFileReader::Read(const std::wstring& path, DecodedAudio& out, std::wstring& error) {
    out = {};
    if (path.empty() || !std::filesystem::exists(path)) {
        error = L"Audio file not found.";
        return false;
    }

    if (Text::EndsWithIgnoreCase(path, L".wav") || Text::EndsWithIgnoreCase(path, L".flac") ||
        Text::EndsWithIgnoreCase(path, L".mp3")) {
        return ReadWithMiniaudio(path, out, error);
    }
    if (Text::EndsWithIgnoreCase(path, L".m4a") || Text::EndsWithIgnoreCase(path, L".mp4") ||
        Text::EndsWithIgnoreCase(path, L".aac")) {
        return ReadWithMediaFoundation(path, out, error);
    }

    error = L"Unsupported audio format.";
    return false;
}
