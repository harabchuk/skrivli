#pragma once

#include "pch.hpp"

#include "Version.hpp"

// Shared palette so every view paints the same surfaces and accents.
namespace ui {

inline int Scale(int px, UINT dpi) {
    return ::MulDiv(px, static_cast<int>(dpi), 96);
}

inline int ShowMessage(HWND hwnd, const wchar_t* text, UINT flags) {
    return ::MessageBoxW(hwnd, text, kAppName, flags);
}

inline COLORREF WindowColor() {
    return ::GetSysColor(COLOR_WINDOW);
}

inline HBRUSH WindowBrush() {
    return ::GetSysColorBrush(COLOR_WINDOW);
}

inline COLORREF TextColor() {
    return ::GetSysColor(COLOR_WINDOWTEXT);
}

inline COLORREF SecondaryTextColor() {
    return ::GetSysColor(COLOR_GRAYTEXT);
}

// Section headers ("Input", "Output").
constexpr COLORREF kAccent = RGB(0x00, 0x5F, 0xB8);
// Hairline separators under section headers.
constexpr COLORREF kDivider = RGB(0xD0, 0xD0, 0xD0);

}  // namespace ui
