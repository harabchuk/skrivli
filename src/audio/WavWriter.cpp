#include "audio/WavWriter.hpp"

#include "util/Log.hpp"
#include "util/Paths.hpp"

#include <algorithm>
#include <cmath>
#include <cstdio>
#include <cstring>
#include <filesystem>
#include <vector>
#include <windows.h>

namespace {

constexpr uint64_t kPatchIntervalBytes = 48000ull * 2ull * 2ull * 2ull; // ~2 s of PCM16 stereo

#pragma pack(push, 1)
struct WavHeader {
    char riff[4] = {'R', 'I', 'F', 'F'};
    uint32_t riffSize = 36;
    char wave[4] = {'W', 'A', 'V', 'E'};
    char fmt[4] = {'f', 'm', 't', ' '};
    uint32_t fmtSize = 16;
    uint16_t audioFormat = 1; // PCM
    uint16_t channels = WavWriter::kChannels;
    uint32_t sampleRate = WavWriter::kSampleRate;
    uint32_t byteRate = WavWriter::kSampleRate * WavWriter::kChannels * (WavWriter::kBitsPerSample / 8);
    uint16_t blockAlign = WavWriter::kChannels * (WavWriter::kBitsPerSample / 8);
    uint16_t bitsPerSample = WavWriter::kBitsPerSample;
    char data[4] = {'d', 'a', 't', 'a'};
    uint32_t dataSize = 0;
};
#pragma pack(pop)

int16_t FloatToPcm16(float sample) {
    const float clamped = std::clamp(sample, -1.0f, 1.0f);
    const float scaled = clamped * 32767.0f;
    return static_cast<int16_t>(std::lrintf(scaled));
}

bool WriteAll(HANDLE file, const void* data, DWORD bytes) {
    const BYTE* cursor = static_cast<const BYTE*>(data);
    DWORD remaining = bytes;
    while (remaining > 0) {
        DWORD written = 0;
        if (!::WriteFile(file, cursor, remaining, &written, nullptr) || written == 0) {
            return false;
        }
        cursor += written;
        remaining -= written;
    }
    return true;
}

}  // namespace

WavWriter::~WavWriter() {
    Abandon();
}

bool WavWriter::IsWritableDirectory(const std::wstring& folder) {
    if (!Paths::EnsureDirectory(folder)) {
        return false;
    }
    const std::filesystem::path probe = std::filesystem::path(folder) / L".meetingbuddy_write_test";
    HANDLE file = ::CreateFileW(
        probe.c_str(),
        GENERIC_WRITE,
        0,
        nullptr,
        CREATE_ALWAYS,
        FILE_ATTRIBUTE_TEMPORARY | FILE_FLAG_DELETE_ON_CLOSE,
        nullptr);
    if (file == INVALID_HANDLE_VALUE) {
        return false;
    }
    ::CloseHandle(file);
    return true;
}

std::wstring WavWriter::MakeBasePath(const std::wstring& folder) {
    SYSTEMTIME local{};
    ::GetLocalTime(&local);

    wchar_t stamp[64]{};
    std::swprintf(
        stamp,
        64,
        L"MeetingBuddy_%04u-%02u-%02u_%02u-%02u-%02u",
        local.wYear,
        local.wMonth,
        local.wDay,
        local.wHour,
        local.wMinute,
        local.wSecond);

    std::filesystem::path base = std::filesystem::path(folder) / stamp;
    std::wstring candidate = base.wstring();
    if (!std::filesystem::exists(candidate + L".wav")
        && !std::filesystem::exists(candidate + L".wav.partial")) {
        return candidate;
    }

    for (int suffix = 2; suffix < 1000; ++suffix) {
        const std::wstring withSuffix = candidate + L"-" + std::to_wstring(suffix);
        if (!std::filesystem::exists(withSuffix + L".wav")
            && !std::filesystem::exists(withSuffix + L".wav.partial")) {
            return withSuffix;
        }
    }
    return candidate + L"-999";
}

bool WavWriter::Open(const std::wstring& folder) {
    Abandon();
    if (!IsWritableDirectory(folder)) {
        Log::Error(L"Recordings folder is not writable");
        return false;
    }

    const std::wstring base = MakeBasePath(folder);
    m_finalPath = base + L".wav";
    m_partialPath = base + L".wav.partial";
    m_framesWritten = 0;
    m_bytesSincePatch = 0;

    HANDLE file = ::CreateFileW(
        m_partialPath.c_str(),
        GENERIC_WRITE | GENERIC_READ,
        FILE_SHARE_READ,
        nullptr,
        CREATE_ALWAYS,
        FILE_ATTRIBUTE_NORMAL,
        nullptr);
    if (file == INVALID_HANDLE_VALUE) {
        Log::Error(L"Failed to create WAV partial file");
        m_partialPath.clear();
        m_finalPath.clear();
        return false;
    }

    m_file = file;
    if (!WriteHeader()) {
        Abandon();
        return false;
    }
    return true;
}

bool WavWriter::WriteHeader() {
    WavHeader header{};
    return WriteAll(static_cast<HANDLE>(m_file), &header, sizeof(header));
}

