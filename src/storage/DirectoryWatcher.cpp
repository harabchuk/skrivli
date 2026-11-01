#include "storage/DirectoryWatcher.hpp"

#include "util/Log.hpp"

#include <vector>

namespace {

constexpr DWORD kNotifyFilter =
    FILE_NOTIFY_CHANGE_FILE_NAME
    | FILE_NOTIFY_CHANGE_DIR_NAME
    | FILE_NOTIFY_CHANGE_SIZE
    | FILE_NOTIFY_CHANGE_LAST_WRITE;

constexpr DWORD kCoalesceMs = 200;

}  // namespace

DirectoryWatcher::~DirectoryWatcher() {
    Stop();
}

bool DirectoryWatcher::Start(const std::wstring& folder, HWND hwnd, UINT notifyMsg) {
    Stop();
    if (folder.empty() || hwnd == nullptr || notifyMsg == 0) {
        return false;
    }

    m_folder = folder;
    m_hwnd = hwnd;
    m_notifyMsg = notifyMsg;

    m_stopEvent = ::CreateEventW(nullptr, TRUE, FALSE, nullptr);
    if (m_stopEvent == nullptr) {
        return false;
    }

    m_dirHandle = ::CreateFileW(
        folder.c_str(),
        FILE_LIST_DIRECTORY,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        nullptr,
        OPEN_EXISTING,
        FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
        nullptr);
    if (m_dirHandle == INVALID_HANDLE_VALUE) {
        Log::Warn(L"DirectoryWatcher could not open folder: " + folder);
        ::CloseHandle(m_stopEvent);
        m_stopEvent = nullptr;
        m_dirHandle = nullptr;
        return false;
    }

    m_running.store(true, std::memory_order_release);
    try {
        m_thread = std::thread([this]() { ThreadMain(); });
    } catch (...) {
        Stop();
        return false;
    }
    return true;
}

void DirectoryWatcher::Stop() {
    m_running.store(false, std::memory_order_release);
    if (m_stopEvent != nullptr) {
        ::SetEvent(m_stopEvent);
    }
    if (m_dirHandle != nullptr && m_dirHandle != INVALID_HANDLE_VALUE) {
        ::CancelIoEx(m_dirHandle, nullptr);
    }
    if (m_thread.joinable()) {
        m_thread.join();
    }
    if (m_dirHandle != nullptr && m_dirHandle != INVALID_HANDLE_VALUE) {
        ::CloseHandle(m_dirHandle);
        m_dirHandle = nullptr;
    }
    if (m_stopEvent != nullptr) {
        ::CloseHandle(m_stopEvent);
        m_stopEvent = nullptr;
    }
}

bool DirectoryWatcher::Restart(const std::wstring& folder) {
    const HWND hwnd = m_hwnd;
    const UINT msg = m_notifyMsg;
    return Start(folder, hwnd, msg);
}

void DirectoryWatcher::ThreadMain() {
    std::vector<BYTE> buffer(64 * 1024);

    while (m_running.load(std::memory_order_acquire)) {
        OVERLAPPED overlapped{};
        overlapped.hEvent = ::CreateEventW(nullptr, TRUE, FALSE, nullptr);
        if (overlapped.hEvent == nullptr) {
            break;
        }

        DWORD bytes = 0;
        const BOOL started = ::ReadDirectoryChangesW(
            m_dirHandle,
            buffer.data(),
            static_cast<DWORD>(buffer.size()),
            FALSE,
            kNotifyFilter,
            &bytes,
            &overlapped,
            nullptr);

        if (!started) {
            const DWORD err = ::GetLastError();
            ::CloseHandle(overlapped.hEvent);
            if (err == ERROR_NOTIFY_ENUM_DIR) {
                if (m_hwnd != nullptr && ::IsWindow(m_hwnd)) {
                    ::PostMessageW(m_hwnd, m_notifyMsg, 1, 0);
                }
                continue;
            }
            break;
        }

        HANDLE waitHandles[2] = {m_stopEvent, overlapped.hEvent};
        const DWORD wait = ::WaitForMultipleObjects(2, waitHandles, FALSE, INFINITE);
        if (wait == WAIT_OBJECT_0) {
            ::CancelIoEx(m_dirHandle, &overlapped);
            ::CloseHandle(overlapped.hEvent);
            break;
        }

        DWORD transferred = 0;
        if (!::GetOverlappedResult(m_dirHandle, &overlapped, &transferred, FALSE)) {
            const DWORD err = ::GetLastError();
            ::CloseHandle(overlapped.hEvent);
            if (err == ERROR_NOTIFY_ENUM_DIR) {
                if (m_hwnd != nullptr && ::IsWindow(m_hwnd)) {
                    ::PostMessageW(m_hwnd, m_notifyMsg, 1, 0);
                }
                continue;
            }
            if (err == ERROR_OPERATION_ABORTED) {
                break;
            }
            break;
        }
        ::CloseHandle(overlapped.hEvent);

        // Coalesce rapid bursts into one UI notification.
        if (::WaitForSingleObject(m_stopEvent, kCoalesceMs) == WAIT_OBJECT_0) {
            break;
        }

        if (m_hwnd != nullptr && ::IsWindow(m_hwnd)) {
            ::PostMessageW(m_hwnd, m_notifyMsg, 0, 0);
        }
    }
}
