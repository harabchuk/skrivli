#include "audio/WavWriter.hpp"

#include "audio/MiniAudio.hpp"
#include "audio/WavHeader.hpp"
#include "util/FileIo.hpp"
#include "util/Log.hpp"
#include "util/Paths.hpp"

#include <atlbase.h>

#include <algorithm>
#include <cstdio>
#include <filesystem>
#include <vector>
#include <windows.h>

namespace {

constexpr uint64_t kPatchIntervalBytes = 48000ull * 2ull * 2ull * 2ull; // ~2 s of PCM16 stereo
constexpr uint32_t kHeaderBytes = WavHeader::kHeaderBytes;
constexpr uint32_t kRiffSizeOffset = 4;
constexpr uint32_t kDataSizeOffset = 40;

bool WriteAll(HANDLE file, const void* data, DWORD bytes) {
    const auto* cursor = static_cast<const BYTE*>(data);
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

bool SeekHandle(HANDLE file, int64_t offset, DWORD method) {
    LARGE_INTEGER distance{};
    distance.QuadPart = offset;
    return ::SetFilePointerEx(file, distance, nullptr, method) != FALSE;
}

// MiniAudio writes the PCM WAV header; we only rewrite the two size fields so
// `.partial` files stay playable mid-recording and after a crash.
bool WriteRiffSizes(HANDLE file, uint64_t dataBytes) {
    if (dataBytes > 0xFFFFFFFFull) {
        return false;
    }
    const auto dataSize = static_cast<uint32_t>(dataBytes);
    const auto riffSize = static_cast<uint32_t>(36 + dataBytes);
    if (!SeekHandle(file, kRiffSizeOffset, FILE_BEGIN)) {
        return false;
    }
    if (!WriteAll(file, &riffSize, sizeof(riffSize))) {
        return false;
    }
    if (!SeekHandle(file, kDataSizeOffset, FILE_BEGIN)) {
        return false;
    }
    if (!WriteAll(file, &dataSize, sizeof(dataSize))) {
        return false;
    }
    return SeekHandle(file, static_cast<int64_t>(kHeaderBytes + dataBytes), FILE_BEGIN);
}

bool OpenExisting(ATL::CHandle& out, const std::wstring& path, DWORD access, DWORD share) {
    auto* file = ::CreateFileW(path.c_str(), access, share, nullptr, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (file == INVALID_HANDLE_VALUE) {
        return false;
    }
    out.Attach(file);
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

void WavWriter::CloseFile(bool flush) {
    // Uninitializing the encoder makes dr_wav seek back and write the final RIFF sizes, so it
    // must run while the handle is still open.
    m_encoder.reset();
    if (m_file != nullptr) {
        if (flush) {
            ::FlushFileBuffers(static_cast<HANDLE>(m_file));
        }
        ::CloseHandle(static_cast<HANDLE>(m_file));
        m_file = nullptr;
    }
}

void WavWriter::ResetState() {
    m_partialPath.clear();
    m_finalPath.clear();
    m_folder.clear();
    m_framesWritten = 0;
    m_bytesSincePatch = 0;
}

bool WavWriter::FolderHasFreeSpace(const std::wstring& folder, uint64_t minBytes) {
    ULARGE_INTEGER freeBytesAvailable{};
    if (!::GetDiskFreeSpaceExW(folder.c_str(), &freeBytesAvailable, nullptr, nullptr)) {
        // If the folder vanished, treat as insufficient.
        return false;
    }
    return freeBytesAvailable.QuadPart >= minBytes;
}

auto WavWriter::MakeBasePath(const std::wstring& folder) -> std::wstring {
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

    auto base = std::filesystem::path(folder) / stamp;
    auto candidate = base.wstring();
    if (isFree(candidate)) {
        return candidate;
    }

    for (int suffix = 2; suffix < 1000; ++suffix) {
        const auto withSuffix = candidate + L"-" + std::to_wstring(suffix);
        if (isFree(withSuffix)) {
            return withSuffix;
        }
    }
    return candidate + L"-999";
}

auto WavWriter::RecoveredPathFromPartial(const std::wstring& partialPath) -> std::wstring {
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
    const auto folder = finalPath.parent_path().wstring();
    if (!FileIo::IsWritableDirectory(folder)) {
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

    auto* file = ::CreateFileW(
        m_partialPath.c_str(),
        GENERIC_WRITE | GENERIC_READ,
        FILE_SHARE_READ,
        nullptr,
        CREATE_ALWAYS,
        FILE_ATTRIBUTE_NORMAL,
        nullptr);
    if (file == INVALID_HANDLE_VALUE) {
        Log::Error(L"Failed to create WAV partial file");
        ResetState();
        return false;
    }

    m_file = file;

    auto encoder = std::unique_ptr<WavEncoder, WavEncoderDeleter>(new WavEncoder());
    const auto config = ma_encoder_config_init(
        ma_encoding_format_wav,
        ma_format_s16,
        kChannels,
        kSampleRate);
    const auto result = ma_encoder_init(
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
    return OpenFinalPath(MakeBasePath(folder) + L".wav");
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
    const auto result = ma_encoder_write_pcm_frames(
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
    if (m_file == nullptr) {
        return false;
    }
    const uint64_t dataBytes = m_framesWritten * kChannels * (kBitsPerSample / 8);
    return WriteRiffSizes(static_cast<HANDLE>(m_file), dataBytes);
}

bool WavWriter::Finalize(std::wstring& outFinalPath) {
    outFinalPath.clear();
    if (m_file == nullptr) {
        return false;
    }

    CloseFile(true);

    if (!::MoveFileExW(m_partialPath.c_str(), m_finalPath.c_str(), MOVEFILE_REPLACE_EXISTING)) {
        Log::Error(L"Failed to rename WAV partial to final");
        return false;
    }

    outFinalPath = m_finalPath;
    ResetState();
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

    CloseFile(true);

    const std::filesystem::path src(m_partialPath);
    const auto leaf = src.filename().wstring();
    auto destPartial = (std::filesystem::path(rescueDir) / leaf).wstring();
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
        ResetState();
        return !outFinalPath.empty();
    }

    const auto recovered = RecoveredPathFromPartial(destPartial);
    if (!::MoveFileExW(destPartial.c_str(), recovered.c_str(), MOVEFILE_REPLACE_EXISTING)) {
        outFinalPath = destPartial;
    } else {
        outFinalPath = recovered;
    }

    ResetState();
    return !outFinalPath.empty();
}

void WavWriter::Abandon() {
    CloseFile();
    // Leave partial on disk for recovery.
    ResetState();
}

void WavWriter::Discard() {
    const auto partial = m_partialPath;
    CloseFile();
    if (!partial.empty()) {
        ::DeleteFileW(partial.c_str());
    }
    ResetState();
}

bool WavWriter::RepairPartialHeader(const std::wstring& partialPath) {
    ATL::CHandle file;
    if (!OpenExisting(file, partialPath, GENERIC_READ | GENERIC_WRITE, FILE_SHARE_READ)) {
        return false;
    }

    LARGE_INTEGER size{};
    if (!::GetFileSizeEx(file, &size) || size.QuadPart < static_cast<LONGLONG>(kHeaderBytes)) {
        return false;
    }
    if (!WavHeader::Read(file)) {
        return false;
    }

    const auto dataBytes = std::min<uint64_t>(
        static_cast<uint64_t>(size.QuadPart) - kHeaderBytes,
        0xFFFFFFFFull);
    return WriteRiffSizes(file, dataBytes);
}

auto WavWriter::RecoverPartialsInFolder(const std::wstring& folder) -> std::vector<std::wstring> {
    std::vector<std::wstring> recovered;
    if (folder.empty() || !std::filesystem::exists(folder)) {
        return recovered;
    }

    std::error_code ec;
    for (const auto& entry : std::filesystem::directory_iterator(folder, ec)) {
        if (ec || !entry.is_regular_file()) {
            continue;
        }
        const auto path = entry.path().wstring();
        const auto name = entry.path().filename().wstring();
        if (name.size() < 12 || name.compare(name.size() - 12, 12, L".wav.partial") != 0) {
            continue;
        }

        bool tooShort = false;
        {
            ATL::CHandle file;
            if (!OpenExisting(file, path, GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_WRITE)) {
                continue;
            }
            LARGE_INTEGER size{};
            tooShort = !::GetFileSizeEx(file, &size) || size.QuadPart < static_cast<LONGLONG>(kHeaderBytes);
        }
        if (tooShort) {
            Log::Warn(L"Leaving short partial unrecovered: " + path);
            continue;
        }

        if (!RepairPartialHeader(path)) {
            Log::Warn(L"Failed to repair partial: " + path);
            continue;
        }

        const auto dest = RecoveredPathFromPartial(path);
        if (!::MoveFileExW(path.c_str(), dest.c_str(), MOVEFILE_REPLACE_EXISTING)) {
            Log::Warn(L"Failed to rename recovered partial: " + path);
            continue;
        }
        Log::Info(L"Recovered recording: " + dest);
        recovered.push_back(dest);
    }
    return recovered;
}