bool WavWriter::SeekAndWriteSizes() {
    if (m_file == nullptr) {
        return false;
    }

    const uint64_t dataBytes = m_framesWritten * kChannels * (kBitsPerSample / 8);
    if (dataBytes > 0xFFFFFFFFull) {
        return false;
    }

    WavHeader header{};
    header.dataSize = static_cast<uint32_t>(dataBytes);
    header.riffSize = static_cast<uint32_t>(36 + dataBytes);

    if (::SetFilePointer(static_cast<HANDLE>(m_file), 0, nullptr, FILE_BEGIN) == INVALID_SET_FILE_POINTER
        && ::GetLastError() != NO_ERROR) {
        return false;
    }
    if (!WriteAll(static_cast<HANDLE>(m_file), &header, sizeof(header))) {
        return false;
    }

    LARGE_INTEGER end{};
    end.QuadPart = static_cast<LONGLONG>(sizeof(WavHeader) + dataBytes);
    return ::SetFilePointerEx(static_cast<HANDLE>(m_file), end, nullptr, FILE_BEGIN) != FALSE;
}

bool WavWriter::WriteFloatFrames(const float* interleavedStereo, uint32_t frameCount) {
    if (m_file == nullptr || interleavedStereo == nullptr || frameCount == 0) {
        return false;
    }

    std::vector<int16_t> pcm(static_cast<size_t>(frameCount) * kChannels);
    for (uint32_t i = 0; i < frameCount * kChannels; ++i) {
        pcm[i] = FloatToPcm16(interleavedStereo[i]);
    }

    const DWORD bytes = static_cast<DWORD>(pcm.size() * sizeof(int16_t));
    if (!WriteAll(static_cast<HANDLE>(m_file), pcm.data(), bytes)) {
        Log::Error(L"WAV write failed");
        return false;
    }

    m_framesWritten += frameCount;
    m_bytesSincePatch += bytes;
    if (m_bytesSincePatch >= kPatchIntervalBytes) {
        if (!PatchHeader()) {
            return false;
        }
        m_bytesSincePatch = 0;
    }
    return true;
}

bool WavWriter::PatchHeader() {
    return SeekAndWriteSizes();
}

bool WavWriter::Finalize(std::wstring& outFinalPath) {
    outFinalPath.clear();
    if (m_file == nullptr) {
        return false;
    }

    if (!SeekAndWriteSizes()) {
        Log::Error(L"Failed to patch WAV header on finalize");
        Abandon();
        return false;
    }

    ::FlushFileBuffers(static_cast<HANDLE>(m_file));
    ::CloseHandle(static_cast<HANDLE>(m_file));
    m_file = nullptr;

    if (!::MoveFileExW(m_partialPath.c_str(), m_finalPath.c_str(), MOVEFILE_REPLACE_EXISTING)) {
        Log::Error(L"Failed to rename WAV partial to final");
        return false;
    }

    outFinalPath = m_finalPath;
    m_partialPath.clear();
    return true;
}

void WavWriter::Abandon() {
    if (m_file != nullptr) {
        ::CloseHandle(static_cast<HANDLE>(m_file));
        m_file = nullptr;
    }
    // Leave partial on disk for recovery.
    m_partialPath.clear();
    m_finalPath.clear();
    m_framesWritten = 0;
    m_bytesSincePatch = 0;
}

bool WavWriter::RepairPartialHeader(const std::wstring& partialPath) {
    HANDLE file = ::CreateFileW(
        partialPath.c_str(),
        GENERIC_READ | GENERIC_WRITE,
        FILE_SHARE_READ,
        nullptr,
        OPEN_EXISTING,
        FILE_ATTRIBUTE_NORMAL,
        nullptr);
    if (file == INVALID_HANDLE_VALUE) {
        return false;
    }

    LARGE_INTEGER size{};
    if (!::GetFileSizeEx(file, &size) || size.QuadPart < static_cast<LONGLONG>(sizeof(WavHeader))) {
        ::CloseHandle(file);
        return false;
    }

    const uint64_t dataBytes = static_cast<uint64_t>(size.QuadPart) - sizeof(WavHeader);
    WavHeader header{};
    DWORD read = 0;
    if (!::ReadFile(file, &header, sizeof(header), &read, nullptr) || read != sizeof(header)) {
        ::CloseHandle(file);
        return false;
    }

    if (std::strncmp(header.riff, "RIFF", 4) != 0 || std::strncmp(header.wave, "WAVE", 4) != 0) {
        ::CloseHandle(file);
        return false;
    }

    header.dataSize = static_cast<uint32_t>(std::min<uint64_t>(dataBytes, 0xFFFFFFFFull));
    header.riffSize = static_cast<uint32_t>(36 + header.dataSize);
    header.channels = kChannels;
    header.sampleRate = kSampleRate;
    header.bitsPerSample = kBitsPerSample;
    header.byteRate = kSampleRate * kChannels * (kBitsPerSample / 8);
    header.blockAlign = kChannels * (kBitsPerSample / 8);
    header.audioFormat = 1;
    header.fmtSize = 16;

    if (::SetFilePointer(file, 0, nullptr, FILE_BEGIN) == INVALID_SET_FILE_POINTER
        && ::GetLastError() != NO_ERROR) {
        ::CloseHandle(file);
        return false;
    }
    const bool ok = WriteAll(file, &header, sizeof(header));
    ::CloseHandle(file);
    return ok;
}
