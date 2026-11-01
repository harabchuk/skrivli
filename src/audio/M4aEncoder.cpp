#include "audio/M4aEncoder.hpp"

#include "util/Log.hpp"

#include <mfapi.h>
#include <mfidl.h>
#include <mfreadwrite.h>
#include <wmcodecdsp.h>
#include <wrl/client.h>

#include <algorithm>
#include <cstdio>
#include <cstring>
#include <filesystem>
#include <vector>

namespace {

constexpr UINT32 kSampleRate = 48000;
constexpr UINT32 kChannels = 2;
constexpr UINT32 kBitsPerSample = 16;
constexpr UINT32 kBytesPerSecond = kSampleRate * kChannels * (kBitsPerSample / 8);
constexpr UINT32 kAacBitrate = 128000;
constexpr UINT32 kChunkFrames = 4800; // 100 ms

bool ReadWavPcm16Stereo48k(const std::wstring& path, std::vector<int16_t>& pcm, std::wstring& error) {
    HANDLE file = ::CreateFileW(
        path.c_str(),
        GENERIC_READ,
        FILE_SHARE_READ,
        nullptr,
        OPEN_EXISTING,
        FILE_ATTRIBUTE_NORMAL,
        nullptr);
    if (file == INVALID_HANDLE_VALUE) {
        error = L"Could not open WAV for compression.";
        return false;
    }

    LARGE_INTEGER size{};
    if (!::GetFileSizeEx(file, &size) || size.QuadPart < 44) {
        ::CloseHandle(file);
        error = L"WAV file is too short.";
        return false;
    }

    std::vector<char> header(44);
    DWORD read = 0;
    if (!::ReadFile(file, header.data(), 44, &read, nullptr) || read != 44) {
        ::CloseHandle(file);
        error = L"Failed to read WAV header.";
        return false;
    }

    if (std::strncmp(header.data(), "RIFF", 4) != 0 || std::strncmp(header.data() + 8, "WAVE", 4) != 0) {
        ::CloseHandle(file);
        error = L"Not a RIFF/WAVE file.";
        return false;
    }

    uint16_t format = 0;
    uint16_t channels = 0;
    uint32_t sampleRate = 0;
    uint16_t bits = 0;
    uint32_t dataSize = 0;
    std::memcpy(&format, header.data() + 20, sizeof(format));
    std::memcpy(&channels, header.data() + 22, sizeof(channels));
    std::memcpy(&sampleRate, header.data() + 24, sizeof(sampleRate));
    std::memcpy(&bits, header.data() + 34, sizeof(bits));
    std::memcpy(&dataSize, header.data() + 40, sizeof(dataSize));

    // Walk chunks if the simple layout is not "fmt " then "data" at fixed offsets.
    // Our WavWriter uses the canonical 44-byte header; accept that path.
    if (format != 1 || channels != kChannels || sampleRate != kSampleRate || bits != kBitsPerSample) {
        ::CloseHandle(file);
        error = L"WAV must be PCM 16-bit stereo 48 kHz.";
        return false;
    }

    const uint64_t remaining = static_cast<uint64_t>(size.QuadPart) - 44;
    const uint64_t bytes = (std::min)(remaining, static_cast<uint64_t>(dataSize));
    if (bytes % 4 != 0) {
        ::CloseHandle(file);
        error = L"WAV data size is not aligned.";
        return false;
    }

    pcm.resize(static_cast<size_t>(bytes / sizeof(int16_t)));
    if (!pcm.empty()) {
        if (!::ReadFile(file, pcm.data(), static_cast<DWORD>(bytes), &read, nullptr) || read != bytes) {
            ::CloseHandle(file);
            error = L"Failed to read WAV PCM data.";
            return false;
        }
    }
    ::CloseHandle(file);
    return true;
}

HRESULT ConfigureSinkWriter(
    IMFSinkWriter* writer,
    DWORD* streamIndex) {
    Microsoft::WRL::ComPtr<IMFMediaType> outType;
    HRESULT hr = ::MFCreateMediaType(&outType);
    if (FAILED(hr)) {
        return hr;
    }
    hr = outType->SetGUID(MF_MT_MAJOR_TYPE, MFMediaType_Audio);
    if (FAILED(hr)) {
        return hr;
    }
    hr = outType->SetGUID(MF_MT_SUBTYPE, MFAudioFormat_AAC);
    if (FAILED(hr)) {
        return hr;
    }
    hr = outType->SetUINT32(MF_MT_AUDIO_SAMPLES_PER_SECOND, kSampleRate);
    if (FAILED(hr)) {
        return hr;
    }
    hr = outType->SetUINT32(MF_MT_AUDIO_NUM_CHANNELS, kChannels);
    if (FAILED(hr)) {
        return hr;
    }
    hr = outType->SetUINT32(MF_MT_AUDIO_BITS_PER_SAMPLE, 16);
    if (FAILED(hr)) {
        return hr;
    }
    hr = outType->SetUINT32(MF_MT_AUDIO_AVG_BYTES_PER_SECOND, kAacBitrate / 8);
    if (FAILED(hr)) {
        return hr;
    }
    // AAC profile LC
    hr = outType->SetUINT32(MF_MT_AAC_AUDIO_PROFILE_LEVEL_INDICATION, 0x29);
    if (FAILED(hr)) {
        return hr;
    }

    hr = writer->AddStream(outType.Get(), streamIndex);
    if (FAILED(hr)) {
        return hr;
    }

    Microsoft::WRL::ComPtr<IMFMediaType> inType;
    hr = ::MFCreateMediaType(&inType);
    if (FAILED(hr)) {
        return hr;
    }
    hr = inType->SetGUID(MF_MT_MAJOR_TYPE, MFMediaType_Audio);
    if (FAILED(hr)) {
        return hr;
    }
    hr = inType->SetGUID(MF_MT_SUBTYPE, MFAudioFormat_PCM);
    if (FAILED(hr)) {
        return hr;
    }
    hr = inType->SetUINT32(MF_MT_AUDIO_NUM_CHANNELS, kChannels);
    if (FAILED(hr)) {
        return hr;
    }
    hr = inType->SetUINT32(MF_MT_AUDIO_SAMPLES_PER_SECOND, kSampleRate);
    if (FAILED(hr)) {
        return hr;
    }
    hr = inType->SetUINT32(MF_MT_AUDIO_BLOCK_ALIGNMENT, kChannels * (kBitsPerSample / 8));
    if (FAILED(hr)) {
        return hr;
    }
    hr = inType->SetUINT32(MF_MT_AUDIO_AVG_BYTES_PER_SECOND, kBytesPerSecond);
    if (FAILED(hr)) {
        return hr;
    }
    hr = inType->SetUINT32(MF_MT_AUDIO_BITS_PER_SAMPLE, kBitsPerSample);
    if (FAILED(hr)) {
        return hr;
    }
    hr = inType->SetUINT32(MF_MT_ALL_SAMPLES_INDEPENDENT, TRUE);
    if (FAILED(hr)) {
        return hr;
    }

    return writer->SetInputMediaType(*streamIndex, inType.Get(), nullptr);
}

}  // namespace

