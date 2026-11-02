#include "infrastructure/storage/Settings.hpp"

#include <windows.h>

#include <filesystem>
#include <fstream>
#include <gtest/gtest.h>
#include <sstream>

namespace {

std::wstring TempDir(const wchar_t* leaf) {
    wchar_t path[MAX_PATH]{};
    const DWORD len = ::GetTempPathW(MAX_PATH, path);
    EXPECT_GT(len, 0u);
    const auto dir = std::filesystem::path(path) / L"MeetingBuddySettingsTests" / leaf;
    std::filesystem::remove_all(dir);
    std::filesystem::create_directories(dir);
    return dir.wstring();
}

std::string ReadFile(const std::wstring& path) {
    std::ifstream file(path, std::ios::binary);
    std::ostringstream ss;
    ss << file.rdbuf();
    return ss.str();
}

void WriteFile(const std::wstring& path, const std::string& body) {
    std::ofstream file(path, std::ios::binary);
    file << body;
}

SettingsData Sample() {
    SettingsData data{};
    data.closeToTray = false;
    data.startWithWindows = true;
    data.startMinimized = true;
    data.hotkeyEnabled = false;
    data.hotkey = L"Ctrl+Shift+M";
    data.firstRunTrayHintShown = true;
    data.windowPlacement = WindowPlacementSettings{40, 50, 800, 600};
    data.folder = L"C:\\Users\\\u6D4B\u8BD5\\Meetings";
    data.micDeviceId = L"{mic}";
    data.systemDeviceId = L"{sys}";
    data.micGain = 1.25f;
    data.micAgc = true;
    data.systemGain = 0.5f;
    data.saveStems = true;
    data.compressToM4a = true;
    data.keepWav = false;
    data.communicationsProcessing = true;
    data.model = L"ggml-small-q5_1.bin";
    data.language = L"en";
    data.autoTranscribe = true;
    data.threads = 4;
    data.useVad = true;
    return data;
}

void ExpectSame(const SettingsData& actual, const SettingsData& expected) {
    EXPECT_EQ(actual.closeToTray, expected.closeToTray);
    EXPECT_EQ(actual.startWithWindows, expected.startWithWindows);
    EXPECT_EQ(actual.startMinimized, expected.startMinimized);
    EXPECT_EQ(actual.hotkeyEnabled, expected.hotkeyEnabled);
    EXPECT_EQ(actual.hotkey, expected.hotkey);
    EXPECT_EQ(actual.firstRunTrayHintShown, expected.firstRunTrayHintShown);
    ASSERT_EQ(actual.windowPlacement.has_value(), expected.windowPlacement.has_value());
    if (expected.windowPlacement.has_value()) {
        EXPECT_EQ(actual.windowPlacement->x, expected.windowPlacement->x);
        EXPECT_EQ(actual.windowPlacement->y, expected.windowPlacement->y);
        EXPECT_EQ(actual.windowPlacement->width, expected.windowPlacement->width);
        EXPECT_EQ(actual.windowPlacement->height, expected.windowPlacement->height);
    }
    EXPECT_EQ(actual.folder, expected.folder);
    EXPECT_EQ(actual.micDeviceId, expected.micDeviceId);
    EXPECT_EQ(actual.systemDeviceId, expected.systemDeviceId);
    EXPECT_FLOAT_EQ(actual.micGain, expected.micGain);
    EXPECT_EQ(actual.micAgc, expected.micAgc);
    EXPECT_FLOAT_EQ(actual.systemGain, expected.systemGain);
    EXPECT_EQ(actual.saveStems, expected.saveStems);
    EXPECT_EQ(actual.compressToM4a, expected.compressToM4a);
    EXPECT_EQ(actual.keepWav, expected.keepWav);
    EXPECT_EQ(actual.communicationsProcessing, expected.communicationsProcessing);
    EXPECT_EQ(actual.model, expected.model);
    EXPECT_EQ(actual.language, expected.language);
    EXPECT_EQ(actual.autoTranscribe, expected.autoTranscribe);
    EXPECT_EQ(actual.threads, expected.threads);
    EXPECT_EQ(actual.useVad, expected.useVad);
}

} // namespace

