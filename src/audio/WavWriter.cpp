#include "audio/WavWriter.hpp"

#include "audio/MiniAudio.hpp"
#include "util/Log.hpp"
#include "util/Paths.hpp"

#include <algorithm>
#include <cstdio>
#include <cstring>
#include <filesystem>
#include <vector>
#include <windows.h>

namespace {

constexpr uint64_t kPatchIntervalBytes = 48000ull * 2ull * 2ull * 2ull; // ~2 s of PCM16 stereo
constexpr uint32_t kHeaderBytes = 44;

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

static_assert(sizeof(WavHeader) == kHeaderBytes);

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

struct WavWriter::WavEncoder {
    ma_encoder encoder{};
    bool live = false;
};

void WavWriter::WavEncoderDeleter::operator()(WavEncoder* encoder) const {
    if (encoder == nullptr) {
        return;
    }
    if (encoder->live) {
        ma_encoder_uninit(&encoder->encoder);
        encoder->live = false;
    }
    delete encoder;
}

struct WavEncoderCallbacks {
    static ma_result Write(ma_encoder* encoder, const void* data, size_t bytes, size_t* bytesWritten) {
        auto* self = static_cast<WavWriter*>(encoder->pUserData);
        size_t written = 0;
        if (self == nullptr || !self->WriteBytes(data, bytes, written)) {
            return MA_ERROR;
        }
        if (bytesWritten != nullptr) {
            *bytesWritten = written;
        }
        return MA_SUCCESS;
    }