bool M4aEncoder::EncodeWavToM4a(const std::wstring& wavPath, std::wstring& outM4aPath, std::wstring& error) {
    outM4aPath.clear();
    error.clear();

    std::vector<int16_t> pcm;
    if (!ReadWavPcm16Stereo48k(wavPath, pcm, error)) {
        return false;
    }

    std::filesystem::path wav(wavPath);
    std::wstring m4aPath = wav.replace_extension(L".m4a").wstring();
    // If the source was .wav, replace_extension works. If somehow not, force it.
    if (_wcsicmp(std::filesystem::path(m4aPath).extension().c_str(), L".m4a") != 0) {
        m4aPath = wavPath + L".m4a";
    }

    ::DeleteFileW(m4aPath.c_str());

    HRESULT hr = ::MFStartup(MF_VERSION, MFSTARTUP_LITE);
    if (FAILED(hr)) {
        error = L"Media Foundation failed to start.";
        return false;
    }

    bool ok = false;
    Microsoft::WRL::ComPtr<IMFSinkWriter> writer;
    hr = ::MFCreateSinkWriterFromURL(m4aPath.c_str(), nullptr, nullptr, &writer);
    if (FAILED(hr) || !writer) {
        error = L"Could not create M4A writer.";
        ::MFShutdown();
        return false;
    }

    DWORD streamIndex = 0;
    hr = ConfigureSinkWriter(writer.Get(), &streamIndex);
    if (FAILED(hr)) {
        error = L"Could not configure AAC encoder.";
        ::MFShutdown();
        ::DeleteFileW(m4aPath.c_str());
        return false;
    }

    hr = writer->BeginWriting();
    if (FAILED(hr)) {
        error = L"AAC encoder BeginWriting failed.";
        ::MFShutdown();
        ::DeleteFileW(m4aPath.c_str());
        return false;
    }

    const size_t totalFrames = pcm.size() / kChannels;
    LONGLONG time100ns = 0;
    const LONGLONG frameDuration = 10000000ll / kSampleRate;

    for (size_t frame = 0; frame < totalFrames; frame += kChunkFrames) {
        const size_t frames = (std::min)(static_cast<size_t>(kChunkFrames), totalFrames - frame);
        const size_t sampleCount = frames * kChannels;
        const DWORD bytes = static_cast<DWORD>(sampleCount * sizeof(int16_t));

        Microsoft::WRL::ComPtr<IMFMediaBuffer> buffer;
        hr = ::MFCreateMemoryBuffer(bytes, &buffer);
        if (FAILED(hr) || !buffer) {
            error = L"MFCreateMemoryBuffer failed.";
            break;
        }

        BYTE* data = nullptr;
        hr = buffer->Lock(&data, nullptr, nullptr);
        if (FAILED(hr) || data == nullptr) {
            error = L"Buffer lock failed.";
            break;
        }
        std::memcpy(data, pcm.data() + frame * kChannels, bytes);
        buffer->Unlock();
        buffer->SetCurrentLength(bytes);

        Microsoft::WRL::ComPtr<IMFSample> sample;
        hr = ::MFCreateSample(&sample);
        if (FAILED(hr) || !sample) {
            error = L"MFCreateSample failed.";
            break;
        }
        sample->AddBuffer(buffer.Get());
        sample->SetSampleTime(time100ns);
        sample->SetSampleDuration(frameDuration * static_cast<LONGLONG>(frames));

        hr = writer->WriteSample(streamIndex, sample.Get());
        if (FAILED(hr)) {
            error = L"WriteSample failed during AAC encode.";
            break;
        }
        time100ns += frameDuration * static_cast<LONGLONG>(frames);
    }

    if (error.empty()) {
        hr = writer->Finalize();
        if (FAILED(hr)) {
            error = L"AAC Finalize failed.";
        } else {
            ok = true;
            outM4aPath = m4aPath;
        }
    }

    writer.Reset();
    ::MFShutdown();

    if (!ok) {
        ::DeleteFileW(m4aPath.c_str());
        if (error.empty()) {
            error = L"M4A compression failed.";
        }
        Log::Error(L"M4A encode failed: " + error);
        return false;
    }

    Log::Info(L"Compressed to M4A: " + outM4aPath);
    return true;
}
