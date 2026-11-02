#include "util/Paths.hpp"

#include <filesystem>
#include <shlobj.h>
#include <windows.h>

namespace {

auto KnownFolder(REFKNOWNFOLDERID id) -> std::wstring {
    PWSTR raw = nullptr;
    const auto hr = ::SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, nullptr, &raw);
    if (FAILED(hr) || raw == nullptr) {
        return {};
    }
    std::wstring path(raw);
    ::CoTaskMemFree(raw);
    return path;
}

auto Join(const std::wstring& root, const wchar_t* leaf) -> std::wstring {
    if (root.empty()) {
        return {};
    }
    std::filesystem::path path(root);
    path /= leaf;
    return path.wstring();
}

}  // namespace

namespace Paths {

auto SettingsDir() -> std::wstring {
    return Join(KnownFolder(FOLDERID_RoamingAppData), L"MeetingBuddy");
}

auto SettingsFile() -> std::wstring {
    return Join(SettingsDir(), L"settings.json");
}

auto ModelsDir() -> std::wstring {
    return Join(KnownFolder(FOLDERID_LocalAppData), L"MeetingBuddy\\models");
}

auto LogsDir() -> std::wstring {
    return Join(KnownFolder(FOLDERID_LocalAppData), L"MeetingBuddy\\logs");
}

auto RescueDir() -> std::wstring {
    return Join(KnownFolder(FOLDERID_LocalAppData), L"MeetingBuddy\\rescue");
}

auto DefaultRecordingsDir() -> std::wstring {
    return Join(KnownFolder(FOLDERID_Documents), L"MeetingBuddy");
}

bool EnsureDirectory(const std::wstring& path) {
    if (path.empty()) {
        return false;
    }
    std::error_code ec;
    std::filesystem::create_directories(path, ec);
    return !ec && std::filesystem::is_directory(path);
}

bool EnsureAppDataDirectories() {
    return EnsureDirectory(SettingsDir())
        && EnsureDirectory(ModelsDir())
        && EnsureDirectory(LogsDir())
        && EnsureDirectory(RescueDir())
        && EnsureDirectory(DefaultRecordingsDir());
}

}  // namespace Paths
