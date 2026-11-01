#include "util/Paths.hpp"

#include <filesystem>
#include <shlobj.h>
#include <windows.h>

namespace {

std::wstring KnownFolder(REFKNOWNFOLDERID id) {
    PWSTR raw = nullptr;
    const HRESULT hr = ::SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, nullptr, &raw);
    if (FAILED(hr) || raw == nullptr) {
        return {};
    }
    std::wstring path(raw);
    ::CoTaskMemFree(raw);
    return path;
}

std::wstring Join(const std::wstring& root, const wchar_t* leaf) {
    if (root.empty()) {
        return {};
    }
    std::filesystem::path path(root);
    path /= leaf;
    return path.wstring();
}

}  // namespace

namespace Paths {

std::wstring SettingsDir() {
    return Join(KnownFolder(FOLDERID_RoamingAppData), L"MeetingBuddy");
}

std::wstring SettingsFile() {
    return Join(SettingsDir(), L"settings.ini");
}

std::wstring ModelsDir() {
    return Join(KnownFolder(FOLDERID_LocalAppData), L"MeetingBuddy\\models");
}

std::wstring LogsDir() {
    return Join(KnownFolder(FOLDERID_LocalAppData), L"MeetingBuddy\\logs");
}

std::wstring RescueDir() {
    return Join(KnownFolder(FOLDERID_LocalAppData), L"MeetingBuddy\\rescue");
}

std::wstring DefaultRecordingsDir() {
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
