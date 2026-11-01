#include "ui/MainWindow.hpp"

#include "util/Log.hpp"
#include "util/Paths.hpp"

#include <iterator>

namespace {

RECT MakeRect(int x, int y, int width, int height) {
    RECT rect{x, y, x + width, y + height};
    return rect;
}

}  // namespace

LRESULT MainWindow::OnCreate(LPCREATESTRUCT /*createStruct*/) {
    m_taskbarCreatedMsg = ::RegisterWindowMessageW(L"TaskbarCreated");
    m_showYourselfMsg = ::RegisterWindowMessageW(L"MeetingBuddy.ShowYourself");

    SetWindowText(L"MeetingBuddy");
    CreateStatusBar();
    CreateTabs();
    ApplyDpiFonts(::GetDpiForWindow(m_hWnd));
    SetStatusIdle();

    if (!m_tray.Add(m_hWnd)) {
        Log::Warn(L"Tray icon was not added");
    }

    m_created = true;
    LayoutChildren();
    return 0;
}

void MainWindow::CreateStatusBar() {
    m_status.Create(m_hWnd);
    m_hWndStatusBar = m_status.m_hWnd;

    int panes[] = {
        IDC_STATUS_PANE_STATE,
        IDC_STATUS_PANE_TIME,
        IDC_STATUS_PANE_TRANSCRIBE};
    m_status.SetPanes(panes, static_cast<int>(std::size(panes)), true);
}

void MainWindow::CreateTabs() {
    const DWORD style = WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS | WS_CLIPCHILDREN | TCS_TABS;
    m_tabs.Create(m_hWnd, rcDefault, nullptr, style, 0, IDC_TAB_MAIN);

    TCITEMW item{};
    item.mask = TCIF_TEXT;
    wchar_t record[] = L"Record";
    item.pszText = record;
    m_tabs.InsertItem(0, &item);

    wchar_t recordings[] = L"Recordings";
    item.pszText = recordings;
    m_tabs.InsertItem(1, &item);

    m_recordView.Create(m_hWnd);
    m_recordingsView.Create(m_hWnd);
    m_tabs.SetCurSel(0);
    ShowSelectedTab();
}

void MainWindow::ApplyDpiFonts(UINT dpi) {
    if (dpi == 0) {
        dpi = USER_DEFAULT_SCREEN_DPI;
    }

    NONCLIENTMETRICSW metrics{};
    metrics.cbSize = sizeof(metrics);
    if (!::SystemParametersInfoForDpi(SPI_GETNONCLIENTMETRICS, sizeof(metrics), &metrics, 0, dpi) &&
        !::SystemParametersInfoW(SPI_GETNONCLIENTMETRICS, sizeof(metrics), &metrics, 0)) {
        return;
    }

    WTL::CFont newFont;
    if (newFont.CreateFontIndirect(&metrics.lfMessageFont) == nullptr) {
        return;
    }

    const HFONT previous = m_uiFont.Detach();
    m_uiFont.Attach(newFont.Detach());
    if (previous != nullptr) {
        ::DeleteObject(previous);
    }

    const HFONT font = m_uiFont;
    if (m_tabs.IsWindow()) {
        m_tabs.SetFont(font);
    }
    if (m_status.IsWindow()) {
        m_status.SetFont(font);
    }
}

void MainWindow::SetStatusIdle() {
    m_status.SetPaneText(IDC_STATUS_PANE_STATE, L"Idle");
    m_status.SetPaneText(IDC_STATUS_PANE_TIME, L"00:00:00");
    m_status.SetPaneText(IDC_STATUS_PANE_TRANSCRIBE, L"");
}

void MainWindow::LayoutChildren() {
    if (!m_created) {
        return;
    }

    UpdateLayout(FALSE);

    CRect client;
    GetClientRect(&client);

    if (m_hWndStatusBar != nullptr && ::IsWindow(m_hWndStatusBar) && ::IsWindowVisible(m_hWndStatusBar)) {
        CRect statusRect;
        ::GetWindowRect(m_hWndStatusBar, &statusRect);
        client.bottom -= statusRect.Height();
        m_status.UpdatePanesLayout();
    }

    if (m_tabs.IsWindow()) {
        m_tabs.SetWindowPos(nullptr, client.left, client.top, client.Width(), client.Height(), SWP_NOZORDER | SWP_NOACTIVATE);

        CRect tabClient = client;
        m_tabs.AdjustRect(FALSE, &tabClient);

        const CRect pageRect(tabClient.left, tabClient.top, tabClient.right, tabClient.bottom);
        if (m_recordView.IsWindow()) {
            m_recordView.SetWindowPos(HWND_TOP, pageRect.left, pageRect.top, pageRect.Width(), pageRect.Height(), SWP_NOACTIVATE);
        }
        if (m_recordingsView.IsWindow()) {
            m_recordingsView.SetWindowPos(HWND_TOP, pageRect.left, pageRect.top, pageRect.Width(), pageRect.Height(), SWP_NOACTIVATE);
        }
    }
}

