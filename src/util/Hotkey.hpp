#pragma once

#include <string>
#include <windows.h>

struct HotkeyChord {
    UINT modifiers = 0; // MOD_* flags
    UINT vk = 0;
    bool valid = false;
};

namespace Hotkey {

// Parses chords like "Ctrl+Alt+R", "Ctrl+Shift+F9". Case-insensitive.
HotkeyChord Parse(const std::wstring& text);

std::wstring Format(const HotkeyChord& chord);

bool Register(HWND hwnd, int id, const HotkeyChord& chord);
void Unregister(HWND hwnd, int id);

}  // namespace Hotkey
