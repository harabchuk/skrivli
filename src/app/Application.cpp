#include "Application.hpp"

#include "infrastructure/common/Log.hpp"
#include "infrastructure/common/Paths.hpp"
#include "infrastructure/storage/Settings.hpp"
#include "infrastructure/system/Autostart.hpp"

#include <shellapi.h>
#include <wil/resource.h>

namespace {

constexpr wchar_t kMutexName[] = L"Local\\MeetingBuddy.SingleInstance";
constexpr wchar_t kMainWindowClass[] = L"MeetingBuddy.MainWindow";
constexpr wchar_t kShowYourselfMessage[] = L"MeetingBuddy.ShowYourself";

} // namespace

bool Application::HasTrayArgument() {
    int argc = 0;
    wil::unique_hlocal argvMemory(::CommandLineToArgvW(::GetCommandLineW(), &argc));
    if (!argvMemory) {
        return false;
    }
    const auto argv = static_cast<wchar_t**>(argvMemory.get());
    bool tray = false;
    for (int i = 1; i < argc; ++i) {
        if (_wcsicmp(argv[i], L"--tray") == 0) {
            tray = true;
            break;
        }
    }
    return tray;
}

bool Application::ClaimSingleInstance() {
    m_instanceMutex.reset(::CreateMutexW(nullptr, TRUE, kMutexName));
    if (!m_instanceMutex) {
        Log::Error(L"CreateMutex failed for single-instance guard");
        return false;
    }
    if (::GetLastError() == ERROR_ALREADY_EXISTS) {
        return false;
    }
    return true;
}

void Application::ActivateExistingInstance() const {
    const auto existing = ::FindWindowW(kMainWindowClass, nullptr);
    if (existing == nullptr) {
        return;
    }
    const auto msg = ::RegisterWindowMessageW(kShowYourselfMessage);
    if (msg != 0) {
        ::PostMessageW(existing, msg, 0, 0);
    }
}

void Application::ReleaseSingleInstance() {
    m_instanceMutex.reset();
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

    // Keep the Run key in sync with saved settings (repairs path after moves).
    Autostart::Apply(Settings::Instance().Data().startWithWindows,
                     Settings::Instance().Data().startMinimized);

    WTL::CMessageLoop messageLoop;
    _Module.AddMessageLoop(&messageLoop);

    if (m_mainWindow.CreateEx() == nullptr) {
        Log::Error(L"Failed to create main window");
        ReleaseSingleInstance();
        return 1;
    }

    m_mainWindow.RestoreWindowPlacement();

    const bool startHidden = Settings::Instance().Data().startMinimized || HasTrayArgument();
    if (startHidden) {
        m_mainWindow.ApplyStartupVisibility(true);
    } else {
        m_mainWindow.ShowWindow(showCommand);
        m_mainWindow.UpdateWindow();
    }

    const int result = messageLoop.Run();
    _Module.RemoveMessageLoop();

    Settings::Instance().Save();
    Log::Info(L"MeetingBuddy exiting");
    ReleaseSingleInstance();
    return result;
}
