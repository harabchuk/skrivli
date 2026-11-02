#include "infrastructure/system/Hotkey.hpp"

#include <cctype>
#include <vector>

namespace {

auto Trim(std::wstring value) -> std::wstring {
    while (!value.empty() && iswspace(value.front())) {
        value.erase(value.begin());
    }
    while (!value.empty() && iswspace(value.back())) {
        value.pop_back();
    }
    return value;
}

auto ToLower(std::wstring value) -> std::wstring {
    for (auto& ch : value) {
        ch = static_cast<wchar_t>(towlower(ch));
    }
    return value;
}

auto SplitPlus(const std::wstring& text) -> std::vector<std::wstring> {
    std::vector<std::wstring> parts;
    std::wstring current;
    for (wchar_t ch : text) {
        if (ch == L'+') {
            parts.push_back(Trim(current));
            current.clear();
        } else {
            current.push_back(ch);
        }
    }
    parts.push_back(Trim(current));
    return parts;
}

UINT ParseKeyToken(const std::wstring& token) {
    const auto lower = ToLower(token);
    if (lower == L"ctrl" || lower == L"control") {
        return 0; // modifier
    }
    if (lower == L"alt") {
        return 0;
    }
    if (lower == L"shift") {
        return 0;
    }
    if (lower == L"win" || lower == L"windows") {
        return 0;
    }
    if (lower.size() == 1) {
        const wchar_t ch = towupper(lower[0]);
        if (ch >= L'A' && ch <= L'Z') {
            return static_cast<UINT>(ch);
        }
        if (ch >= L'0' && ch <= L'9') {
            return static_cast<UINT>(ch);
        }
    }
    if (lower.size() >= 2 && lower[0] == L'f') {
        wchar_t* end = nullptr;
        const long n = std::wcstol(lower.c_str() + 1, &end, 10);
        if (end != lower.c_str() + 1 && n >= 1 && n <= 24) {
            return static_cast<UINT>(VK_F1 + (n - 1));
        }
    }
    return 0;
}

bool IsModifierToken(const std::wstring& token) {
    const auto lower = ToLower(token);
    return lower == L"ctrl" || lower == L"control" || lower == L"alt" || lower == L"shift"
        || lower == L"win" || lower == L"windows";
}

}  // namespace

namespace Hotkey {

HotkeyChord Parse(const std::wstring& text) {
    HotkeyChord chord{};
    const auto parts = SplitPlus(text);
    if (parts.empty()) {
        return chord;
    }

    UINT modifiers = 0;
    UINT vk = 0;
    for (const auto& part : parts) {
        if (part.empty()) {
            continue;
        }
        const auto lower = ToLower(part);
        if (lower == L"ctrl" || lower == L"control") {
            modifiers |= MOD_CONTROL;
            continue;
        }
        if (lower == L"alt") {
            modifiers |= MOD_ALT;
            continue;
        }
        if (lower == L"shift") {
            modifiers |= MOD_SHIFT;
            continue;
        }
        if (lower == L"win" || lower == L"windows") {
            modifiers |= MOD_WIN;
            continue;
        }
        if (vk != 0) {
            return HotkeyChord{};
        }
        vk = ParseKeyToken(part);
        if (vk == 0 || IsModifierToken(part)) {
            return HotkeyChord{};
        }
    }

    if (vk == 0 || modifiers == 0) {
        return HotkeyChord{};
    }
    chord.modifiers = modifiers | MOD_NOREPEAT;
    chord.vk = vk;
    chord.valid = true;
    return chord;
}

auto Format(const HotkeyChord& chord) -> std::wstring {
    if (!chord.valid) {
        return {};
    }
    std::wstring text;
    const UINT mods = chord.modifiers & ~static_cast<UINT>(MOD_NOREPEAT);
    if (mods & MOD_CONTROL) {
        text += L"Ctrl+";
    }
    if (mods & MOD_ALT) {
        text += L"Alt+";
    }
    if (mods & MOD_SHIFT) {
        text += L"Shift+";
    }
    if (mods & MOD_WIN) {
        text += L"Win+";
    }
    if (chord.vk >= VK_F1 && chord.vk <= VK_F24) {
        text += L"F" + std::to_wstring(chord.vk - VK_F1 + 1);
    } else if ((chord.vk >= L'A' && chord.vk <= L'Z') || (chord.vk >= L'0' && chord.vk <= L'9')) {
        text.push_back(static_cast<wchar_t>(chord.vk));
    } else {
        return {};
    }
    return text;
}

bool Register(HWND hwnd, int id, const HotkeyChord& chord) {
    if (hwnd == nullptr || !chord.valid) {
        return false;
    }
    return ::RegisterHotKey(hwnd, id, chord.modifiers, chord.vk) != FALSE;
}

void Unregister(HWND hwnd, int id) {
    if (hwnd != nullptr) {
        ::UnregisterHotKey(hwnd, id);
    }
}

}  // namespace Hotkey
