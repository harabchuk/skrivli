#include "storage/Settings.hpp"

#include "util/Log.hpp"
#include "util/Paths.hpp"

#include <glaze/glaze.hpp>

#include <windows.h>

#include <fstream>
#include <sstream>
#include <utility>

namespace settings_file {

struct WindowPlacement {
    int x = 0;
    int y = 0;
    int width = 0;
    int height = 0;
};

struct GeneralSettings {
    bool closeToTray = true;
    bool startWithWindows = false;
    bool startMinimized = false;
    bool hotkeyEnabled = true;
    std::string hotkey = "Ctrl+Alt+R";
    bool firstRunTrayHintShown = false;
    std::optional<WindowPlacement> window;
};

struct RecordingSettings {
    std::string folder;
    std::string micDeviceId;
    std::string systemDeviceId;
    float micGain = 1.0f;
    bool micAgc = false;
    float systemGain = 0.8f;
    bool saveStems = false;
    bool compressToM4a = false;
    bool keepWav = true;
    bool communicationsProcessing = false;
};

struct TranscriptionSettings {
    std::string model = "ggml-base-q5_1.bin";
    std::string language = "auto";
    bool autoTranscribe = false;
    int threads = 0;
    bool useVad = false;
};

struct SettingsFile {
    GeneralSettings general{};
    RecordingSettings recording{};
    TranscriptionSettings transcription{};
};

constexpr glz::opts kReadOpts{
    .comments = true,
    .error_on_unknown_keys = false,
};

constexpr glz::opts kWriteOpts{
    .prettify = true,
};

std::string WideToUtf8(const std::wstring& wide) {
    if (wide.empty()) {
        return {};
    }
    const int size = ::WideCharToMultiByte(CP_UTF8, 0, wide.c_str(), static_cast<int>(wide.size()), nullptr, 0, nullptr, nullptr);
    if (size <= 0) {
        return {};
    }
    std::string out(static_cast<size_t>(size), '\0');
    ::WideCharToMultiByte(CP_UTF8, 0, wide.c_str(), static_cast<int>(wide.size()), out.data(), size, nullptr, nullptr);
    return out;
}

std::wstring Utf8ToWide(const std::string& utf8) {
    if (utf8.empty()) {
        return {};
    }
    const int size = ::MultiByteToWideChar(CP_UTF8, 0, utf8.c_str(), static_cast<int>(utf8.size()), nullptr, 0);
    if (size <= 0) {
        return {};
    }
    std::wstring out(static_cast<size_t>(size), L'\0');
    ::MultiByteToWideChar(CP_UTF8, 0, utf8.c_str(), static_cast<int>(utf8.size()), out.data(), size);
    return out;
}

bool FileExists(const std::wstring& path) {
    const DWORD attr = ::GetFileAttributesW(path.c_str());
    return attr != INVALID_FILE_ATTRIBUTES && (attr & FILE_ATTRIBUTE_DIRECTORY) == 0;
}

SettingsFile ToFile(const SettingsData& data) {
    SettingsFile file{};
    file.general.closeToTray = data.closeToTray;
    file.general.startWithWindows = data.startWithWindows;
    file.general.startMinimized = data.startMinimized;
    file.general.hotkeyEnabled = data.hotkeyEnabled;
    file.general.hotkey = WideToUtf8(data.hotkey);
    file.general.firstRunTrayHintShown = data.firstRunTrayHintShown;
    if (data.windowPlacement.has_value()) {
        const auto& placement = *data.windowPlacement;
        file.general.window = WindowPlacement{placement.x, placement.y, placement.width, placement.height};
    }

    file.recording.folder = WideToUtf8(data.folder);
    file.recording.micDeviceId = WideToUtf8(data.micDeviceId);
    file.recording.systemDeviceId = WideToUtf8(data.systemDeviceId);
    file.recording.micGain = data.micGain;
    file.recording.micAgc = data.micAgc;
    file.recording.systemGain = data.systemGain;
    file.recording.saveStems = data.saveStems;
    file.recording.compressToM4a = data.compressToM4a;
    file.recording.keepWav = data.keepWav;
    file.recording.communicationsProcessing = data.communicationsProcessing;

    file.transcription.model = WideToUtf8(data.model);
    file.transcription.language = WideToUtf8(data.language);
    file.transcription.autoTranscribe = data.autoTranscribe;
    file.transcription.threads = data.threads;
    file.transcription.useVad = data.useVad;
    return file;
}

SettingsData FromFile(const SettingsFile& file) {
    SettingsData data{};
    data.closeToTray = file.general.closeToTray;
    data.startWithWindows = file.general.startWithWindows;
    data.startMinimized = file.general.startMinimized;
    data.hotkeyEnabled = file.general.hotkeyEnabled;
    data.hotkey = Utf8ToWide(file.general.hotkey);
    data.firstRunTrayHintShown = file.general.firstRunTrayHintShown;
    if (file.general.window.has_value()) {
        const auto& window = *file.general.window;
        if (window.width > 0 && window.height > 0) {
            data.windowPlacement = WindowPlacementSettings{window.x, window.y, window.width, window.height};
        }
    }

    data.folder = Utf8ToWide(file.recording.folder);
    data.micDeviceId = Utf8ToWide(file.recording.micDeviceId);
    data.systemDeviceId = Utf8ToWide(file.recording.systemDeviceId);
    data.micGain = file.recording.micGain;
    data.micAgc = file.recording.micAgc;
    data.systemGain = file.recording.systemGain;
    data.saveStems = file.recording.saveStems;
    data.compressToM4a = file.recording.compressToM4a;
    data.keepWav = file.recording.keepWav;
    data.communicationsProcessing = file.recording.communicationsProcessing;

    data.model = Utf8ToWide(file.transcription.model);
    data.language = Utf8ToWide(file.transcription.language);
    data.autoTranscribe = file.transcription.autoTranscribe;
    data.threads = file.transcription.threads;
    data.useVad = file.transcription.useVad;
    return data;
}

bool LoadDocument(const std::wstring& path, SettingsData& out) {
    std::ifstream file(path, std::ios::binary);
    if (!file) {
        return false;
    }
    std::ostringstream ss;
    ss << file.rdbuf();
    std::string text = ss.str();
    if (text.size() >= 3
        && static_cast<unsigned char>(text[0]) == 0xEF
        && static_cast<unsigned char>(text[1]) == 0xBB
        && static_cast<unsigned char>(text[2]) == 0xBF) {
        text.erase(0, 3);
    }
    if (text.empty()) {
        Log::Error(L"Settings file is empty: " + path);
        return false;
    }

    SettingsFile data{};
    if (const auto ec = glz::read<kReadOpts>(data, text)) {
        Log::Error(L"Failed to parse settings: " + Utf8ToWide(glz::format_error(ec, text)));
        return false;
    }
    out = FromFile(data);
    return true;
}

bool StoreDocument(const std::wstring& path, const SettingsData& data) {
    std::string body;
    const SettingsFile file = ToFile(data);
    if (const auto ec = glz::write<kWriteOpts>(file, body)) {
        Log::Error(L"Failed to serialize settings");
        return false;
    }
    if (!body.empty() && body.back() != '\n') {
        body.push_back('\n');
    }

    const std::wstring temp = path + L".tmp";
    HANDLE handle = ::CreateFileW(
        temp.c_str(),
        GENERIC_WRITE,
        FILE_SHARE_READ,
        nullptr,
        CREATE_ALWAYS,
        FILE_ATTRIBUTE_NORMAL,
        nullptr);
    if (handle == INVALID_HANDLE_VALUE) {
        Log::Error(L"Failed to write settings");
        return false;
    }
    DWORD written = 0;
    const BOOL writeOk = ::WriteFile(handle, body.data(), static_cast<DWORD>(body.size()), &written, nullptr);
    ::CloseHandle(handle);
    if (writeOk == FALSE || written != body.size()) {
        ::DeleteFileW(temp.c_str());
        Log::Error(L"Failed to write settings");
        return false;
    }
    if (::MoveFileExW(temp.c_str(), path.c_str(), MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH) == FALSE) {
        ::DeleteFileW(temp.c_str());
        Log::Error(L"Failed to replace settings");
        return false;
    }
    return true;
}

void ArchiveUnreadable(const std::wstring& path) {
    const std::wstring archive = path + L".invalid";
    if (::CopyFileW(path.c_str(), archive.c_str(), FALSE) == FALSE) {
        Log::Error(L"Failed to archive unreadable settings: " + path);
    }
}

}  // namespace settings_file

