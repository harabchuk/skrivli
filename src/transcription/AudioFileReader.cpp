#include "transcription/AudioFileReader.hpp"

#include "storage/RecordingSidecar.hpp"
#include "util/Log.hpp"

#include <mfapi.h>
#include <mfidl.h>
#include <mfreadwrite.h>
#include <wrl/client.h>

#include <algorithm>
#include <cmath>
#include <cstring>
#include <filesystem>
#include <fstream>
#include <vector>

namespace {

bool EndsWithIgnoreCase(const std::wstring& value, const wchar_t* suffix) {
    const size_t len = wcslen(suffix);
    if (value.size() < len) {
        return false;
    }
    return _wcsicmp(value.c_str() + (value.size() - len), suffix) == 0;
}

void ResampleLinearStereo(
    const std::vector<float>& in,
    uint32_t inRate,
    uint16_t inChannels,
    std::vector<float>& out48Stereo) {
    if (inRate == 0 || inChannels == 0 || in.empty()) {
        out48Stereo.clear();
        return;
    }

    const size_t inFrames = in.size() / inChannels;
    if (inFrames == 0) {
        out48Stereo.clear();
        return;
    }

    if (inRate == 48000 && inChannels == 2) {
        out48Stereo = in;
        return;
    }

    const double outFramesD = static_cast<double>(inFrames) * 48000.0 / static_cast<double>(inRate);
    const size_t outFrames = static_cast<size_t>((std::max)(1.0, static_cast<double>(std::llround(outFramesD))));
    out48Stereo.assign(outFrames * 2, 0.0f);

    for (size_t of = 0; of < outFrames; ++of) {
        const double src = static_cast<double>(of) * static_cast<double>(inRate) / 48000.0;
        const size_t i0 = static_cast<size_t>(src);
        const size_t i1 = (std::min)(i0 + 1, inFrames - 1);
        const float t = static_cast<float>(src - static_cast<double>(i0));

        float l0 = 0.0f;
        float r0 = 0.0f;
        float l1 = 0.0f;
        float r1 = 0.0f;
        if (inChannels == 1) {
            l0 = r0 = in[i0];
            l1 = r1 = in[i1];
        } else {
            l0 = in[i0 * inChannels];
            r0 = in[i0 * inChannels + 1];
            l1 = in[i1 * inChannels];
            r1 = in[i1 * inChannels + 1];
        }
        out48Stereo[of * 2] = l0 + (l1 - l0) * t;
        out48Stereo[of * 2 + 1] = r0 + (r1 - r0) * t;
    }
}

bool ReadWavFile(const std::wstring& path, DecodedAudio& out, std::wstring& error) {
    std::ifstream file(path, std::ios::binary);
    if (!file) {
        error = L"Could not open WAV file.";
        return false;
    }

    file.seekg(0, std::ios::end);
    const std::streamoff size = file.tellg();
    if (size < 44) {
        error = L"WAV file is too small.";
        return false;
    }
    file.seekg(0, std::ios::beg);

    std::vector<char> bytes(static_cast<size_t>(size));
    file.read(bytes.data(), size);
    if (!file) {
        error = L"Failed to read WAV file.";
        return false;
    }

    if (std::memcmp(bytes.data(), "RIFF", 4) != 0 || std::memcmp(bytes.data() + 8, "WAVE", 4) != 0) {
        error = L"Not a RIFF/WAVE file.";
        return false;
    }

    uint16_t audioFormat = 0;
    uint16_t channels = 0;
    uint32_t sampleRate = 0;
    uint16_t bitsPerSample = 0;
    const char* dataPtr = nullptr;
    uint32_t dataSize = 0;

    size_t offset = 12;
    while (offset + 8 <= bytes.size()) {
        const char* chunkId = bytes.data() + offset;
        uint32_t chunkSize = 0;
        std::memcpy(&chunkSize, bytes.data() + offset + 4, 4);
        const size_t chunkData = offset + 8;
        if (chunkData + chunkSize > bytes.size()) {
            break;
        }

        if (std::memcmp(chunkId, "fmt ", 4) == 0 && chunkSize >= 16) {
            std::memcpy(&audioFormat, bytes.data() + chunkData, 2);
            std::memcpy(&channels, bytes.data() + chunkData + 2, 2);
            std::memcpy(&sampleRate, bytes.data() + chunkData + 4, 4);
            std::memcpy(&bitsPerSample, bytes.data() + chunkData + 14, 2);
        } else if (std::memcmp(chunkId, "data", 4) == 0) {
            dataPtr = bytes.data() + chunkData;
            dataSize = chunkSize;
            break;
        }

        offset = chunkData + chunkSize + (chunkSize & 1u);
    }

    if (dataPtr == nullptr || channels == 0 || sampleRate == 0 || bitsPerSample == 0) {
        error = L"WAV is missing fmt/data chunks.";
        return false;
    }

    std::vector<float> interleaved;
    if (audioFormat == 1 && bitsPerSample == 16) {
        const size_t sampleCount = dataSize / 2;
        interleaved.resize(sampleCount);
        for (size_t i = 0; i < sampleCount; ++i) {
            int16_t sample = 0;
            std::memcpy(&sample, dataPtr + i * 2, 2);
            interleaved[i] = static_cast<float>(sample) / 32768.0f;
        }
    } else if (audioFormat == 3 && bitsPerSample == 32) {
        const size_t sampleCount = dataSize / 4;
        interleaved.resize(sampleCount);
        std::memcpy(interleaved.data(), dataPtr, sampleCount * 4);
    } else {
        error = L"Unsupported WAV format (need PCM16 or float32).";
        return false;
    }

    const size_t frames = interleaved.size() / channels;
    out.durationSeconds = static_cast<double>(frames) / static_cast<double>(sampleRate);
    out.sampleRate = 48000;
    out.channels = 2;
    ResampleLinearStereo(interleaved, sampleRate, channels, out.interleavedStereo);
    out.durationSeconds =
        static_cast<double>(out.interleavedStereo.size() / 2) / 48000.0;
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

    out.sampleRate = 48000;
    out.channels = 2;
    ResampleLinearStereo(interleaved, sampleRate, static_cast<uint16_t>(channels), out.interleavedStereo);
    out.durationSeconds = static_cast<double>(out.interleavedStereo.size() / 2) / 48000.0;
    return true;
}

}  // namespace

bool AudioFileReader::Read(const std::wstring& path, DecodedAudio& out, std::wstring& error) {
    out = {};
    if (path.empty() || !std::filesystem::exists(path)) {
        error = L"Audio file not found.";
        return false;
    }

    if (EndsWithIgnoreCase(path, L".wav")) {
        return ReadWavFile(path, out, error);
    }
    if (EndsWithIgnoreCase(path, L".m4a") || EndsWithIgnoreCase(path, L".mp4") || EndsWithIgnoreCase(path, L".aac")) {
        return ReadWithMediaFoundation(path, out, error);
    }

    error = L"Unsupported audio format.";
    return false;
}
