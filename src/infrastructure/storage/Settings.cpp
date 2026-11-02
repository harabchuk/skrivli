#include "storage/Settings.hpp"

#include "transcription/ModelManager.hpp"
#include "util/FileIo.hpp"
#include "util/Log.hpp"
#include "util/Paths.hpp"
#include "util/Text.hpp"

#include <glaze/glaze.hpp>

#include <windows.h>

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
    std::string model = Text::WideToUtf8(ModelManager::kDefaultModel);
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

bool FileExists(const std::wstring& path) {
    const auto attr = ::GetFileAttributesW(path.c_str());
    return attr != INVALID_FILE_ATTRIBUTES && (attr & FILE_ATTRIBUTE_DIRECTORY) == 0;
}

SettingsFile ToFile(const SettingsData& data) {
    SettingsFile file{};
    file.general.closeToTray = data.closeToTray;
    file.general.startWithWindows = data.startWithWindows;
    file.general.startMinimized = data.startMinimized;
    file.general.hotkeyEnabled = data.hotkeyEnabled;
    file.general.hotkey = Text::WideToUtf8(data.hotkey);
    file.general.firstRunTrayHintShown = data.firstRunTrayHintShown;
    if (data.windowPlacement.has_value()) {
        const auto& placement = *data.windowPlacement;
        file.general.window = WindowPlacement{placement.x, placement.y, placement.width, placement.height};
    }

    file.recording.folder = Text::WideToUtf8(data.folder);
    file.recording.micDeviceId = Text::WideToUtf8(data.micDeviceId);
    file.recording.systemDeviceId = Text::WideToUtf8(data.systemDeviceId);
    file.recording.micGain = data.micGain;
    file.recording.micAgc = data.micAgc;
    file.recording.systemGain = data.systemGain;
    file.recording.saveStems = data.saveStems;
    file.recording.compressToM4a = data.compressToM4a;
    file.recording.keepWav = data.keepWav;
    file.recording.communicationsProcessing = data.communicationsProcessing;

    file.transcription.model = Text::WideToUtf8(data.model);
    file.transcription.language = Text::WideToUtf8(data.language);
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
    data.hotkey = Text::Utf8ToWide(file.general.hotkey);
    data.firstRunTrayHintShown = file.general.firstRunTrayHintShown;
    if (file.general.window.has_value()) {
        const auto& window = *file.general.window;
        if (window.width > 0 && window.height > 0) {
            data.windowPlacement = WindowPlacementSettings{window.x, window.y, window.width, window.height};
        }
    }

    data.folder = Text::Utf8ToWide(file.recording.folder);
    data.micDeviceId = Text::Utf8ToWide(file.recording.micDeviceId);
    data.systemDeviceId = Text::Utf8ToWide(file.recording.systemDeviceId);
    data.micGain = file.recording.micGain;
    data.micAgc = file.recording.micAgc;
    data.systemGain = file.recording.systemGain;
    data.saveStems = file.recording.saveStems;
    data.compressToM4a = file.recording.compressToM4a;
    data.keepWav = file.recording.keepWav;
    data.communicationsProcessing = file.recording.communicationsProcessing;

    data.model = Text::Utf8ToWide(file.transcription.model);
    data.language = Text::Utf8ToWide(file.transcription.language);
    data.autoTranscribe = file.transcription.autoTranscribe;
    data.threads = file.transcription.threads;
    data.useVad = file.transcription.useVad;
    return data;
}

bool LoadDocument(const std::wstring& path, SettingsData& out) {
    const auto bytes = FileIo::ReadFileBytes(path, FileIo::StripBom::Yes);
    if (!bytes.has_value()) {
        return false;
    }
    const auto& text = *bytes;
    if (text.empty()) {
        Log::Error(L"Settings file is empty: " + path);
        return false;
    }

    SettingsFile data{};
    if (const auto ec = glz::read<kReadOpts>(data, text)) {
        Log::Error(L"Failed to parse settings: " + Text::Utf8ToWide(glz::format_error(ec, text)));
        return false;
    }
    out = FromFile(data);
    return true;
}

bool StoreDocument(const std::wstring& path, const SettingsData& data) {
    std::string body;
    const auto file = ToFile(data);
    if (const auto ec = glz::write<kWriteOpts>(file, body)) {
        Log::Error(L"Failed to serialize settings");
        return false;
    }
    if (!body.empty() && body.back() != '\n') {
        body.push_back('\n');
    }

    if (!FileIo::WriteFileBytes(path, body, FileIo::WriteMode::Atomic)) {
        Log::Error(L"Failed to write settings");
        return false;
    }
    return true;
}

void ArchiveUnreadable(const std::wstring& path) {
    const auto archive = path + L".invalid";
    if (::CopyFileW(path.c_str(), archive.c_str(), FALSE) == FALSE) {
        Log::Error(L"Failed to archive unreadable settings: " + path);
    }
}

}  // namespace settings_file

using settings_file::ArchiveUnreadable;
using settings_file::FileExists;
using settings_file::LoadDocument;
using settings_file::StoreDocument;

auto Settings::Instance() -> Settings& {
    static Settings settings;
    return settings;
}

bool Settings::Load() {
    Paths::EnsureDirectory(Paths::SettingsDir());
    const auto path = Paths::SettingsFile();
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

auto Settings::RecordingsFolder() const -> std::wstring {
    if (!m_data.folder.empty()) {
        return m_data.folder;
    }
    return Paths::DefaultRecordingsDir();
}
