#pragma once

#include <windows.h>

#include <string>

namespace Clipboard {

bool SetUnicodeText(HWND owner, const std::wstring& text);

} // namespace Clipboard