void MainWindow::ShowSelectedTab() {
    const int sel = m_tabs.GetCurSel();
    if (m_recordView.IsWindow()) {
        m_recordView.ShowWindow(sel == 0 ? SW_SHOW : SW_HIDE);
    }
    if (m_recordingsView.IsWindow()) {
        m_recordingsView.ShowWindow(sel == 1 ? SW_SHOW : SW_HIDE);
    }
}

void MainWindow::OnSize(UINT type, CSize /*size*/) {
    if (type != SIZE_MINIMIZED) {
        LayoutChildren();
    }
}

void MainWindow::OnGetMinMaxInfo(LPMINMAXINFO minMaxInfo) {
    if (minMaxInfo == nullptr) {
        return;
    }

    CRect windowRect;
    CRect clientRect;
    GetWindowRect(&windowRect);
    GetClientRect(&clientRect);
    const int nonClientW = windowRect.Width() - clientRect.Width();
    const int nonClientH = windowRect.Height() - clientRect.Height();
    minMaxInfo->ptMinTrackSize.x = kMinClientWidth + nonClientW;
    minMaxInfo->ptMinTrackSize.y = kMinClientHeight + nonClientH;
}

void MainWindow::OnSysCommand(UINT id, CPoint point) {
    if ((id & 0xFFF0) == SC_MINIMIZE) {
        HideToTray(false);
        return;
    }
    SetMsgHandled(FALSE);
    (void)point;
}

void MainWindow::OnDpiChanged(UINT dpiX, UINT /*dpiY*/, PRECT suggested) {
    ApplyDpiFonts(dpiX);
    if (suggested != nullptr) {
        SetWindowPos(
            nullptr,
            suggested->left,
            suggested->top,
            suggested->right - suggested->left,
            suggested->bottom - suggested->top,
            SWP_NOZORDER | SWP_NOACTIVATE);
    }
    LayoutChildren();
}

void MainWindow::OnExitSizeMove() {
    PersistWindowPlacement();
}

void MainWindow::OnClose() {
    if (m_exiting) {
        PersistWindowPlacement();
        DestroyWindow();
        return;
    }

    if (Settings::Instance().Data().closeToTray) {
        HideToTray(true);
        return;
    }

    PersistWindowPlacement();
    DestroyWindow();
}

void MainWindow::OnDestroy() {
    m_tray.Remove();
    PostQuitMessage(0);
}

void MainWindow::RequestExit() {
    m_exiting = true;
    PersistWindowPlacement();
    Settings::Instance().Save();
    DestroyWindow();
}

void MainWindow::ShowFromTray() {
    ShowWindow(SW_RESTORE);
    ShowWindow(SW_SHOW);
    SetForegroundWindow(m_hWnd);
    ::BringWindowToTop(m_hWnd);
}

void MainWindow::HideToTray(bool /*fromClose*/) {
    PersistWindowPlacement();
    ShowWindow(SW_HIDE);

    auto& settings = Settings::Instance().Data();
    if (!settings.firstRunTrayHintShown) {
        m_tray.ShowBalloon(L"MeetingBuddy", L"MeetingBuddy is still running in the tray.");
        settings.firstRunTrayHintShown = true;
        Settings::Instance().Save();
    }
}

void MainWindow::PersistWindowPlacement() {
    if (!IsWindow() || !IsWindowVisible() || IsIconic() || IsZoomed()) {
        return;
    }

    CRect rect;
    GetWindowRect(&rect);
    if (!IsPlacementOnAnyMonitor(rect)) {
        return;
    }

    WindowPlacementSettings placement{};
    placement.x = rect.left;
    placement.y = rect.top;
    placement.width = rect.Width();
    placement.height = rect.Height();
    Settings::Instance().Data().windowPlacement = placement;
    Settings::Instance().Save();
}

