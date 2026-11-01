#include "storage/RecordingLibrary.hpp"

#include "audio/WavWriter.hpp"
#include "util/Log.hpp"

#include <shellapi.h>
#include <shobjidl.h>
#include <wrl/client.h>

#include <algorithm>
#include <cstdio>
#include <cstring>
#include <filesystem>
#include <fstream>

namespace {

bool EndsWithIgnoreCase(const std::wstring& value, const wchar_t* suffix) {
    const size_t len = wcslen(suffix);
    if (value.size() < len) {
        return false;
    }
    return _wcsicmp(value.c_str() + (value.size() - len), suffix) == 0;
}

std::wstring StemPath(const std::filesystem::path& audioPath) {
    auto path = audioPath;
    // Handle .recovered.wav → stem without .recovered
    const std::wstring file = path.filename().wstring();
    if (EndsWithIgnoreCase(file, L".recovered.wav")) {
        std::wstring full = path.wstring();
        full.erase(full.size() - wcslen(L".recovered.wav"));
        return full;
    }
    path.replace_extension(L"");
    return path.wstring();
}

std::string FileNameToStartLocal(const std::wstring& fileName) {
    const std::wstring prefix = L"MeetingBuddy_";
    if (fileName.size() < prefix.size() + 19 || fileName.compare(0, prefix.size(), prefix) != 0) {
        return {};
    }
    // YYYY-MM-DD_HH-MM-SS → YYYY-MM-DDTHH:MM:SS
    const std::wstring stamp = fileName.substr(prefix.size(), 19);
    if (stamp.size() != 19 || stamp[10] != L'_' || stamp[4] != L'-' || stamp[7] != L'-'
        || stamp[13] != L'-' || stamp[16] != L'-') {
        return {};
    }
    std::wstring iso = stamp;
    iso[10] = L'T';
    iso[13] = L':';
    iso[16] = L':';
    return Sidecar::WideToUtf8(iso);
}

std::string FileTimeToStartLocal(const std::wstring& path) {
    HANDLE file = ::CreateFileW(
        path.c_str(),
        GENERIC_READ,
        FILE_SHARE_READ | FILE_SHARE_WRITE,
        nullptr,
        OPEN_EXISTING,
        FILE_ATTRIBUTE_NORMAL,
        nullptr);
    if (file == INVALID_HANDLE_VALUE) {
        return {};
    }
    FILETIME create{};
    FILETIME write{};
    if (!::GetFileTime(file, &create, nullptr, &write)) {
        ::CloseHandle(file);
        return {};
    }
    ::CloseHandle(file);
    FILETIME localFile{};
    SYSTEMTIME utc{};
    SYSTEMTIME local{};
    if (!::FileTimeToSystemTime(&write, &utc)) {
        return {};
    }
    if (!::SystemTimeToTzSpecificLocalTime(nullptr, &utc, &local)) {
        local = utc;
    }
    (void)localFile;
    return Sidecar::FormatLocalTimestamp(local);
}

bool ShellDelete(const std::vector<std::wstring>& paths) {
    Microsoft::WRL::ComPtr<IFileOperation> op;
    HRESULT hr = ::CoCreateInstance(CLSID_FileOperation, nullptr, CLSCTX_ALL, IID_PPV_ARGS(&op));
    if (FAILED(hr) || !op) {
        return false;
    }
    op->SetOperationFlags(FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOFX_RECYCLEONDELETE);

    for (const auto& path : paths) {
        Microsoft::WRL::ComPtr<IShellItem> item;
        hr = ::SHCreateItemFromParsingName(path.c_str(), nullptr, IID_PPV_ARGS(&item));
        if (FAILED(hr) || !item) {
            continue;
        }
        op->DeleteItem(item.Get(), nullptr);
    }
    hr = op->PerformOperations();
    return SUCCEEDED(hr);
}

}  // namespace

std::wstring RecordingLibrary::JsonPathForAudio(const std::wstring& audioPath) {
    return StemPath(audioPath) + L".json";
}

