#pragma once

#include <windows.h>
#include <objbase.h>
#include <avrt.h>
#include <mfapi.h>
#include <winhttp.h>

#include <memory>

// Small RAII wrappers for process/thread-scoped Win32 state.
// For kernel handles prefer ATL `CHandle`; for CoTaskMem strings prefer `CComHeapPtr<wchar_t>`.

// Balances CoInitializeEx on the owning thread. A thread that already runs in a different
// apartment (RPC_E_CHANGED_MODE) is usable but must not call CoUninitialize.
class ScopedComInit {
public:
    explicit ScopedComInit(DWORD coInit = COINIT_MULTITHREADED) noexcept
        : m_hr(::CoInitializeEx(nullptr, coInit)) {}

    ~ScopedComInit() {
        if (SUCCEEDED(m_hr)) {
            ::CoUninitialize();
        }
    }

    ScopedComInit(const ScopedComInit&) = delete;
    ScopedComInit& operator=(const ScopedComInit&) = delete;

    HRESULT Result() const noexcept { return m_hr; }

    // True when COM is usable on this thread (S_OK, S_FALSE, or an existing apartment).
    bool IsUsable() const noexcept { return SUCCEEDED(m_hr) || m_hr == RPC_E_CHANGED_MODE; }

private:
    HRESULT m_hr;
};

// Registers the calling thread with MMCSS for the given task and reverts it on destruction.
class ScopedMmcss {
public:
    explicit ScopedMmcss(const wchar_t* taskName) noexcept {
        DWORD taskIndex = 0;
        m_handle = ::AvSetMmThreadCharacteristicsW(taskName, &taskIndex);
    }

    ~ScopedMmcss() {
        if (m_handle != nullptr) {
            ::AvRevertMmThreadCharacteristics(m_handle);
        }
    }

    ScopedMmcss(const ScopedMmcss&) = delete;
    ScopedMmcss& operator=(const ScopedMmcss&) = delete;

    bool IsRegistered() const noexcept { return m_handle != nullptr; }

private:
    HANDLE m_handle = nullptr;
};

// Pairs MFStartup with MFShutdown; shutdown only runs when startup succeeded.
class ScopedMfStartup {
public:
    explicit ScopedMfStartup(DWORD flags = MFSTARTUP_LITE) noexcept
        : m_hr(::MFStartup(MF_VERSION, flags)) {}

    ~ScopedMfStartup() {
        if (SUCCEEDED(m_hr)) {
            ::MFShutdown();
        }
    }

    ScopedMfStartup(const ScopedMfStartup&) = delete;
    ScopedMfStartup& operator=(const ScopedMfStartup&) = delete;

    HRESULT Result() const noexcept { return m_hr; }
    bool IsStarted() const noexcept { return SUCCEEDED(m_hr); }

private:
    HRESULT m_hr;
};

struct WinHttpHandleDeleter {
    using pointer = HINTERNET;

    void operator()(HINTERNET handle) const noexcept {
        if (handle != nullptr) {
            ::WinHttpCloseHandle(handle);
        }
    }
};

// Owns a session, connection, or request HINTERNET. Declare in open order so handles close
// in reverse (request, connect, session).
using UniqueWinHttp = std::unique_ptr<void, WinHttpHandleDeleter>;
