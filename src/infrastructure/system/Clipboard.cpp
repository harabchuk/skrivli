#include "util/Clipboard.hpp"

bool Clipboard::SetUnicodeText(HWND owner, const std::wstring& text) {
    if (!::OpenClipboard(owner)) {
        return false;
    }
    ::EmptyClipboard();

    const auto bytes = (text.size() + 1) * sizeof(wchar_t);
    const auto mem = ::GlobalAlloc(GMEM_MOVEABLE, bytes);
    if (mem == nullptr) {
        ::CloseClipboard();
        return false;
    }

    auto* locked = ::GlobalLock(mem);
    if (locked == nullptr) {
        ::GlobalFree(mem);
        ::CloseClipboard();
        return false;
    }
    memcpy(locked, text.c_str(), bytes);
    ::GlobalUnlock(mem);

    if (::SetClipboardData(CF_UNICODETEXT, mem) == nullptr) {
        ::GlobalFree(mem);
        ::CloseClipboard();
        return false;
    }

    ::CloseClipboard();
    return true;
}
