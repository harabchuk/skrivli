#pragma once

#include "transcription/ModelManager.hpp"

#include <optional>
#include <string>

struct WindowPlacementSettings {
    int x = 0;
    int y = 0;
    int width = 0;
    int height = 0;
};

struct SettingsData {
    bool closeToTray = true;
    bool startWithWindows = false;
    bool startMinimized = false;
    bool hotkeyEnabled = true;
    std::wstring hotkey = L"Ctrl+Alt+R";
    bool firstRunTrayHintShown = false;
    std::optional<WindowPlacementSettings> windowPlacement;

    std::wstring folder;
    std::wstring micDeviceId;
    std::wstring systemDeviceId;
    float micGain = 1.0f;
    bool micAgc = false;
    float systemGain = 0.8f;
    bool saveStems = false;
    bool compressToM4a = false;
    bool keepWav = true;
    bool communicationsProcessing = false;

    std::wstring model = ModelManager::kDefaultModel;
    std::wstring language = L"auto";
    bool autoTranscribe = false;
    int threads = 0;
    bool useVad = false;
};

class Settings {
public:
    static Settings& Instance();

    // Loads the settings file. A missing file yields defaults.
    bool Load();
    bool Save() const;

    // A missing file resets to defaults and returns true. An unreadable file resets
    // to defaults and returns false.
    bool LoadFromFile(const std::wstring& path);
    bool SaveToFile(const std::wstring& path) const;

    SettingsData& Data() { return m_data; }
    const SettingsData& Data() const { return m_data; }

    std::wstring RecordingsFolder() const;

private:
    Settings() = default;

    SettingsData m_data;
};