std::wstring RecordingLibrary::MdPathForAudio(const std::wstring& audioPath) {
    return StemPath(audioPath) + L".md";
}

uint64_t RecordingLibrary::FileSizeBytes(const std::wstring& path) {
    WIN32_FILE_ATTRIBUTE_DATA data{};
    if (!::GetFileAttributesExW(path.c_str(), GetFileExInfoStandard, &data)) {
        return 0;
    }
    ULARGE_INTEGER size{};
    size.HighPart = data.nFileSizeHigh;
    size.LowPart = data.nFileSizeLow;
    return size.QuadPart;
}

double RecordingLibrary::ReadWavDurationSeconds(const std::wstring& wavPath) {
    HANDLE file = ::CreateFileW(
        wavPath.c_str(),
        GENERIC_READ,
        FILE_SHARE_READ,
        nullptr,
        OPEN_EXISTING,
        FILE_ATTRIBUTE_NORMAL,
        nullptr);
    if (file == INVALID_HANDLE_VALUE) {
        return 0.0;
    }

    LARGE_INTEGER size{};
    if (!::GetFileSizeEx(file, &size) || size.QuadPart < 44) {
        ::CloseHandle(file);
        return 0.0;
    }

    char header[44]{};
    DWORD read = 0;
    if (!::ReadFile(file, header, 44, &read, nullptr) || read != 44) {
        ::CloseHandle(file);
        return 0.0;
    }
    ::CloseHandle(file);

    if (std::strncmp(header, "RIFF", 4) != 0 || std::strncmp(header + 8, "WAVE", 4) != 0) {
        return 0.0;
    }

    uint16_t channels = 0;
    uint32_t sampleRate = 0;
    uint16_t bits = 0;
    uint32_t dataSize = 0;
    std::memcpy(&channels, header + 22, sizeof(channels));
    std::memcpy(&sampleRate, header + 24, sizeof(sampleRate));
    std::memcpy(&bits, header + 34, sizeof(bits));
    std::memcpy(&dataSize, header + 40, sizeof(dataSize));
    if (channels == 0 || sampleRate == 0 || bits == 0) {
        return 0.0;
    }
    const uint64_t bytesPerSecond = static_cast<uint64_t>(sampleRate) * channels * (bits / 8);
    if (bytesPerSecond == 0) {
        return 0.0;
    }
    return static_cast<double>(dataSize) / static_cast<double>(bytesPerSecond);
}

bool RecordingLibrary::WriteSidecarForAudio(const std::wstring& audioPath, const RecordingSidecar& data) {
    return Sidecar::Save(JsonPathForAudio(audioPath), data);
}

