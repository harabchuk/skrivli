#pragma once

#include "core/Transcript.hpp"

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

    std::wstring model = kDefaultWhisperModel;
    std::wstring language = L"auto";
    bool autoTranscribe = false;
    int threads = 0;
    bool useVad = false;
};

class Settings {
  public:
    static auto Instance() -> Settings&;

    // Loads the settings file. A missing file yields defaults.
    bool Load();
    bool Save() const;

    // A missing file resets to defaults and returns true. An unreadable file resets
    // to defaults and returns false.
    bool LoadFromFile(const std::wstring& path);
    bool SaveToFile(const std::wstring& path) const;

    auto Data() -> SettingsData& {
        return m_data;
    }
    auto Data() const -> const SettingsData& {
        return m_data;
    }

    auto RecordingsFolder() const -> std::wstring;

  private:
    Settings() = default;

    SettingsData m_data;
};
