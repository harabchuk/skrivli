#pragma once

#include <windows.h>

#include <memory>

// Posts a heap payload to a window. Ownership transfers to the receiver on success (it must call
// TakeOwned<T>); if the window is gone or the post fails, the payload is deleted here.
template <typename T>
bool PostOwned(HWND hwnd, UINT msg, std::unique_ptr<T> payload, WPARAM wParam = 0) {
    if (hwnd == nullptr || !::IsWindow(hwnd)) {
        return false;
    }
    if (!::PostMessageW(hwnd, msg, wParam, reinterpret_cast<LPARAM>(payload.get()))) {
        return false;
    }
    payload.release();
    return true;
}

// Reclaims ownership of a payload posted with PostOwned<T>.
template <typename T>
std::unique_ptr<T> TakeOwned(LPARAM lParam) {
    return std::unique_ptr<T>(reinterpret_cast<T*>(lParam));
}
