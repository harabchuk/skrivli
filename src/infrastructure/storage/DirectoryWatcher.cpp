#include "infrastructure/storage/DirectoryWatcher.hpp"

#include "infrastructure/common/Log.hpp"

#include <vector>

namespace {

constexpr DWORD kNotifyFilter = FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_DIR_NAME |
                                FILE_NOTIFY_CHANGE_SIZE | FILE_NOTIFY_CHANGE_LAST_WRITE;

constexpr DWORD kCoalesceMs = 200;

} // namespace

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

    if (!m_stopEvent.try_create(wil::EventOptions::ManualReset, nullptr)) {
        return false;
    }

    m_dirHandle.reset(::CreateFileW(
        folder.c_str(), FILE_LIST_DIRECTORY, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        nullptr, OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED, nullptr));
    if (!m_dirHandle) {
        Log::Warn(L"DirectoryWatcher could not open folder: " + folder);
        m_stopEvent.reset();
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
    if (m_stopEvent) {
        m_stopEvent.SetEvent();
    }
    if (m_dirHandle) {
        ::CancelIoEx(m_dirHandle.get(), nullptr);
    }
    if (m_thread.joinable()) {
        m_thread.join();
    }
    m_dirHandle.reset();
    m_stopEvent.reset();
}

void DirectoryWatcher::ThreadMain() {
    std::vector<BYTE> buffer(64 * 1024);

    while (m_running.load(std::memory_order_acquire)) {
        wil::unique_event_nothrow changeEvent;
        if (!changeEvent.try_create(wil::EventOptions::ManualReset, nullptr)) {
            break;
        }

        OVERLAPPED overlapped{};
        overlapped.hEvent = changeEvent.get();

        DWORD bytes = 0;
        const BOOL started = ::ReadDirectoryChangesW(m_dirHandle.get(), buffer.data(),
                                                     static_cast<DWORD>(buffer.size()), FALSE,
                                                     kNotifyFilter, &bytes, &overlapped, nullptr);

        if (!started) {
            const auto err = ::GetLastError();
            if (err == ERROR_NOTIFY_ENUM_DIR) {
                if (m_hwnd != nullptr && ::IsWindow(m_hwnd)) {
                    ::PostMessageW(m_hwnd, m_notifyMsg, 1, 0);
                }
                continue;
            }
            break;
        }

        const HANDLE waitHandles[2] = {m_stopEvent.get(), overlapped.hEvent};
        const auto wait = ::WaitForMultipleObjects(2, waitHandles, FALSE, INFINITE);
        if (wait == WAIT_OBJECT_0) {
            ::CancelIoEx(m_dirHandle.get(), &overlapped);
            break;
        }

        DWORD transferred = 0;
        if (!::GetOverlappedResult(m_dirHandle.get(), &overlapped, &transferred, FALSE)) {
            const auto err = ::GetLastError();
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

        // Coalesce rapid bursts into one UI notification.
        if (::WaitForSingleObject(m_stopEvent.get(), kCoalesceMs) == WAIT_OBJECT_0) {
            break;
        }

        if (m_hwnd != nullptr && ::IsWindow(m_hwnd)) {
            ::PostMessageW(m_hwnd, m_notifyMsg, 0, 0);
        }
    }
}
