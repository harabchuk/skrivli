#include "storage/RecordingLibrary.hpp"

#include "audio/WavHeader.hpp"
#include "audio/WavWriter.hpp"
#include "util/FileIo.hpp"
#include "util/Log.hpp"
#include "util/Text.hpp"

#include <atlbase.h>
#include <shellapi.h>
#include <shobjidl.h>
#include <wrl/client.h>

#include <algorithm>
#include <cstdio>
#include <cstring>
#include <filesystem>
#include <fstream>

namespace {

bool IsStemAudioFile(const std::wstring& fileName) {
    return Text::EndsWithIgnoreCase(fileName, L".mic.wav")
        || Text::EndsWithIgnoreCase(fileName, L".system.wav")
        || Text::EndsWithIgnoreCase(fileName, L".mic.recovered.wav")
        || Text::EndsWithIgnoreCase(fileName, L".system.recovered.wav");
}

auto StemAudioPath(const std::wstring& stem, const wchar_t* suffix) -> std::wstring {
    return stem + suffix;
}

auto StemPath(const std::filesystem::path& audioPath) -> std::wstring {
    auto path = audioPath;
    // Handle .recovered.wav → stem without .recovered
    const auto file = path.filename().wstring();
    if (Text::EndsWithIgnoreCase(file, L".recovered.wav")) {
        auto full = path.wstring();
        full.erase(full.size() - wcslen(L".recovered.wav"));
        return full;
    }
    path.replace_extension(L"");
    return path.wstring();
}

auto FileNameToStartLocal(const std::wstring& fileName) -> std::string {
    const std::wstring prefix = L"MeetingBuddy_";
    if (fileName.size() < prefix.size() + 19 || fileName.compare(0, prefix.size(), prefix) != 0) {
        return {};
    }
    // YYYY-MM-DD_HH-MM-SS → YYYY-MM-DDTHH:MM:SS
    const auto stamp = fileName.substr(prefix.size(), 19);
    if (stamp.size() != 19 || stamp[10] != L'_' || stamp[4] != L'-' || stamp[7] != L'-'
        || stamp[13] != L'-' || stamp[16] != L'-') {
        return {};
    }
    std::wstring iso = stamp;
    iso[10] = L'T';
    iso[13] = L':';
    iso[16] = L':';
    return Text::WideToUtf8(iso);
}

auto FileTimeToStartLocal(const std::wstring& path) -> std::string {
    ATL::CHandle file(::CreateFileW(
        path.c_str(),
        GENERIC_READ,
        FILE_SHARE_READ | FILE_SHARE_WRITE,
        nullptr,
        OPEN_EXISTING,
        FILE_ATTRIBUTE_NORMAL,
        nullptr));
    if (file.m_h == INVALID_HANDLE_VALUE) {
        file.Detach();
        return {};
    }
    FILETIME write{};
    if (!::GetFileTime(file, nullptr, nullptr, &write)) {
        return {};
    }
    file.Close();
    SYSTEMTIME utc{};
    SYSTEMTIME local{};
    if (!::FileTimeToSystemTime(&write, &utc)) {
        return {};
    }
    if (!::SystemTimeToTzSpecificLocalTime(nullptr, &utc, &local)) {
        local = utc;
    }
    return Sidecar::FormatLocalTimestamp(local);
}

bool ShellDelete(const std::vector<std::wstring>& paths) {
    Microsoft::WRL::ComPtr<IFileOperation> op;
    auto hr = ::CoCreateInstance(CLSID_FileOperation, nullptr, CLSCTX_ALL, IID_PPV_ARGS(&op));
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

auto RecordingLibrary::JsonPathForAudio(const std::wstring& audioPath) -> std::wstring {
    return StemPath(audioPath) + L".json";
}

auto RecordingLibrary::MdPathForAudio(const std::wstring& audioPath) -> std::wstring {
    return StemPath(audioPath) + L".md";
}

double RecordingLibrary::ReadWavDurationSeconds(const std::wstring& wavPath) {
    ATL::CHandle file(::CreateFileW(
        wavPath.c_str(),
        GENERIC_READ,
        FILE_SHARE_READ,
        nullptr,
        OPEN_EXISTING,
        FILE_ATTRIBUTE_NORMAL,
        nullptr));
    if (file.m_h == INVALID_HANDLE_VALUE) {
        file.Detach();
        return 0.0;
    }

    const auto header = WavHeader::Read(file);
    return header ? header->DurationSeconds() : 0.0;
}

bool RecordingLibrary::WriteSidecarForAudio(const std::wstring& audioPath, const RecordingSidecar& data) {
    return Sidecar::Save(JsonPathForAudio(audioPath), data);
}

auto RecordingLibrary::Scan(const std::wstring& folder) -> std::vector<RecordingEntry> {
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
        const auto ext = path.extension().wstring();
        if (_wcsicmp(ext.c_str(), L".wav") != 0 && _wcsicmp(ext.c_str(), L".m4a") != 0) {
            continue;
        }
        const auto fileName = path.filename().wstring();
        if (IsStemAudioFile(fileName)) {
            continue;
        }
        // Prefer M4A when a compressed sibling exists alongside the WAV.
        if (_wcsicmp(ext.c_str(), L".wav") == 0) {
            const auto m4aSibling = StemPath(path) + L".m4a";
            if (std::filesystem::exists(m4aSibling)) {
                continue;
            }
        }

        RecordingEntry entry{};
        entry.audioPath = path.wstring();
        entry.fileName = fileName;
        entry.displayName = entry.fileName;
        entry.stem = StemPath(path);
        entry.jsonPath = entry.stem + L".json";
        entry.mdPath = entry.stem + L".md";
        entry.sizeBytes = FileIo::FileSize(entry.audioPath);
        entry.hasMarkdown = std::filesystem::exists(entry.mdPath);

        RecordingSidecar side{};
        if (Sidecar::Load(entry.jsonPath, side)) {
            entry.startLocal = Text::Utf8ToWide(side.startLocal);
            entry.durationSeconds = side.durationSeconds;
            entry.transcriptStatus = side.transcriptStatus;
            entry.transcriptError = Text::Utf8ToWide(side.transcriptError);
            if (entry.hasMarkdown && entry.transcriptStatus == TranscriptStatus::None) {
                entry.transcriptStatus = TranscriptStatus::Ready;
            }
        } else {
            const auto fromName = FileNameToStartLocal(entry.fileName);
            if (!fromName.empty()) {
                entry.startLocal = Text::Utf8ToWide(fromName);
            } else {
                entry.startLocal = Text::Utf8ToWide(FileTimeToStartLocal(entry.audioPath));
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

    const auto dir = std::filesystem::path(entry.audioPath).parent_path();
    const auto oldExt = std::filesystem::path(entry.audioPath).extension().wstring();
    const auto newAudio = (dir / (newStemName + oldExt)).wstring();
    const auto newJson = (dir / (newStemName + L".json")).wstring();
    const auto newMd = (dir / (newStemName + L".md")).wstring();

    if (Text::PathEquals(newAudio, entry.audioPath)) {
        return true;
    }
    if (std::filesystem::exists(newAudio) || std::filesystem::exists(newJson) || std::filesystem::exists(newMd)) {
        error = L"A recording with that name already exists.";
        return false;
    }

    const auto oldMic = StemAudioPath(entry.stem, L".mic.wav");
    const auto oldSys = StemAudioPath(entry.stem, L".system.wav");
    const auto newMic = (dir / (newStemName + L".mic.wav")).wstring();
    const auto newSys = (dir / (newStemName + L".system.wav")).wstring();
    const bool hasMic = std::filesystem::exists(oldMic);
    const bool hasSys = std::filesystem::exists(oldSys);
    if ((hasMic && std::filesystem::exists(newMic)) || (hasSys && std::filesystem::exists(newSys))) {
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
    if (hasMic && !::MoveFileW(oldMic.c_str(), newMic.c_str())) {
        Log::Warn(L"Could not rename mic stem with recording");
    }
    if (hasSys && !::MoveFileW(oldSys.c_str(), newSys.c_str())) {
        Log::Warn(L"Could not rename system stem with recording");
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
    const auto micStem = StemAudioPath(entry.stem, L".mic.wav");
    const auto sysStem = StemAudioPath(entry.stem, L".system.wav");
    if (std::filesystem::exists(micStem)) {
        paths.push_back(micStem);
    }
    if (std::filesystem::exists(sysStem)) {
        paths.push_back(sysStem);
    }
    if (!ShellDelete(paths)) {
        error = L"Could not move the recording to the Recycle Bin.";
        return false;
    }
    return true;
}
