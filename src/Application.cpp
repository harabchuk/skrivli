#include "Application.hpp"

#include "storage/Settings.hpp"
#include "util/Log.hpp"
#include "util/Paths.hpp"

namespace {

constexpr wchar_t kMutexName[] = L"Local\\MeetingBuddy.SingleInstance";
constexpr wchar_t kMainWindowClass[] = L"MeetingBuddy.MainWindow";
constexpr wchar_t kShowYourselfMessage[] = L"MeetingBuddy.ShowYourself";

}  // namespace

bool Application::ClaimSingleInstance() {
    m_instanceMutex = ::CreateMutexW(nullptr, TRUE, kMutexName);
    if (m_instanceMutex == nullptr) {
        Log::Error(L"CreateMutex failed for single-instance guard");
        return false;
    }
    if (::GetLastError() == ERROR_ALREADY_EXISTS) {
        return false;
    }
    return true;
}

void Application::ActivateExistingInstance() const {
    const HWND existing = ::FindWindowW(kMainWindowClass, nullptr);
    if (existing == nullptr) {
        return;
    }
    const UINT msg = ::RegisterWindowMessageW(kShowYourselfMessage);
    if (msg != 0) {
        ::PostMessageW(existing, msg, 0, 0);
    }
}

void Application::ReleaseSingleInstance() {
    if (m_instanceMutex != nullptr) {
        ::CloseHandle(m_instanceMutex);
        m_instanceMutex = nullptr;
    }
}

int Application::Run(HINSTANCE /*instance*/, int showCommand) {
    if (!ClaimSingleInstance()) {
        ActivateExistingInstance();
        ReleaseSingleInstance();
        return 0;
    }

    Paths::EnsureAppDataDirectories();
    Settings::Instance().Load();
    Log::Info(L"MeetingBuddy starting");

    WTL::CMessageLoop messageLoop;
    _Module.AddMessageLoop(&messageLoop);

    if (m_mainWindow.CreateEx() == nullptr) {
        Log::Error(L"Failed to create main window");
        ReleaseSingleInstance();
        return 1;
    }

    m_mainWindow.RestoreWindowPlacement();

    int show = showCommand;
    if (Settings::Instance().Data().startMinimized) {
        show = SW_HIDE;
    }
    m_mainWindow.ShowWindow(show);
    m_mainWindow.UpdateWindow();

    const int result = messageLoop.Run();
    _Module.RemoveMessageLoop();

    Settings::Instance().Save();
    Log::Info(L"MeetingBuddy exiting");
    ReleaseSingleInstance();
    return result;
}