std::vector<RecordingEntry> RecordingLibrary::Scan(const std::wstring& folder) {
    WavWriter::RecoverPartialsInFolder(folder);

    std::vector<RecordingEntry> entries;
    if (folder.empty() || !std::filesystem::exists(folder)) {
        return entries;
    }

    std::error_code ec;
    for (const auto& item : std::filesystem::directory_iterator(folder, ec)) {
        if (ec || !item.is_regular_file()) {
            continue;
        }
        const auto path = item.path();
        const std::wstring ext = path.extension().wstring();
        if (_wcsicmp(ext.c_str(), L".wav") != 0 && _wcsicmp(ext.c_str(), L".m4a") != 0) {
            continue;
        }

        RecordingEntry entry{};
        entry.audioPath = path.wstring();
        entry.fileName = path.filename().wstring();
        entry.displayName = entry.fileName;
        entry.stem = StemPath(path);
        entry.jsonPath = entry.stem + L".json";
        entry.mdPath = entry.stem + L".md";
        entry.sizeBytes = FileSizeBytes(entry.audioPath);
        entry.hasMarkdown = std::filesystem::exists(entry.mdPath);

        RecordingSidecar side{};
        if (Sidecar::Load(entry.jsonPath, side)) {
            entry.startLocal = Sidecar::Utf8ToWide(side.startLocal);
            entry.durationSeconds = side.durationSeconds;
            entry.transcriptStatus = side.transcriptStatus;
            entry.transcriptError = Sidecar::Utf8ToWide(side.transcriptError);
            if (entry.hasMarkdown && entry.transcriptStatus == TranscriptStatus::None) {
                entry.transcriptStatus = TranscriptStatus::Ready;
            }
        } else {
            const std::string fromName = FileNameToStartLocal(entry.fileName);
            if (!fromName.empty()) {
                entry.startLocal = Sidecar::Utf8ToWide(fromName);
            } else {
                entry.startLocal = Sidecar::Utf8ToWide(FileTimeToStartLocal(entry.audioPath));
            }
            if (_wcsicmp(ext.c_str(), L".wav") == 0) {
                entry.durationSeconds = ReadWavDurationSeconds(entry.audioPath);
            }
            if (entry.hasMarkdown) {
                entry.transcriptStatus = TranscriptStatus::Ready;
            }
        }

        entries.push_back(std::move(entry));
    }

    std::sort(entries.begin(), entries.end(), [](const RecordingEntry& a, const RecordingEntry& b) {
        if (a.startLocal != b.startLocal) {
            return a.startLocal > b.startLocal; // newest first
        }
        return a.fileName > b.fileName;
    });
    return entries;
}

bool RecordingLibrary::Rename(const RecordingEntry& entry, const std::wstring& newStemName, std::wstring& error) {
    error.clear();
    if (newStemName.empty() || newStemName.find_first_of(L"\\/:*?\"<>|") != std::wstring::npos) {
        error = L"That name is not valid.";
        return false;
    }

    const std::filesystem::path dir = std::filesystem::path(entry.audioPath).parent_path();
    const std::wstring oldExt = std::filesystem::path(entry.audioPath).extension().wstring();
    const std::wstring newAudio = (dir / (newStemName + oldExt)).wstring();
    const std::wstring newJson = (dir / (newStemName + L".json")).wstring();
    const std::wstring newMd = (dir / (newStemName + L".md")).wstring();

    if (_wcsicmp(newAudio.c_str(), entry.audioPath.c_str()) == 0) {
        return true;
    }
    if (std::filesystem::exists(newAudio) || std::filesystem::exists(newJson) || std::filesystem::exists(newMd)) {
        error = L"A recording with that name already exists.";
        return false;
    }

    if (!::MoveFileW(entry.audioPath.c_str(), newAudio.c_str())) {
        error = L"Could not rename the audio file.";
        return false;
    }
    if (std::filesystem::exists(entry.jsonPath)) {
        if (!::MoveFileW(entry.jsonPath.c_str(), newJson.c_str())) {
            ::MoveFileW(newAudio.c_str(), entry.audioPath.c_str());
            error = L"Could not rename the sidecar JSON.";
            return false;
        }
    }
    if (std::filesystem::exists(entry.mdPath)) {
        if (!::MoveFileW(entry.mdPath.c_str(), newMd.c_str())) {
            // Best-effort rollback of audio/json
            ::MoveFileW(newAudio.c_str(), entry.audioPath.c_str());
            if (std::filesystem::exists(newJson)) {
                ::MoveFileW(newJson.c_str(), entry.jsonPath.c_str());
            }
            error = L"Could not rename the transcript Markdown.";
            return false;
        }
    }
    return true;
}

bool RecordingLibrary::DeleteToRecycleBin(const RecordingEntry& entry, std::wstring& error) {
    error.clear();
    std::vector<std::wstring> paths;
    paths.push_back(entry.audioPath);
    if (std::filesystem::exists(entry.jsonPath)) {
        paths.push_back(entry.jsonPath);
    }
    if (std::filesystem::exists(entry.mdPath)) {
        paths.push_back(entry.mdPath);
    }
    if (!ShellDelete(paths)) {
        error = L"Could not move the recording to the Recycle Bin.";
        return false;
    }
    return true;
}
