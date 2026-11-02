#include "infrastructure/system/Autostart.hpp"

#include "infrastructure/common/Log.hpp"

#include <windows.h>

#include <wil/registry.h>

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
    wil::unique_hkey key;
    if (FAILED(wil::reg::open_unique_key_nothrow(HKEY_CURRENT_USER, kRunKey, key))) {
        return false;
    }
    DWORD type = 0;
    DWORD size = 0;
    const auto rc = ::RegQueryValueExW(key.get(), kRunValueName, nullptr, &type, nullptr, &size);
    return rc == ERROR_SUCCESS && type == REG_SZ;
}

bool Apply(bool enable, bool startMinimized) {
    wil::unique_hkey key;
    const auto openHr = wil::reg::open_unique_key_nothrow(
        HKEY_CURRENT_USER, kRunKey, key, wil::reg::key_access::readwrite);
    if (FAILED(openHr)) {
        Log::Error(L"Failed to open HKCU Run key for autostart");
        return false;
    }

    bool ok = true;
    if (enable) {
        const auto cmd = CommandLineForRunKey(startMinimized);
        if (cmd.empty()) {
            return false;
        }
        const auto rc = ::RegSetValueExW(
            key.get(),
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
        const auto rc = ::RegDeleteValueW(key.get(), kRunValueName);
        if (rc != ERROR_SUCCESS && rc != ERROR_FILE_NOT_FOUND) {
            Log::Error(L"Failed to remove Start with Windows Run value");
            ok = false;
        }
    }

    return ok;
}

}  // namespace Autostart
