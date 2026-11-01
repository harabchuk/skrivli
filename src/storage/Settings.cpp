#include "storage/Settings.hpp"

#include "util/Paths.hpp"

#include <iterator>
#include <string>
#include <windows.h>

namespace {

constexpr wchar_t kSectionGeneral[] = L"General";
constexpr wchar_t kSectionRecording[] = L"Recording";
constexpr wchar_t kSectionTranscription[] = L"Transcription";

std::wstring ReadString(const std::wstring& file, const wchar_t* section, const wchar_t* key, const wchar_t* fallback) {
    wchar_t buffer[2048]{};
    ::GetPrivateProfileStringW(section, key, fallback, buffer, static_cast<DWORD>(std::size(buffer)), file.c_str());
    return buffer;
}

int ReadInt(const std::wstring& file, const wchar_t* section, const wchar_t* key, int fallback) {
    return ::GetPrivateProfileIntW(section, key, fallback, file.c_str());
}

float ReadFloat(const std::wstring& file, const wchar_t* section, const wchar_t* key, float fallback) {
    const std::wstring text = ReadString(file, section, key, L"");
    if (text.empty()) {
        return fallback;
    }
    wchar_t* end = nullptr;
    const float value = std::wcstof(text.c_str(), &end);
    if (end == text.c_str()) {
        return fallback;
    }
    return value;
}

bool WriteString(const std::wstring& file, const wchar_t* section, const wchar_t* key, const std::wstring& value) {
    return ::WritePrivateProfileStringW(section, key, value.c_str(), file.c_str()) != FALSE;
}

bool WriteInt(const std::wstring& file, const wchar_t* section, const wchar_t* key, int value) {
    return WriteString(file, section, key, std::to_wstring(value));
}

bool WriteFloat(const std::wstring& file, const wchar_t* section, const wchar_t* key, float value) {
    wchar_t buffer[64]{};
    std::swprintf(buffer, 64, L"%.6g", static_cast<double>(value));
    return WriteString(file, section, key, buffer);
}

bool KeyExists(const std::wstring& file, const wchar_t* section, const wchar_t* key) {
    constexpr wchar_t kSentinel[] = L"__MB_MISSING__";
    const std::wstring value = ReadString(file, section, key, kSentinel);
    return value != kSentinel;
}

}  // namespace

Settings& Settings::Instance() {
    static Settings settings;
    return settings;
}

bool Settings::Load() {
    Paths::EnsureDirectory(Paths::SettingsDir());
    const std::wstring file = Paths::SettingsFile();
    m_data = SettingsData{};

    m_data.closeToTray = ReadInt(file, kSectionGeneral, L"CloseToTray", 1) != 0;
    m_data.startWithWindows = ReadInt(file, kSectionGeneral, L"StartWithWindows", 0) != 0;
    m_data.startMinimized = ReadInt(file, kSectionGeneral, L"StartMinimized", 0) != 0;
    m_data.hotkeyEnabled = ReadInt(file, kSectionGeneral, L"HotkeyEnabled", 1) != 0;
    m_data.hotkey = ReadString(file, kSectionGeneral, L"Hotkey", L"Ctrl+Alt+R");
    m_data.firstRunTrayHintShown = ReadInt(file, kSectionGeneral, L"FirstRunTrayHintShown", 0) != 0;

    if (KeyExists(file, kSectionGeneral, L"WindowX")
        && KeyExists(file, kSectionGeneral, L"WindowY")
        && KeyExists(file, kSectionGeneral, L"WindowW")
        && KeyExists(file, kSectionGeneral, L"WindowH")) {
        WindowPlacementSettings placement{};
        placement.x = ReadInt(file, kSectionGeneral, L"WindowX", 0);
        placement.y = ReadInt(file, kSectionGeneral, L"WindowY", 0);
        placement.width = ReadInt(file, kSectionGeneral, L"WindowW", 0);
        placement.height = ReadInt(file, kSectionGeneral, L"WindowH", 0);
        if (placement.width > 0 && placement.height > 0) {
            m_data.windowPlacement = placement;
        }
    }

    m_data.folder = ReadString(file, kSectionRecording, L"Folder", L"");
    m_data.micDeviceId = ReadString(file, kSectionRecording, L"MicDeviceId", L"");
    m_data.systemDeviceId = ReadString(file, kSectionRecording, L"SystemDeviceId", L"");
    m_data.micGain = ReadFloat(file, kSectionRecording, L"MicGain", 1.0f);
    m_data.systemGain = ReadFloat(file, kSectionRecording, L"SystemGain", 0.8f);
    m_data.saveStems = ReadInt(file, kSectionRecording, L"SaveStems", 0) != 0;
    m_data.compressToM4a = ReadInt(file, kSectionRecording, L"CompressToM4a", 0) != 0;
    m_data.keepWav = ReadInt(file, kSectionRecording, L"KeepWav", 1) != 0;
    m_data.communicationsProcessing = ReadInt(file, kSectionRecording, L"CommunicationsProcessing", 0) != 0;

    m_data.model = ReadString(file, kSectionTranscription, L"Model", L"ggml-base-q5_1.bin");
    m_data.language = ReadString(file, kSectionTranscription, L"Language", L"auto");
    m_data.autoTranscribe = ReadInt(file, kSectionTranscription, L"AutoTranscribe", 0) != 0;
    m_data.threads = ReadInt(file, kSectionTranscription, L"Threads", 0);
    m_data.useVad = ReadInt(file, kSectionTranscription, L"UseVad", 0) != 0;

    return true;
}

