#include "infrastructure/storage/RecordingLibrary.hpp"

#include "infrastructure/audio/WavHeader.hpp"
#include "infrastructure/common/FileIo.hpp"
#include "infrastructure/common/Log.hpp"
#include "infrastructure/common/Text.hpp"
#include "infrastructure/files/WavWriter.hpp"
#include "infrastructure/storage/RecordingSidecar.hpp"

#include <algorithm>
#include <cstdio>
#include <cstring>
#include <filesystem>
#include <fstream>
#include <shellapi.h>
#include <shobjidl.h>
#include <wil/com.h>
#include <wil/resource.h>

namespace {

bool IsStemAudioFile(const std::wstring& fileName) {
    return Text::EndsWithIgnoreCase(fileName, L".mic.wav") ||
           Text::EndsWithIgnoreCase(fileName, L".system.wav") ||
           Text::EndsWithIgnoreCase(fileName, L".mic.recovered.wav") ||
           Text::EndsWithIgnoreCase(fileName, L".system.recovered.wav");
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
    const std::wstring prefix = L"Skrivli_";
    if (fileName.size() < prefix.size() + 19 || fileName.compare(0, prefix.size(), prefix) != 0) {
        return {};
    }
    // YYYY-MM-DD_HH-MM-SS → YYYY-MM-DDTHH:MM:SS
    const auto stamp = fileName.substr(prefix.size(), 19);
    if (stamp.size() != 19 || stamp[10] != L'_' || stamp[4] != L'-' || stamp[7] != L'-' ||
        stamp[13] != L'-' || stamp[16] != L'-') {
        return {};
    }
    std::wstring iso = stamp;
    iso[10] = L'T';
    iso[13] = L':';
    iso[16] = L':';
    return Text::WideToUtf8(iso);
}

auto FileTimeToStartLocal(const std::wstring& path) -> std::string {
    wil::unique_hfile file(::CreateFileW(path.c_str(), GENERIC_READ,
                                         FILE_SHARE_READ | FILE_SHARE_WRITE, nullptr, OPEN_EXISTING,
                                         FILE_ATTRIBUTE_NORMAL, nullptr));
    if (!file) {
        return {};
    }
    FILETIME write{};
    if (!::GetFileTime(file.get(), nullptr, nullptr, &write)) {
        return {};
    }
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
    wil::com_ptr_nothrow<IFileOperation> op;
    auto hr = ::CoCreateInstance(CLSID_FileOperation, nullptr, CLSCTX_ALL, IID_PPV_ARGS(op.put()));
    if (FAILED(hr) || !op) {
        return false;
    }
    op->SetOperationFlags(FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOFX_RECYCLEONDELETE);

    for (const auto& path : paths) {
        wil::com_ptr_nothrow<IShellItem> item;
        hr = ::SHCreateItemFromParsingName(path.c_str(), nullptr, IID_PPV_ARGS(item.put()));
        if (FAILED(hr) || !item) {
            continue;
        }
        op->DeleteItem(item.get(), nullptr);
    }
    hr = op->PerformOperations();
    return SUCCEEDED(hr);
}

} // namespace

auto RecordingLibrary::JsonPathForAudio(const std::wstring& audioPath) -> std::wstring {
    return StemPath(audioPath) + L".json";
}

auto RecordingLibrary::TranscriptPathForAudio(const std::wstring& audioPath) -> std::wstring {
    return StemPath(audioPath) + L".txt";
}

auto RecordingLibrary::LegacyMdPathForAudio(const std::wstring& audioPath) -> std::wstring {
    return StemPath(audioPath) + L".md";
}

double RecordingLibrary::ReadWavDurationSeconds(const std::wstring& wavPath) {
    wil::unique_hfile file(::CreateFileW(wavPath.c_str(), GENERIC_READ, FILE_SHARE_READ, nullptr,
                                         OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr));
    if (!file) {
        return 0.0;
    }

    const auto header = WavHeader::Read(file.get());
    return header ? header->DurationSeconds() : 0.0;
}

