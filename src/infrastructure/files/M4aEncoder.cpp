#include "infrastructure/files/M4aEncoder.hpp"

#include "infrastructure/audio/WavHeader.hpp"
#include "infrastructure/common/Log.hpp"
#include "infrastructure/common/Scoped.hpp"

#include <mfapi.h>
#include <mfidl.h>
#include <mfreadwrite.h>
#include <wmcodecdsp.h>

#include <wil/com.h>
#include <wil/result.h>

#include <algorithm>
#include <cstdio>
#include <cstring>
#include <filesystem>
#include <initializer_list>
#include <vector>

namespace {

constexpr UINT32 kSampleRate = 48000;
constexpr UINT32 kChannels = 2;
constexpr UINT32 kBitsPerSample = 16;
constexpr UINT32 kBytesPerSecond = kSampleRate * kChannels * (kBitsPerSample / 8);
constexpr UINT32 kAacBitrate = 128000;
constexpr UINT32 kChunkFrames = 4800; // 100 ms

bool ReadWavPcm16Stereo48k(const std::wstring& path, std::vector<int16_t>& pcm, std::wstring& error) {
    wil::unique_hfile file(::CreateFileW(
        path.c_str(),
        GENERIC_READ,
        FILE_SHARE_READ,
        nullptr,
        OPEN_EXISTING,
        FILE_ATTRIBUTE_NORMAL,
        nullptr));
    if (!file) {
        error = L"Could not open WAV for compression.";
        return false;
    }

    LARGE_INTEGER size{};
    if (!::GetFileSizeEx(file.get(), &size) || size.QuadPart < 44) {
        error = L"WAV file is too short.";
        return false;
    }

    const auto header = WavHeader::Read(file.get());
    if (!header) {
        error = L"Not a RIFF/WAVE file.";
        return false;
    }

    // Our WavWriter uses the canonical 44-byte header; other chunk layouts are not walked.
    if (header->format != 1 || header->channels != kChannels || header->sampleRate != kSampleRate
        || header->bits != kBitsPerSample) {
        error = L"WAV must be PCM 16-bit stereo 48 kHz.";
        return false;
    }

    const uint64_t remaining = static_cast<uint64_t>(size.QuadPart) - WavHeader::kHeaderBytes;
    const uint64_t bytes = (std::min)(remaining, static_cast<uint64_t>(header->dataSize));
    if (bytes % 4 != 0) {
        error = L"WAV data size is not aligned.";
        return false;
    }

    pcm.resize(static_cast<size_t>(bytes / sizeof(int16_t)));
    if (!pcm.empty()) {
        DWORD read = 0;
        if (!::ReadFile(file.get(), pcm.data(), static_cast<DWORD>(bytes), &read, nullptr) || read != bytes) {
            error = L"Failed to read WAV PCM data.";
            return false;
        }
    }
    return true;
}

struct MfAttr {
    const GUID& key;
    const GUID* guidValue;
    UINT32 uintValue;

