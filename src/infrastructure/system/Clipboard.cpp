#include "infrastructure/system/Clipboard.hpp"

#include <wil/resource.h>

bool Clipboard::SetUnicodeText(HWND owner, const std::wstring& text) {
    if (!::OpenClipboard(owner)) {
        return false;
    }
    const auto closeClipboard = wil::scope_exit([] { ::CloseClipboard(); });
    ::EmptyClipboard();

    const auto bytes = (text.size() + 1) * sizeof(wchar_t);
    wil::unique_hglobal mem(::GlobalAlloc(GMEM_MOVEABLE, bytes));
    if (!mem) {
        return false;
    }

    auto* locked = ::GlobalLock(mem.get());
    if (locked == nullptr) {
        return false;
    }
    memcpy(locked, text.c_str(), bytes);
    ::GlobalUnlock(mem.get());

    if (::SetClipboardData(CF_UNICODETEXT, mem.get()) == nullptr) {
        return false;
    }
    mem.release();
    return true;
}