void MainWindow::RestoreWindowPlacement() {
    const auto& placement = Settings::Instance().Data().windowPlacement;
    if (!placement.has_value()) {
        ResizeClient(kDefaultClientWidth, kDefaultClientHeight);
        CenterWindow();
        return;
    }

    RECT rect = MakeRect(placement->x, placement->y, placement->width, placement->height);
    if (!IsPlacementOnAnyMonitor(rect) || placement->width < kMinClientWidth || placement->height < kMinClientHeight) {
        ResizeClient(kDefaultClientWidth, kDefaultClientHeight);
        CenterWindow();
        return;
    }

    SetWindowPos(
        nullptr,
        placement->x,
        placement->y,
        placement->width,
        placement->height,
        SWP_NOZORDER | SWP_NOACTIVATE);
}

bool MainWindow::IsPlacementOnAnyMonitor(const RECT& windowRect) const {
    const HMONITOR monitor = ::MonitorFromRect(&windowRect, MONITOR_DEFAULTTONULL);
    return monitor != nullptr;
}

LRESULT MainWindow::OnRegisteredMessage(UINT msg, WPARAM /*wParam*/, LPARAM /*lParam*/, BOOL& handled) {
    if (msg == m_taskbarCreatedMsg) {
        m_tray.ReAdd();
        handled = TRUE;
        return 0;
    }
    if (msg == m_showYourselfMsg) {
        ShowFromTray();
        handled = TRUE;
        return 0;
    }
    handled = FALSE;
    return 0;
}

LRESULT MainWindow::OnTrayIcon(UINT /*msg*/, WPARAM /*wParam*/, LPARAM lParam, BOOL& handled) {
    handled = TRUE;
    switch (LOWORD(lParam)) {
    case NIN_SELECT:
    case NIN_KEYSELECT:
    case WM_LBUTTONUP:
        if (IsWindowVisible()) {
            HideToTray(false);
        } else {
            ShowFromTray();
        }
        break;
    case WM_CONTEXTMENU:
    case WM_RBUTTONUP:
        ShowTrayContextMenu();
        break;
    default:
        break;
    }
    return 0;
}

void MainWindow::ShowTrayContextMenu() {
    CMenu menu;
    menu.LoadMenu(IDR_TRAY_MENU);
    CMenuHandle popup = menu.GetSubMenu(0);
    if (popup.m_hMenu == nullptr) {
        return;
    }

    popup.EnableMenuItem(ID_TRAY_STARTSTOP, MF_BYCOMMAND | MF_GRAYED);

    POINT cursor{};
    ::GetCursorPos(&cursor);
    SetForegroundWindow(m_hWnd);
    popup.TrackPopupMenu(TPM_RIGHTBUTTON | TPM_LEFTALIGN, cursor.x, cursor.y, m_hWnd);
    PostMessage(WM_NULL);
}

void MainWindow::OnFileExit(UINT /*code*/, int /*id*/, HWND /*control*/) {
    RequestExit();
}

void MainWindow::OnOpenRecordingsFolder(UINT /*code*/, int /*id*/, HWND /*control*/) {
    const std::wstring folder = Settings::Instance().RecordingsFolder();
    Paths::EnsureDirectory(folder);
    ::ShellExecuteW(m_hWnd, L"explore", folder.c_str(), nullptr, nullptr, SW_SHOWNORMAL);
}

void MainWindow::OnSettings(UINT /*code*/, int /*id*/, HWND /*control*/) {
    ::MessageBoxW(
        m_hWnd,
        L"Settings will be available in a later milestone.",
        L"MeetingBuddy",
        MB_OK | MB_ICONINFORMATION);
}

void MainWindow::OnAbout(UINT /*code*/, int /*id*/, HWND /*control*/) {
    ::MessageBoxW(
        m_hWnd,
        L"MeetingBuddy\nLocal meeting recorder and optional transcription.\n\n"
        L"Transcription is local. The only network use is a model download you start.",
        L"About MeetingBuddy",
        MB_OK | MB_ICONINFORMATION);
}

void MainWindow::OnTrayShow(UINT /*code*/, int /*id*/, HWND /*control*/) {
    ShowFromTray();
}

void MainWindow::OnTrayStartStop(UINT /*code*/, int /*id*/, HWND /*control*/) {
    // Recording arrives in M2.
}

LRESULT MainWindow::OnTabSelChange(LPNMHDR /*header*/) {
    ShowSelectedTab();
    return 0;
}
