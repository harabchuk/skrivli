#include "MainWindow.hpp"

LRESULT MainWindow::OnCreate(LPCREATESTRUCT /*createStruct*/) {
    CRect buttonRect(0, 0, kButtonWidth, kButtonHeight);
    m_recordButton.Create(
        m_hWnd,
        buttonRect,
        L"Record",
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_PUSHBUTTON,
        0,
        IDC_RECORD);

    LayoutControls();
    return 0;
}

void MainWindow::OnSize(UINT /*type*/, CSize /*size*/) {
    LayoutControls();
}

void MainWindow::OnClose() {
    DestroyWindow();
}

void MainWindow::OnDestroy() {
    PostQuitMessage(0);
}

LRESULT MainWindow::OnEraseBkgnd(HDC hdc) {
    CRect client;
    GetClientRect(&client);
    FillRect(hdc, &client, reinterpret_cast<HBRUSH>(COLOR_WINDOW + 1));
    return 1;
}

void MainWindow::OnRecordClick(UINT /*code*/, int /*id*/, HWND /*control*/) {
    m_recording = !m_recording;
    m_recordButton.SetWindowText(m_recording ? L"Stop" : L"Record");
}

void MainWindow::LayoutControls() {
    if (!m_recordButton.IsWindow()) {
        return;
    }

    CRect client;
    GetClientRect(&client);

    const int left = (client.Width() - kButtonWidth) / 2;
    const int top = (client.Height() - kButtonHeight) / 2;
    m_recordButton.SetWindowPos(
        nullptr,
        left,
        top,
        kButtonWidth,
        kButtonHeight,
        SWP_NOZORDER | SWP_NOACTIVATE);
}