using settings_file::ArchiveUnreadable;
using settings_file::FileExists;
using settings_file::LoadDocument;
using settings_file::StoreDocument;

Settings& Settings::Instance() {
    static Settings settings;
    return settings;
}

bool Settings::Load() {
    Paths::EnsureDirectory(Paths::SettingsDir());
    const std::wstring path = Paths::SettingsFile();
    if (FileExists(path)) {
        if (!LoadFromFile(path)) {
            ArchiveUnreadable(path);
            return false;
        }
        return true;
    }

    m_data = SettingsData{};
    return true;
}

bool Settings::Save() const {
    Paths::EnsureDirectory(Paths::SettingsDir());
    return SaveToFile(Paths::SettingsFile());
}

bool Settings::LoadFromFile(const std::wstring& path) {
    if (!FileExists(path)) {
        m_data = SettingsData{};
        return true;
    }

    SettingsData data{};
    const bool ok = LoadDocument(path, data);
    m_data = ok ? std::move(data) : SettingsData{};
    return ok;
}

bool Settings::SaveToFile(const std::wstring& path) const {
    return StoreDocument(path, m_data);
}

std::wstring Settings::RecordingsFolder() const {
    if (!m_data.folder.empty()) {
        return m_data.folder;
    }
    return Paths::DefaultRecordingsDir();
}