TEST(Settings, MissingFileYieldsDefaults) {
    // Arrange
    const std::wstring path =
        (std::filesystem::path(TempDir(L"Missing")) / L"settings.json").wstring();

    // Act
    ASSERT_TRUE(Settings::Instance().LoadFromFile(path));

    // Assert
    ExpectSame(Settings::Instance().Data(), SettingsData{});
    EXPECT_FALSE(Settings::Instance().Data().windowPlacement.has_value());
}

TEST(Settings, RoundTripPreservesEveryField) {
    // Arrange
    const std::wstring path =
        (std::filesystem::path(TempDir(L"RoundTrip")) / L"settings.json").wstring();
    const SettingsData original = Sample();
    Settings::Instance().Data() = original;

    // Act
    ASSERT_TRUE(Settings::Instance().SaveToFile(path));
    ASSERT_TRUE(Settings::Instance().LoadFromFile(path));

    // Assert
    ExpectSame(Settings::Instance().Data(), original);
    const std::string body = ReadFile(path);
    EXPECT_NE(body.find("\"hotkey\": \"Ctrl+Shift+M\""), std::string::npos);
    EXPECT_NE(body.find("\"window\""), std::string::npos);
}

TEST(Settings, OmitsWindowUntilPlacementIsKnown) {
    // Arrange
    const std::wstring path =
        (std::filesystem::path(TempDir(L"NoWindow")) / L"settings.json").wstring();
    Settings::Instance().Data() = SettingsData{};

    // Act
    ASSERT_TRUE(Settings::Instance().SaveToFile(path));

    // Assert
    EXPECT_EQ(ReadFile(path).find("\"window\""), std::string::npos);
}

TEST(Settings, PartialFileKeepsDefaultsAndSkipsUnknownKeys) {
    // Arrange
    const std::wstring path =
        (std::filesystem::path(TempDir(L"Partial")) / L"settings.json").wstring();
    WriteFile(path, R"({
        // comment
        "general": { "closeToTray": false, "extra": 1 },
        "recording": { "micGain": 1.5 },
        "future": true
    })");

    // Act
    ASSERT_TRUE(Settings::Instance().LoadFromFile(path));

    // Assert
    const SettingsData& data = Settings::Instance().Data();
    EXPECT_FALSE(data.closeToTray);
    EXPECT_FLOAT_EQ(data.micGain, 1.5f);
    EXPECT_EQ(data.hotkey, L"Ctrl+Alt+R");
    EXPECT_FLOAT_EQ(data.systemGain, 0.8f);
    EXPECT_EQ(data.model, L"ggml-base-q5_1.bin");
    EXPECT_TRUE(data.keepWav);
    EXPECT_FALSE(data.windowPlacement.has_value());
}

TEST(Settings, DropsWindowWithNonPositiveSize) {
    // Arrange
    const std::wstring path =
        (std::filesystem::path(TempDir(L"BadWindow")) / L"settings.json").wstring();
    WriteFile(path, R"({
        "general": { "window": { "x": 1, "y": 2, "width": 0, "height": 10 } }
    })");

    // Act
    ASSERT_TRUE(Settings::Instance().LoadFromFile(path));

    // Assert
    EXPECT_FALSE(Settings::Instance().Data().windowPlacement.has_value());
}

TEST(Settings, UnreadableFileResetsToDefaults) {
    // Arrange
    const std::wstring path =
        (std::filesystem::path(TempDir(L"Unreadable")) / L"settings.json").wstring();
    WriteFile(path, "{");

    // Act
    const bool ok = Settings::Instance().LoadFromFile(path);

    // Assert
    EXPECT_FALSE(ok);
    ExpectSame(Settings::Instance().Data(), SettingsData{});
}