    MfAttr(const GUID& attrKey, const GUID& value)
        : key(attrKey), guidValue(&value), uintValue(0) {}
    MfAttr(const GUID& attrKey, UINT32 value)
        : key(attrKey), guidValue(nullptr), uintValue(value) {}
};

HRESULT SetAttrs(IMFAttributes* attrs, std::initializer_list<MfAttr> list) {
    for (const MfAttr& attr : list) {
        const HRESULT hr = attr.guidValue != nullptr
            ? attrs->SetGUID(attr.key, *attr.guidValue)
            : attrs->SetUINT32(attr.key, attr.uintValue);
        RETURN_IF_FAILED(hr);
    }
    return S_OK;
}

HRESULT ConfigureSinkWriter(
    IMFSinkWriter* writer,
    DWORD* streamIndex) {
    wil::com_ptr_nothrow<IMFMediaType> outType;
    RETURN_IF_FAILED(::MFCreateMediaType(outType.put()));
    const HRESULT outHr = SetAttrs(outType.get(), {
        {MF_MT_MAJOR_TYPE, MFMediaType_Audio},
        {MF_MT_SUBTYPE, MFAudioFormat_AAC},
        {MF_MT_AUDIO_SAMPLES_PER_SECOND, kSampleRate},
        {MF_MT_AUDIO_NUM_CHANNELS, kChannels},
        {MF_MT_AUDIO_BITS_PER_SAMPLE, 16},
        {MF_MT_AUDIO_AVG_BYTES_PER_SECOND, kAacBitrate / 8},
        {MF_MT_AAC_AUDIO_PROFILE_LEVEL_INDICATION, 0x29}, // AAC profile LC
    });
    RETURN_IF_FAILED(outHr);

    RETURN_IF_FAILED(writer->AddStream(outType.get(), streamIndex));

    wil::com_ptr_nothrow<IMFMediaType> inType;
    RETURN_IF_FAILED(::MFCreateMediaType(inType.put()));
    const HRESULT inHr = SetAttrs(inType.get(), {
        {MF_MT_MAJOR_TYPE, MFMediaType_Audio},
        {MF_MT_SUBTYPE, MFAudioFormat_PCM},
        {MF_MT_AUDIO_NUM_CHANNELS, kChannels},
        {MF_MT_AUDIO_SAMPLES_PER_SECOND, kSampleRate},
        {MF_MT_AUDIO_BLOCK_ALIGNMENT, kChannels * (kBitsPerSample / 8)},
        {MF_MT_AUDIO_AVG_BYTES_PER_SECOND, kBytesPerSecond},
        {MF_MT_AUDIO_BITS_PER_SAMPLE, kBitsPerSample},
        {MF_MT_ALL_SAMPLES_INDEPENDENT, static_cast<UINT32>(TRUE)},
    });
    RETURN_IF_FAILED(inHr);

    return writer->SetInputMediaType(*streamIndex, inType.get(), nullptr);
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
    auto m4aPath = wav.replace_extension(L".m4a").wstring();
    // If the source was .wav, replace_extension works. If somehow not, force it.
    if (_wcsicmp(std::filesystem::path(m4aPath).extension().c_str(), L".m4a") != 0) {
        m4aPath = wavPath + L".m4a";
    }

    ::DeleteFileW(m4aPath.c_str());

    const ScopedMfStartup mfStartup;
    if (!mfStartup.IsStarted()) {
        error = L"Media Foundation failed to start.";
        return false;
    }

    bool ok = false;
    wil::com_ptr_nothrow<IMFSinkWriter> writer;
    HRESULT hr = ::MFCreateSinkWriterFromURL(m4aPath.c_str(), nullptr, nullptr, writer.put());
    if (FAILED(hr) || !writer) {
        error = L"Could not create M4A writer.";
        return false;
    }

    DWORD streamIndex = 0;
    hr = ConfigureSinkWriter(writer.get(), &streamIndex);
    if (FAILED(hr)) {
        error = L"Could not configure AAC encoder.";
        ::DeleteFileW(m4aPath.c_str());
        return false;
    }

    hr = writer->BeginWriting();
    if (FAILED(hr)) {
        error = L"AAC encoder BeginWriting failed.";
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

        wil::com_ptr_nothrow<IMFMediaBuffer> buffer;
        hr = ::MFCreateMemoryBuffer(bytes, buffer.put());
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

        wil::com_ptr_nothrow<IMFSample> sample;
        hr = ::MFCreateSample(sample.put());
        if (FAILED(hr) || !sample) {
            error = L"MFCreateSample failed.";
            break;
        }
        sample->AddBuffer(buffer.get());
        sample->SetSampleTime(time100ns);
        sample->SetSampleDuration(frameDuration * static_cast<LONGLONG>(frames));

        hr = writer->WriteSample(streamIndex, sample.get());
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

    writer.reset();

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