bool RecordingLibrary::WriteSidecarForAudio(const std::wstring& audioPath,
                                            const RecordingSidecar& data) {
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
        const auto txtPath = entry.stem + L".txt";
        const auto mdPath = entry.stem + L".md";
        entry.sizeBytes = FileIo::FileSize(entry.audioPath);
        if (std::filesystem::exists(txtPath)) {
            entry.transcriptPath = txtPath;
            entry.hasTranscript = true;
            entry.transcriptIsLegacyMarkdown = false;
        } else if (std::filesystem::exists(mdPath)) {
            entry.transcriptPath = mdPath;
            entry.hasTranscript = true;
            entry.transcriptIsLegacyMarkdown = true;
        }

        RecordingSidecar side{};
        if (Sidecar::Load(entry.jsonPath, side)) {
            entry.startLocal = Text::Utf8ToWide(side.startLocal);
            entry.durationSeconds = side.durationSeconds;
            entry.transcriptStatus = side.transcriptStatus;
            entry.transcriptError = Text::Utf8ToWide(side.transcriptError);
            if (entry.hasTranscript && entry.transcriptStatus == TranscriptStatus::None) {
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
            if (entry.hasTranscript) {
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

bool RecordingLibrary::Rename(const RecordingEntry& entry, const std::wstring& newStemName,
                              std::wstring& error) {
    error.clear();
    if (newStemName.empty() || newStemName.find_first_of(L"\\/:*?\"<>|") != std::wstring::npos) {
        error = L"That name is not valid.";
        return false;
    }

    const auto dir = std::filesystem::path(entry.audioPath).parent_path();
    const auto oldExt = std::filesystem::path(entry.audioPath).extension().wstring();
    const auto newAudio = (dir / (newStemName + oldExt)).wstring();
    const auto newJson = (dir / (newStemName + L".json")).wstring();
    const auto oldTxt = entry.stem + L".txt";
    const auto oldMd = entry.stem + L".md";
    const auto newTxt = (dir / (newStemName + L".txt")).wstring();
    const auto newMd = (dir / (newStemName + L".md")).wstring();

    if (Text::PathEquals(newAudio, entry.audioPath)) {
        return true;
    }
    const bool hasTxt = std::filesystem::exists(oldTxt);
    const bool hasMd = std::filesystem::exists(oldMd);
    if (std::filesystem::exists(newAudio) || std::filesystem::exists(newJson) ||
        (hasTxt && std::filesystem::exists(newTxt)) || (hasMd && std::filesystem::exists(newMd))) {
        error = L"A recording with that name already exists.";
        return false;
    }

    const auto oldMic = StemAudioPath(entry.stem, L".mic.wav");
    const auto oldSys = StemAudioPath(entry.stem, L".system.wav");
    const auto newMic = (dir / (newStemName + L".mic.wav")).wstring();
    const auto newSys = (dir / (newStemName + L".system.wav")).wstring();
    const bool hasMic = std::filesystem::exists(oldMic);
    const bool hasSys = std::filesystem::exists(oldSys);
    if ((hasMic && std::filesystem::exists(newMic)) ||
        (hasSys && std::filesystem::exists(newSys))) {
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
    if (hasTxt) {
        if (!::MoveFileW(oldTxt.c_str(), newTxt.c_str())) {
            ::MoveFileW(newAudio.c_str(), entry.audioPath.c_str());
            if (std::filesystem::exists(newJson)) {
                ::MoveFileW(newJson.c_str(), entry.jsonPath.c_str());
            }
            error = L"Could not rename the transcript.";
            return false;
        }
    }
    if (hasMd) {
        if (!::MoveFileW(oldMd.c_str(), newMd.c_str())) {
            ::MoveFileW(newAudio.c_str(), entry.audioPath.c_str());
            if (std::filesystem::exists(newJson)) {
                ::MoveFileW(newJson.c_str(), entry.jsonPath.c_str());
            }
            if (hasTxt && std::filesystem::exists(newTxt)) {
                ::MoveFileW(newTxt.c_str(), oldTxt.c_str());
            }
            error = L"Could not rename the transcript.";
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
    const auto txtPath = entry.stem + L".txt";
    const auto mdPath = entry.stem + L".md";
    if (std::filesystem::exists(txtPath)) {
        paths.push_back(txtPath);
    }
    if (std::filesystem::exists(mdPath)) {
        paths.push_back(mdPath);
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
