#pragma once

#include <windows.h>

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
    DirectoryWatcher& operator=(const DirectoryWatcher&) = delete;

    bool Start(const std::wstring& folder, HWND hwnd, UINT notifyMsg);
    void Stop();
    bool Restart(const std::wstring& folder);

    const std::wstring& Folder() const { return m_folder; }

private:
    void ThreadMain();

    std::wstring m_folder;
    HWND m_hwnd = nullptr;
    UINT m_notifyMsg = 0;
    HANDLE m_stopEvent = nullptr;
    HANDLE m_dirHandle = nullptr;
    std::thread m_thread;
    std::atomic<bool> m_running{false};
};
