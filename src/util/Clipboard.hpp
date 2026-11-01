#pragma once

#include <string>
#include <windows.h>

namespace Clipboard {

bool SetUnicodeText(HWND owner, const std::wstring& text);

}  // namespace Clipboard