    static ma_result Seek(ma_encoder* encoder, ma_int64 offset, ma_seek_origin origin) {
        auto* self = static_cast<WavWriter*>(encoder->pUserData);
        if (self == nullptr || !self->SeekFile(offset, static_cast<int>(origin))) {
            return MA_ERROR;
        }
        return MA_SUCCESS;
    }
};

WavWriter::~WavWriter() {
    Abandon();
}

bool WavWriter::WriteBytes(const void* data, size_t bytes, size_t& bytesWritten) {
    bytesWritten = 0;
    if (m_file == nullptr || data == nullptr || bytes > 0xFFFFFFFFull) {
        return false;
    }
    if (!WriteAll(static_cast<HANDLE>(m_file), data, static_cast<DWORD>(bytes))) {
        return false;
    }
    bytesWritten = bytes;
    return true;
}

bool WavWriter::SeekFile(int64_t offset, int origin) {
    if (m_file == nullptr) {
        return false;
    }
    DWORD method = FILE_BEGIN;
    if (origin == static_cast<int>(ma_seek_origin_current)) {
        method = FILE_CURRENT;
    } else if (origin == static_cast<int>(ma_seek_origin_end)) {
        method = FILE_END;
    }
    LARGE_INTEGER distance{};
    distance.QuadPart = offset;
    return ::SetFilePointerEx(static_cast<HANDLE>(m_file), distance, nullptr, method) != FALSE;
}

void WavWriter::CloseEncoder() {
    m_encoder.reset();
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

bool WavWriter::FolderHasFreeSpace(const std::wstring& folder, uint64_t minBytes) {
    ULARGE_INTEGER freeBytesAvailable{};
    if (!::GetDiskFreeSpaceExW(folder.c_str(), &freeBytesAvailable, nullptr, nullptr)) {
        // If the folder vanished, treat as insufficient.
        return false;
    }
    return freeBytesAvailable.QuadPart >= minBytes;
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

    auto isFree = [](const std::wstring& stem) {
        return !std::filesystem::exists(stem + L".wav")
            && !std::filesystem::exists(stem + L".wav.partial")
            && !std::filesystem::exists(stem + L".m4a")
            && !std::filesystem::exists(stem + L".mic.wav")
            && !std::filesystem::exists(stem + L".system.wav");
    };

    std::filesystem::path base = std::filesystem::path(folder) / stamp;
    std::wstring candidate = base.wstring();
    if (isFree(candidate)) {
        return candidate;
    }

    for (int suffix = 2; suffix < 1000; ++suffix) {
        const std::wstring withSuffix = candidate + L"-" + std::to_wstring(suffix);
        if (isFree(withSuffix)) {
            return withSuffix;
        }
    }
    return candidate + L"-999";
}

std::wstring WavWriter::RecoveredPathFromPartial(const std::wstring& partialPath) {
    std::wstring path = partialPath;
    const std::wstring suffix = L".wav.partial";
    if (path.size() >= suffix.size()
        && path.compare(path.size() - suffix.size(), suffix.size(), suffix) == 0) {
        path.erase(path.size() - suffix.size());
        return path + L".recovered.wav";
    }
    return path + L".recovered.wav";
}

bool WavWriter::OpenFinalPath(const std::wstring& finalWavPath) {
    Abandon();
    if (finalWavPath.empty()) {
        return false;
    }

    const std::filesystem::path finalPath(finalWavPath);
    const std::wstring folder = finalPath.parent_path().wstring();
    if (!IsWritableDirectory(folder)) {
        Log::Error(L"Recordings folder is not writable");
        return false;
    }
    if (!FolderHasFreeSpace(folder)) {
        Log::Error(L"Recordings folder has insufficient free disk space");
        return false;
    }

    m_folder = folder;
    m_finalPath = finalWavPath;
    m_partialPath = finalWavPath + L".partial";
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
        m_folder.clear();
        return false;
    }

    m_file = file;

    auto encoder = std::unique_ptr<WavEncoder, WavEncoderDeleter>(new WavEncoder());
    const ma_encoder_config config = ma_encoder_config_init(
        ma_encoding_format_wav,
        ma_format_s16,
        kChannels,
        kSampleRate);
    const ma_result result = ma_encoder_init(
        &WavEncoderCallbacks::Write,
        &WavEncoderCallbacks::Seek,
        this,
        &config,
        &encoder->encoder);
    if (result != MA_SUCCESS) {
        Log::Error(L"Failed to open miniaudio WAV encoder");
        Abandon();
        return false;
    }
    encoder->live = true;
    m_encoder = std::move(encoder);
    return true;
}

bool WavWriter::Open(const std::wstring& folder) {
    Abandon();
    if (!IsWritableDirectory(folder)) {
        Log::Error(L"Recordings folder is not writable");
        return false;
    }
    if (!FolderHasFreeSpace(folder)) {
        Log::Error(L"Recordings folder has insufficient free disk space");
        return false;
    }

    const std::wstring base = MakeBasePath(folder);
    return OpenFinalPath(base + L".wav");
}

bool WavWriter::SeekAndWriteSizes() {
    if (m_file == nullptr) {
        return false;
    }

    const uint64_t dataBytes = m_framesWritten * kChannels * (kBitsPerSample / 8);
    if (dataBytes > 0xFFFFFFFFull) {
        return false;
    }

    const uint32_t dataSize = static_cast<uint32_t>(dataBytes);
    const uint32_t riffSize = static_cast<uint32_t>(36 + dataBytes);

    if (!SeekFile(4, static_cast<int>(ma_seek_origin_start))) {
        return false;
    }
    if (!WriteAll(static_cast<HANDLE>(m_file), &riffSize, sizeof(riffSize))) {
        return false;
    }
    // Canonical PCM header: "data" size sits at byte 40.
    if (!SeekFile(40, static_cast<int>(ma_seek_origin_start))) {
        return false;
    }
    if (!WriteAll(static_cast<HANDLE>(m_file), &dataSize, sizeof(dataSize))) {
        return false;
    }

    LARGE_INTEGER end{};
    end.QuadPart = static_cast<LONGLONG>(kHeaderBytes + dataBytes);
    return ::SetFilePointerEx(static_cast<HANDLE>(m_file), end, nullptr, FILE_BEGIN) != FALSE;
}

bool WavWriter::WriteFloatFrames(const float* interleavedStereo, uint32_t frameCount) {
    if (m_encoder == nullptr || !m_encoder->live || interleavedStereo == nullptr || frameCount == 0) {
        return false;
    }

    m_pcmScratch.resize(static_cast<size_t>(frameCount) * kChannels);
    ma_convert_pcm_frames_format(
        m_pcmScratch.data(),
        ma_format_s16,
        interleavedStereo,
        ma_format_f32,
        frameCount,
        kChannels,
        ma_dither_mode_none);

    ma_uint64 framesWritten = 0;
    const ma_result result = ma_encoder_write_pcm_frames(
        &m_encoder->encoder,
        m_pcmScratch.data(),
        frameCount,
        &framesWritten);
    if (result != MA_SUCCESS || framesWritten != frameCount) {
        Log::Error(L"WAV write failed");
        return false;
    }

    const uint64_t bytes = static_cast<uint64_t>(framesWritten) * kChannels * (kBitsPerSample / 8);
    m_framesWritten += framesWritten;
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

    CloseEncoder();
    ::FlushFileBuffers(static_cast<HANDLE>(m_file));
    ::CloseHandle(static_cast<HANDLE>(m_file));
    m_file = nullptr;

    if (!::MoveFileExW(m_partialPath.c_str(), m_finalPath.c_str(), MOVEFILE_REPLACE_EXISTING)) {
        Log::Error(L"Failed to rename WAV partial to final");
        return false;
    }

    outFinalPath = m_finalPath;
    m_partialPath.clear();
    m_finalPath.clear();
    m_folder.clear();
    m_framesWritten = 0;
    m_bytesSincePatch = 0;
    return true;
}

bool WavWriter::RescueTo(const std::wstring& rescueDir, std::wstring& outFinalPath) {
    outFinalPath.clear();
    if (m_file == nullptr) {
        return false;
    }
    if (!Paths::EnsureDirectory(rescueDir)) {
        Abandon();
        return false;
    }

    if (!SeekAndWriteSizes()) {
        Log::Error(L"Failed to patch WAV header before rescue");
    }
    CloseEncoder();
    ::FlushFileBuffers(static_cast<HANDLE>(m_file));
    ::CloseHandle(static_cast<HANDLE>(m_file));
    m_file = nullptr;

    const std::filesystem::path src(m_partialPath);
    const std::wstring leaf = src.filename().wstring();
    std::wstring destPartial = (std::filesystem::path(rescueDir) / leaf).wstring();
    if (!::MoveFileExW(m_partialPath.c_str(), destPartial.c_str(), MOVEFILE_REPLACE_EXISTING)) {
        // Fall back to copy if move across volumes fails mid-way.
        if (!::CopyFileW(m_partialPath.c_str(), destPartial.c_str(), FALSE)) {
            Log::Error(L"Failed to move partial into rescue folder");
            Abandon();
            return false;
        }
        ::DeleteFileW(m_partialPath.c_str());
    }

    if (!RepairPartialHeader(destPartial)) {
        Log::Warn(L"Rescue partial header repair failed; leaving file as-is");
        outFinalPath = destPartial;
        m_partialPath.clear();
        m_finalPath.clear();
        m_folder.clear();
        m_framesWritten = 0;
        m_bytesSincePatch = 0;
        return !outFinalPath.empty();
    }

    const std::wstring recovered = RecoveredPathFromPartial(destPartial);
    if (!::MoveFileExW(destPartial.c_str(), recovered.c_str(), MOVEFILE_REPLACE_EXISTING)) {
        outFinalPath = destPartial;
    } else {
        outFinalPath = recovered;
    }

    m_partialPath.clear();
    m_finalPath.clear();
    m_folder.clear();
    m_framesWritten = 0;
    m_bytesSincePatch = 0;
    return !outFinalPath.empty();
}

void WavWriter::Abandon() {
    CloseEncoder();
    if (m_file != nullptr) {
        ::CloseHandle(static_cast<HANDLE>(m_file));
        m_file = nullptr;
    }
    // Leave partial on disk for recovery.
    m_partialPath.clear();
    m_finalPath.clear();
    m_folder.clear();
    m_framesWritten = 0;
    m_bytesSincePatch = 0;
}

void WavWriter::Discard() {
    const std::wstring partial = m_partialPath;
    CloseEncoder();
    if (m_file != nullptr) {
        ::CloseHandle(static_cast<HANDLE>(m_file));
        m_file = nullptr;
    }
    if (!partial.empty()) {
        ::DeleteFileW(partial.c_str());
    }
    m_partialPath.clear();
    m_finalPath.clear();
    m_folder.clear();
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

std::vector<std::wstring> WavWriter::RecoverPartialsInFolder(const std::wstring& folder) {
    std::vector<std::wstring> recovered;
    if (folder.empty() || !std::filesystem::exists(folder)) {
        return recovered;
    }

    std::error_code ec;
    for (const auto& entry : std::filesystem::directory_iterator(folder, ec)) {
        if (ec || !entry.is_regular_file()) {
            continue;
        }
        const std::wstring path = entry.path().wstring();
        const std::wstring name = entry.path().filename().wstring();
        if (name.size() < 12 || name.compare(name.size() - 12, 12, L".wav.partial") != 0) {
            continue;
        }

        LARGE_INTEGER size{};
        HANDLE file = ::CreateFileW(
            path.c_str(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            nullptr,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            nullptr);
        if (file == INVALID_HANDLE_VALUE) {
            continue;
        }
        const bool tooShort = !::GetFileSizeEx(file, &size) || size.QuadPart < 44;
        ::CloseHandle(file);
        if (tooShort) {
            Log::Warn(L"Leaving short partial unrecovered: " + path);
            continue;
        }

        if (!RepairPartialHeader(path)) {
            Log::Warn(L"Failed to repair partial: " + path);
            continue;
        }

        const std::wstring dest = RecoveredPathFromPartial(path);
        if (!::MoveFileExW(path.c_str(), dest.c_str(), MOVEFILE_REPLACE_EXISTING)) {
            Log::Warn(L"Failed to rename recovered partial: " + path);
            continue;
        }
        Log::Info(L"Recovered recording: " + dest);
        recovered.push_back(dest);
    }
    return recovered;
}