bool Settings::Save() const {
    Paths::EnsureDirectory(Paths::SettingsDir());
    const std::wstring file = Paths::SettingsFile();

    bool ok = true;
    ok = WriteInt(file, kSectionGeneral, L"CloseToTray", m_data.closeToTray ? 1 : 0) && ok;
    ok = WriteInt(file, kSectionGeneral, L"StartWithWindows", m_data.startWithWindows ? 1 : 0) && ok;
    ok = WriteInt(file, kSectionGeneral, L"StartMinimized", m_data.startMinimized ? 1 : 0) && ok;
    ok = WriteInt(file, kSectionGeneral, L"HotkeyEnabled", m_data.hotkeyEnabled ? 1 : 0) && ok;
    ok = WriteString(file, kSectionGeneral, L"Hotkey", m_data.hotkey) && ok;
    ok = WriteInt(file, kSectionGeneral, L"FirstRunTrayHintShown", m_data.firstRunTrayHintShown ? 1 : 0) && ok;

    if (m_data.windowPlacement.has_value()) {
        const auto& placement = *m_data.windowPlacement;
        ok = WriteInt(file, kSectionGeneral, L"WindowX", placement.x) && ok;
        ok = WriteInt(file, kSectionGeneral, L"WindowY", placement.y) && ok;
        ok = WriteInt(file, kSectionGeneral, L"WindowW", placement.width) && ok;
        ok = WriteInt(file, kSectionGeneral, L"WindowH", placement.height) && ok;
    }

    ok = WriteString(file, kSectionRecording, L"Folder", m_data.folder) && ok;
    ok = WriteString(file, kSectionRecording, L"MicDeviceId", m_data.micDeviceId) && ok;
    ok = WriteString(file, kSectionRecording, L"SystemDeviceId", m_data.systemDeviceId) && ok;
    ok = WriteFloat(file, kSectionRecording, L"MicGain", m_data.micGain) && ok;
    ok = WriteFloat(file, kSectionRecording, L"SystemGain", m_data.systemGain) && ok;
    ok = WriteInt(file, kSectionRecording, L"SaveStems", m_data.saveStems ? 1 : 0) && ok;
    ok = WriteInt(file, kSectionRecording, L"CompressToM4a", m_data.compressToM4a ? 1 : 0) && ok;
    ok = WriteInt(file, kSectionRecording, L"KeepWav", m_data.keepWav ? 1 : 0) && ok;
    ok = WriteInt(file, kSectionRecording, L"CommunicationsProcessing", m_data.communicationsProcessing ? 1 : 0) && ok;

    ok = WriteString(file, kSectionTranscription, L"Model", m_data.model) && ok;
    ok = WriteString(file, kSectionTranscription, L"Language", m_data.language) && ok;
    ok = WriteInt(file, kSectionTranscription, L"AutoTranscribe", m_data.autoTranscribe ? 1 : 0) && ok;
    ok = WriteInt(file, kSectionTranscription, L"Threads", m_data.threads) && ok;
    ok = WriteInt(file, kSectionTranscription, L"UseVad", m_data.useVad ? 1 : 0) && ok;

    return ok;
}

std::wstring Settings::RecordingsFolder() const {
    if (!m_data.folder.empty()) {
        return m_data.folder;
    }
    return Paths::DefaultRecordingsDir();
}
