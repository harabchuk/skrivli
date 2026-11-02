#include "util/Autostart.hpp"

#include "util/Log.hpp"

#include <windows.h>

#include <string>

namespace {

constexpr wchar_t kRunKey[] = L"Software\\Microsoft\\Windows\\CurrentVersion\\Run";

auto QuotedExePath() -> std::wstring {
    wchar_t path[MAX_PATH]{};
    const auto len = ::GetModuleFileNameW(nullptr, path, MAX_PATH);
    if (len == 0 || len >= MAX_PATH) {
        return {};
    }
    return L"\"" + std::wstring(path) + L"\"";
}

}  // namespace

namespace Autostart {

auto CommandLineForRunKey(bool startMinimized) -> std::wstring {
    auto cmd = QuotedExePath();
    if (cmd.empty()) {
        return {};
    }
    if (startMinimized) {
        cmd += L" --tray";
    }
    return cmd;
}

bool IsEnabled() {
    HKEY key = nullptr;
    if (::RegOpenKeyExW(HKEY_CURRENT_USER, kRunKey, 0, KEY_READ, &key) != ERROR_SUCCESS) {
        return false;
    }
    DWORD type = 0;
    DWORD size = 0;
    const auto rc = ::RegQueryValueExW(key, kRunValueName, nullptr, &type, nullptr, &size);
    ::RegCloseKey(key);
    return rc == ERROR_SUCCESS && type == REG_SZ;
}

bool Apply(bool enable, bool startMinimized) {
    HKEY key = nullptr;
    const auto openRc = ::RegOpenKeyExW(HKEY_CURRENT_USER, kRunKey, 0, KEY_SET_VALUE | KEY_QUERY_VALUE, &key);
    if (openRc != ERROR_SUCCESS) {
        Log::Error(L"Failed to open HKCU Run key for autostart");
        return false;
    }

    bool ok = true;
    if (enable) {
        const auto cmd = CommandLineForRunKey(startMinimized);
        if (cmd.empty()) {
            ::RegCloseKey(key);
            return false;
        }
        const auto rc = ::RegSetValueExW(
            key,
            kRunValueName,
            0,
            REG_SZ,
            reinterpret_cast<const BYTE*>(cmd.c_str()),
            static_cast<DWORD>((cmd.size() + 1) * sizeof(wchar_t)));
        if (rc != ERROR_SUCCESS) {
            Log::Error(L"Failed to set Start with Windows Run value");
            ok = false;
        }
    } else {
        const auto rc = ::RegDeleteValueW(key, kRunValueName);
        if (rc != ERROR_SUCCESS && rc != ERROR_FILE_NOT_FOUND) {
            Log::Error(L"Failed to remove Start with Windows Run value");
            ok = false;
        }
    }

    ::RegCloseKey(key);
    return ok;
}

}  // namespace Autostart
