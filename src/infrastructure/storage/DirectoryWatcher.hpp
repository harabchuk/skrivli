#pragma once

#include <windows.h>

#include <wil/resource.h>

#include <atomic>
#include <string>
#include <thread>

// Background ReadDirectoryChangesW watcher. Posts notifyMsg to hwnd after a
// short coalesce delay. Never touches UI controls itself.
class DirectoryWatcher {
public:
    DirectoryWatcher() = default;
    ~DirectoryWatcher();

    DirectoryWatcher(const DirectoryWatcher&) = delete;
    auto operator=(const DirectoryWatcher&) -> DirectoryWatcher& = delete;

    bool Start(const std::wstring& folder, HWND hwnd, UINT notifyMsg);
    void Stop();
    auto Folder() const -> const std::wstring& { return m_folder; }

private:
    void ThreadMain();

    std::wstring m_folder;
    HWND m_hwnd = nullptr;
    UINT m_notifyMsg = 0;
    wil::unique_event_nothrow m_stopEvent;
    wil::unique_hfile m_dirHandle;
    std::thread m_thread;
    std::atomic<bool> m_running{false};
};
