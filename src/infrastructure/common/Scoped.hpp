#pragma once

#include <windows.h>
#include <objbase.h>
#include <avrt.h>
#include <mfapi.h>

#include <wil/resource.h>

// Process/thread-scoped RAII that WIL's own helpers do not match.
// A thread already in a different apartment (RPC_E_CHANGED_MODE) is usable, but must not
// call CoUninitialize. wil::CoInitializeEx() is unavailable under WIL_SUPPRESS_EXCEPTIONS
// and would uninitialize that other apartment.

class ScopedComInit {
public:
    explicit ScopedComInit(DWORD coInit = COINIT_MULTITHREADED) noexcept
        : m_hr(::CoInitializeEx(nullptr, coInit)) {
        if (FAILED(m_hr)) {
            m_uninit.release();
        }
    }

    ScopedComInit(const ScopedComInit&) = delete;
    auto operator=(const ScopedComInit&) -> ScopedComInit& = delete;

    HRESULT Result() const noexcept { return m_hr; }

    // True when COM is usable on this thread (S_OK, S_FALSE, or an existing apartment).
    bool IsUsable() const noexcept { return SUCCEEDED(m_hr) || m_hr == RPC_E_CHANGED_MODE; }

private:
    HRESULT m_hr;
    wil::unique_couninitialize_call m_uninit;
};

// Registers the calling thread with MMCSS for the given task and reverts it on destruction.
class ScopedMmcss {
public:
    explicit ScopedMmcss(const wchar_t* taskName) noexcept {
        DWORD taskIndex = 0;
        m_handle.reset(::AvSetMmThreadCharacteristicsW(taskName, &taskIndex));
    }

    ScopedMmcss(const ScopedMmcss&) = delete;
    auto operator=(const ScopedMmcss&) -> ScopedMmcss& = delete;

    bool IsRegistered() const noexcept { return static_cast<bool>(m_handle); }

private:
    using Handle = wil::unique_any<HANDLE, decltype(&::AvRevertMmThreadCharacteristics), ::AvRevertMmThreadCharacteristics>;

    Handle m_handle;
};

// Pairs MFStartup with MFShutdown; shutdown only runs when startup succeeded.
class ScopedMfStartup {
public:
    explicit ScopedMfStartup(DWORD flags = MFSTARTUP_LITE) noexcept
        : m_hr(::MFStartup(MF_VERSION, flags)) {
        if (FAILED(m_hr)) {
            m_shutdown.release();
        }
    }

    ScopedMfStartup(const ScopedMfStartup&) = delete;
    auto operator=(const ScopedMfStartup&) -> ScopedMfStartup& = delete;

    HRESULT Result() const noexcept { return m_hr; }
    bool IsStarted() const noexcept { return SUCCEEDED(m_hr); }

private:
    using ShutdownCall = wil::unique_call<decltype(&::MFShutdown), ::MFShutdown>;

    HRESULT m_hr;
    ShutdownCall m_shutdown;
};
