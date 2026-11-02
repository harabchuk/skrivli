#pragma once

#include <cstdint>
#include <string>
#include <string_view>

namespace Text {

auto WideToUtf8(const std::wstring& wide) -> std::string;
auto Utf8ToWide(const std::string& utf8) -> std::wstring;

bool EndsWithIgnoreCase(const std::wstring& value, std::wstring_view suffix);
bool PathEquals(const std::wstring& a, const std::wstring& b);

// "hh:mm:ss" with at least two digits per field. String is std::string or std::wstring.
template <typename String = std::wstring>
String FormatHms(uint64_t totalSeconds);
template <>
auto FormatHms<std::wstring>(uint64_t totalSeconds) -> std::wstring;
template <>
auto FormatHms<std::string>(uint64_t totalSeconds) -> std::string;

// "2026-11-01T13:18:00" -> "2026-11-01 13:18"; any other input is returned unchanged.
template <typename String>
String IsoToDisplayMinute(const String& iso) {
    using Char = typename String::value_type;
    constexpr size_t kMinuteEnd = 16;
    constexpr size_t kSeparatorIndex = 10;
    if (iso.size() >= kMinuteEnd && iso[kSeparatorIndex] == static_cast<Char>('T')) {
        auto out = iso.substr(0, kMinuteEnd);
        out[kSeparatorIndex] = static_cast<Char>(' ');
        return out;
    }
    return iso;
}

}  // namespace Text
