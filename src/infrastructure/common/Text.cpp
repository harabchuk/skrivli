#include "infrastructure/common/Text.hpp"

#include <windows.h>

#include <cstdio>
#include <cwchar>

namespace Text {

auto WideToUtf8(const std::wstring& wide) -> std::string {
    if (wide.empty()) {
        return {};
    }
    const int size = ::WideCharToMultiByte(CP_UTF8, 0, wide.c_str(), static_cast<int>(wide.size()),
                                           nullptr, 0, nullptr, nullptr);
    if (size <= 0) {
        return {};
    }
    std::string out(static_cast<size_t>(size), '\0');
    ::WideCharToMultiByte(CP_UTF8, 0, wide.c_str(), static_cast<int>(wide.size()), out.data(), size,
                          nullptr, nullptr);
    return out;
}

auto Utf8ToWide(const std::string& utf8) -> std::wstring {
    if (utf8.empty()) {
        return {};
    }
    const int size =
        ::MultiByteToWideChar(CP_UTF8, 0, utf8.c_str(), static_cast<int>(utf8.size()), nullptr, 0);
    if (size <= 0) {
        return {};
    }
    std::wstring out(static_cast<size_t>(size), L'\0');
    ::MultiByteToWideChar(CP_UTF8, 0, utf8.c_str(), static_cast<int>(utf8.size()), out.data(),
                          size);
    return out;
}

bool EndsWithIgnoreCase(const std::wstring& value, std::wstring_view suffix) {
    if (value.size() < suffix.size()) {
        return false;
    }
    return _wcsnicmp(value.c_str() + (value.size() - suffix.size()), suffix.data(),
                     suffix.size()) == 0;
}

bool PathEquals(const std::wstring& a, const std::wstring& b) {
    return _wcsicmp(a.c_str(), b.c_str()) == 0;
}

template <> auto FormatHms<std::wstring>(uint64_t totalSeconds) -> std::wstring {
    const unsigned long long hours = totalSeconds / 3600;
    const unsigned long long minutes = (totalSeconds / 60) % 60;
    const unsigned long long seconds = totalSeconds % 60;
    wchar_t buffer[32]{};
    std::swprintf(buffer, 32, L"%02llu:%02llu:%02llu", hours, minutes, seconds);
    return buffer;
}

template <> auto FormatHms<std::string>(uint64_t totalSeconds) -> std::string {
    const unsigned long long hours = totalSeconds / 3600;
    const unsigned long long minutes = (totalSeconds / 60) % 60;
    const unsigned long long seconds = totalSeconds % 60;
    char buffer[32]{};
    std::snprintf(buffer, sizeof(buffer), "%02llu:%02llu:%02llu", hours, minutes, seconds);
    return buffer;
}

} // namespace Text
