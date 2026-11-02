#include "storage/Settings.hpp"

#include "util/Log.hpp"
#include "util/Paths.hpp"

#include <glaze/glaze.hpp>

#include <windows.h>

#include <fstream>
#include <sstream>
#include <utility>

namespace settings_json {

struct WindowJson {
    int x = 0;
    int y = 0;
    int width = 0;
    int height = 0;
};

struct GeneralJson {
    bool closeToTray = true;
    bool startWithWindows = false;
    bool startMinimized = false;
    bool hotkeyEnabled = true;
    std::string hotkey = "Ctrl+Alt+R";
    bool firstRunTrayHintShown = false;
    std::optional<WindowJson> window;
};

struct RecordingJson {
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

struct TranscriptionJson {
    std::string model = "ggml-base-q5_1.bin";
    std::string language = "auto";
    bool autoTranscribe = false;
    int threads = 0;
    bool useVad = false;
};

struct SettingsJson {
    GeneralJson general{};
    RecordingJson recording{};
    TranscriptionJson transcription{};
};

}  // namespace settings_json

namespace {

using settings_json::GeneralJson;
using settings_json::RecordingJson;
using settings_json::SettingsJson;
using settings_json::TranscriptionJson;
using settings_json::WindowJson;

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

SettingsJson ToJson(const SettingsData& data) {
    SettingsJson json{};
    json.general.closeToTray = data.closeToTray;
    json.general.startWithWindows = data.startWithWindows;
    json.general.startMinimized = data.startMinimized;
    json.general.hotkeyEnabled = data.hotkeyEnabled;
    json.general.hotkey = WideToUtf8(data.hotkey);
    json.general.firstRunTrayHintShown = data.firstRunTrayHintShown;
    if (data.windowPlacement.has_value()) {
        const auto& placement = *data.windowPlacement;
        json.general.window = WindowJson{placement.x, placement.y, placement.width, placement.height};
    }

    json.recording.folder = WideToUtf8(data.folder);
    json.recording.micDeviceId = WideToUtf8(data.micDeviceId);
    json.recording.systemDeviceId = WideToUtf8(data.systemDeviceId);
    json.recording.micGain = data.micGain;
    json.recording.micAgc = data.micAgc;
    json.recording.systemGain = data.systemGain;
    json.recording.saveStems = data.saveStems;
    json.recording.compressToM4a = data.compressToM4a;
    json.recording.keepWav = data.keepWav;
    json.recording.communicationsProcessing = data.communicationsProcessing;

    json.transcription.model = WideToUtf8(data.model);
    json.transcription.language = WideToUtf8(data.language);
    json.transcription.autoTranscribe = data.autoTranscribe;
    json.transcription.threads = data.threads;
    json.transcription.useVad = data.useVad;
    return json;
}

SettingsData FromJson(const SettingsJson& json) {
    SettingsData data{};
    data.closeToTray = json.general.closeToTray;
    data.startWithWindows = json.general.startWithWindows;
    data.startMinimized = json.general.startMinimized;
    data.hotkeyEnabled = json.general.hotkeyEnabled;
    data.hotkey = Utf8ToWide(json.general.hotkey);
    data.firstRunTrayHintShown = json.general.firstRunTrayHintShown;
    if (json.general.window.has_value()) {
        const auto& window = *json.general.window;
        if (window.width > 0 && window.height > 0) {
            data.windowPlacement = WindowPlacementSettings{window.x, window.y, window.width, window.height};
        }
    }

    data.folder = Utf8ToWide(json.recording.folder);
    data.micDeviceId = Utf8ToWide(json.recording.micDeviceId);
    data.systemDeviceId = Utf8ToWide(json.recording.systemDeviceId);
    data.micGain = json.recording.micGain;
    data.micAgc = json.recording.micAgc;
    data.systemGain = json.recording.systemGain;
    data.saveStems = json.recording.saveStems;
    data.compressToM4a = json.recording.compressToM4a;
    data.keepWav = json.recording.keepWav;
    data.communicationsProcessing = json.recording.communicationsProcessing;

    data.model = Utf8ToWide(json.transcription.model);
    data.language = Utf8ToWide(json.transcription.language);
    data.autoTranscribe = json.transcription.autoTranscribe;
    data.threads = json.transcription.threads;
    data.useVad = json.transcription.useVad;
    return data;
}

bool ReadJson(const std::wstring& path, SettingsData& out) {
    std::ifstream file(path, std::ios::binary);
    if (!file) {
        return false;
    }
    std::ostringstream ss;
    ss << file.rdbuf();
    std::string json = ss.str();
    if (json.size() >= 3
        && static_cast<unsigned char>(json[0]) == 0xEF
        && static_cast<unsigned char>(json[1]) == 0xBB
        && static_cast<unsigned char>(json[2]) == 0xBF) {
        json.erase(0, 3);
    }
    if (json.empty()) {
        Log::Error(L"Settings file is empty: " + path);
        return false;
    }

    SettingsJson data{};
    if (const auto ec = glz::read<kReadOpts>(data, json)) {
        Log::Error(L"Failed to parse settings JSON: " + Utf8ToWide(glz::format_error(ec, json)));
        return false;
    }
    out = FromJson(data);
    return true;
}

void ArchiveUnreadable(const std::wstring& path) {
    const std::wstring archive = path + L".invalid";
    if (::CopyFileW(path.c_str(), archive.c_str(), FALSE) == FALSE) {
        Log::Error(L"Failed to archive unreadable settings: " + path);
    }
}

}  // namespace

Settings& Settings::Instance() {
    static Settings settings;
    return settings;
}

bool Settings::Load() {
    Paths::EnsureDirectory(Paths::SettingsDir());
    const std::wstring jsonPath = Paths::SettingsFile();
    if (FileExists(jsonPath)) {
        if (!LoadFromFile(jsonPath)) {
            ArchiveUnreadable(jsonPath);
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
    const bool ok = ReadJson(path, data);
    m_data = ok ? std::move(data) : SettingsData{};
    return ok;
}

bool Settings::SaveToFile(const std::wstring& path) const {
    std::string body;
    const SettingsJson json = ToJson(m_data);
    if (const auto ec = glz::write<kWriteOpts>(json, body)) {
        Log::Error(L"Failed to serialize settings JSON");
        return false;
    }
    if (!body.empty() && body.back() != '\n') {
        body.push_back('\n');
    }

    const std::wstring temp = path + L".tmp";
    HANDLE file = ::CreateFileW(
        temp.c_str(),
        GENERIC_WRITE,
        FILE_SHARE_READ,
        nullptr,
        CREATE_ALWAYS,
        FILE_ATTRIBUTE_NORMAL,
        nullptr);
    if (file == INVALID_HANDLE_VALUE) {
        Log::Error(L"Failed to write settings JSON");
        return false;
    }
    DWORD written = 0;
    const BOOL writeOk = ::WriteFile(file, body.data(), static_cast<DWORD>(body.size()), &written, nullptr);
    ::CloseHandle(file);
    if (writeOk == FALSE || written != body.size()) {
        ::DeleteFileW(temp.c_str());
        Log::Error(L"Failed to write settings JSON");
        return false;
    }
    if (::MoveFileExW(temp.c_str(), path.c_str(), MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH) == FALSE) {
        ::DeleteFileW(temp.c_str());
        Log::Error(L"Failed to replace settings JSON");
        return false;
    }
    return true;
}

std::wstring Settings::RecordingsFolder() const {
    if (!m_data.folder.empty()) {
        return m_data.folder;
    }
    return Paths::DefaultRecordingsDir();
}
