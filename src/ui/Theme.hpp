#pragma once

#include "pch.hpp"

// Shared palette so every view paints the same surfaces and accents.
namespace ui {

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

// Section headers ("Input", "Output") and the selected tab label.
constexpr COLORREF kAccent = RGB(0x00, 0x5F, 0xB8);
// Hairline separators (section rules, tab strip underline).
constexpr COLORREF kDivider = RGB(0xD0, 0xD0, 0xD0);
// Hovered, unselected tab label.
constexpr COLORREF kHotText = RGB(0x33, 0x33, 0x33);

}  // namespace ui
