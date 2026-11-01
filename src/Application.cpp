#include "Application.hpp"

int Application::Run(HINSTANCE /*instance*/, int showCommand) {
    WTL::CMessageLoop messageLoop;
    _Module.AddMessageLoop(&messageLoop);

    constexpr DWORD style = WS_OVERLAPPEDWINDOW & ~(WS_THICKFRAME | WS_MAXIMIZEBOX);
    if (m_mainWindow.CreateEx(nullptr, nullptr, style) == nullptr) {
        return 1;
    }

    m_mainWindow.SetWindowText(L"MeetingBuddy");
    m_mainWindow.ResizeClient(420, 280);
    m_mainWindow.CenterWindow();
    m_mainWindow.ShowWindow(showCommand);
    m_mainWindow.UpdateWindow();

    const int result = messageLoop.Run();
    _Module.RemoveMessageLoop();
    return result;
}
