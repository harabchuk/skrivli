#include "ui/MainWindow.hpp"

#include "audio/WavWriter.hpp"
#include "storage/RecordingLibrary.hpp"
#include "storage/RecordingSidecar.hpp"
#include "ui/TranscriptWindow.hpp"
#include "util/Log.hpp"
#include "util/Paths.hpp"

#include <iterator>
#include <cstdio>
#include <memory>

namespace {

RECT MakeRect(int x, int y, int width, int height) {
    RECT rect{x, y, x + width, y + height};
    return rect;
}

std::wstring FormatElapsed(uint64_t elapsedQpc, uint64_t frequency) {
    if (frequency == 0) {
        frequency = 1;
    }
    const uint64_t totalSeconds = elapsedQpc / frequency;
    const uint64_t hours = totalSeconds / 3600;
    const uint64_t minutes = (totalSeconds / 60) % 60;
    const uint64_t seconds = totalSeconds % 60;
    wchar_t buffer[32]{};
    std::swprintf(buffer, 32, L"%02llu:%02llu:%02llu", hours, minutes, seconds);
    return buffer;
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

    m_session.SetNotifyHwnd(m_hWnd);
    m_session.SetDeviceManager(&m_devices);
    m_recordView.SetSession(&m_session);
    m_recordView.SetDeviceManager(&m_devices);
    m_recordView.SetTranscribeHandler(
        [](void* context, const std::wstring& audioPath) {
            auto* self = static_cast<MainWindow*>(context);
            if (self != nullptr) {
                self->EnqueueTranscription(audioPath);
            }
        },
        this);
    TranscriptionService::Instance().SetNotifyHwnd(m_hWnd);
    TranscriptionService::Instance().Start();

    if (!m_devices.Initialize(m_hWnd)) {
        Log::Warn(L"DeviceManager failed to initialize");
    }
    m_recordView.RefreshDevices();

    RecoverPartialRecordings();

    if (!m_tray.Add(m_hWnd)) {
        Log::Warn(L"Tray icon was not added");
    }

    m_created = true;
    LayoutChildren();
    return 0;
}

void MainWindow::RecoverPartialRecordings() {
    const std::wstring folder = Settings::Instance().RecordingsFolder();
    auto recovered = WavWriter::RecoverPartialsInFolder(folder);
    auto rescued = WavWriter::RecoverPartialsInFolder(Paths::RescueDir());
    recovered.insert(recovered.end(), rescued.begin(), rescued.end());
    for (const auto& path : recovered) {
        RecordingSidecar side{};
        side.startLocal = Sidecar::FormatLocalTimestampNow();
        side.endLocal = side.startLocal;
        side.durationSeconds = RecordingLibrary::ReadWavDurationSeconds(path);
        side.sampleRate = 48000;
        side.channels = 2;
        side.deviceNotes.push_back("Recovered from partial file");
        side.transcriptStatus = TranscriptStatus::None;
        RecordingLibrary::WriteSidecarForAudio(path, side);
    }
    if (!recovered.empty()) {
        Log::Info(L"Recovered " + std::to_wstring(recovered.size()) + L" partial recording(s)");
        m_tray.ShowBalloon(
            L"MeetingBuddy",
            L"Recovered an interrupted recording. Check your recordings or rescue folder.");
    }
}

void MainWindow::WriteSidecarForPath(const std::wstring& audioPath) {
    if (audioPath.empty()) {
        return;
    }

    RecordingSidecar side{};
    side.startLocal = Sidecar::FormatLocalTimestamp(m_session.StartLocalTime());
    side.endLocal = Sidecar::FormatLocalTimestampNow();
    side.durationSeconds = RecordingLibrary::ReadWavDurationSeconds(audioPath);
    if (side.durationSeconds <= 0.0) {
        side.durationSeconds = m_session.ElapsedSeconds();
    }
    side.sampleRate = 48000;
    side.channels = 2;
    side.micDevice = Sidecar::WideToUtf8(m_session.MicDeviceName());
    side.systemDevice = Sidecar::WideToUtf8(m_session.SystemDeviceName());
    for (const auto& note : m_session.TakeDeviceNotes()) {
        side.deviceNotes.push_back(Sidecar::WideToUtf8(note));
    }
    side.transcriptStatus = TranscriptStatus::None;
    if (!RecordingLibrary::WriteSidecarForAudio(audioPath, side)) {
        Log::Warn(L"Failed to write sidecar for " + audioPath);
    }
}

void MainWindow::RefreshRecordingsList() {
    if (m_recordingsView.IsWindow()) {
        m_recordingsView.Refresh();
    }
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

void MainWindow::SetStatusRecording() {
    m_status.SetPaneText(IDC_STATUS_PANE_STATE, L"Recording");
}

void MainWindow::UpdateTrayRecordingTip() {
    const uint64_t start = m_session.StartQpc();
    if (start == 0) {
        return;
    }
    LARGE_INTEGER now{};
    LARGE_INTEGER freq{};
    ::QueryPerformanceCounter(&now);
    ::QueryPerformanceFrequency(&freq);
    const std::wstring tip =
        L"Recording " + FormatElapsed(static_cast<uint64_t>(now.QuadPart) - start, static_cast<uint64_t>(freq.QuadPart));
    m_tray.SetTip(tip.c_str());

    const std::wstring elapsed =
        FormatElapsed(static_cast<uint64_t>(now.QuadPart) - start, static_cast<uint64_t>(freq.QuadPart));
    m_status.SetPaneText(IDC_STATUS_PANE_TIME, elapsed.c_str());
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

void MainWindow::OnTimer(UINT_PTR id) {
    if (id == IDT_RECORD_ELAPSED && m_session.IsActive()) {
        UpdateTrayRecordingTip();
    }
}

BOOL MainWindow::OnQueryEndSession(UINT /*source*/, UINT /*flags*/) {
    // Always allow shutdown; OnEndSession will stop and save.
    return TRUE;
}

void MainWindow::OnEndSession(BOOL ending, UINT /*flags*/) {
    if (!ending) {
        return;
    }
    m_endSessionSaving = true;
    if (m_session.IsActive()) {
        m_session.Stop(RecordingSession::StopMode::Save);
    }
    PersistWindowPlacement();
    Settings::Instance().Save();
}

bool MainWindow::ConfirmStopRecordingForExit() {
    if (!m_session.IsActive()) {
        return true;
    }

    const int answer = ::MessageBoxW(
        m_hWnd,
        L"Stop and save recording?",
        L"MeetingBuddy",
        MB_YESNOCANCEL | MB_ICONQUESTION | MB_DEFBUTTON1);

    if (answer == IDCANCEL) {
        return false;
    }
    if (answer == IDYES) {
        m_session.Stop(RecordingSession::StopMode::Save);
        return true;
    }
    // IDNO — exit without saving
    m_session.Stop(RecordingSession::StopMode::Discard);
    return true;
}

void MainWindow::FinishExit() {
    m_exiting = true;
    PersistWindowPlacement();
    Settings::Instance().Save();
    DestroyWindow();
}

void MainWindow::OnClose() {
    if (m_exiting || m_endSessionSaving) {
        PersistWindowPlacement();
        DestroyWindow();
        return;
    }

    if (Settings::Instance().Data().closeToTray) {
        HideToTray(true);
        return;
    }

    if (!ConfirmStopRecordingForExit()) {
        return;
    }
    FinishExit();
}

void MainWindow::OnDestroy() {
    KillTimer(IDT_RECORD_ELAPSED);
    if (m_session.IsActive()) {
        m_session.Stop(RecordingSession::StopMode::Save);
    }
    TranscriptionService::Instance().Stop();
    TranscriptWindow::CloseAll();
    m_devices.Shutdown();
    m_tray.Remove();
    PostQuitMessage(0);
}

void MainWindow::SetTranscribeStatusText(const std::wstring& text) {
    m_status.SetPaneText(IDC_STATUS_PANE_TRANSCRIBE, text.c_str());
}

bool MainWindow::EnqueueTranscription(const std::wstring& audioPath) {
    if (audioPath.empty()) {
        return false;
    }
    return TranscriptionService::Instance().Enqueue(m_hWnd, audioPath);
}

void MainWindow::RequestExit() {
    if (!ConfirmStopRecordingForExit()) {
        return;
    }
    FinishExit();
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
    case NIN_BALLOONUSERCLICK:
        if (!m_balloonTranscriptPath.empty()) {
            TranscriptWindow::OpenOrActivate(m_balloonTranscriptPath);
            m_balloonTranscriptPath.clear();
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

LRESULT MainWindow::OnRecordingStarted(UINT /*msg*/, WPARAM /*wParam*/, LPARAM /*lParam*/, BOOL& handled) {
    handled = TRUE;
    m_recordView.OnRecordingStarted();
    SetStatusRecording();
    m_tray.SetRecording(true);
    UpdateTrayRecordingTip();
    SetTimer(IDT_RECORD_ELAPSED, 250);
    return 0;
}

LRESULT MainWindow::OnRecordingFinished(UINT /*msg*/, WPARAM /*wParam*/, LPARAM lParam, BOOL& handled) {
    handled = TRUE;
    KillTimer(IDT_RECORD_ELAPSED);
    std::unique_ptr<std::wstring> path(reinterpret_cast<std::wstring*>(lParam));
    if (path && !path->empty()) {
        WriteSidecarForPath(*path);
        Log::Info(L"Recording saved: " + *path);
        if (Settings::Instance().Data().autoTranscribe) {
            EnqueueTranscription(*path);
        }
    }
    m_recordView.OnRecordingFinished(path ? *path : std::wstring{});
    SetStatusIdle();
    m_tray.SetRecording(false);
    RefreshRecordingsList();
    return 0;
}

LRESULT MainWindow::OnRecordingError(UINT /*msg*/, WPARAM /*wParam*/, LPARAM lParam, BOOL& handled) {
    handled = TRUE;
    std::unique_ptr<std::wstring> message(reinterpret_cast<std::wstring*>(lParam));
    if (m_session.IsActive()) {
        m_session.Stop(RecordingSession::StopMode::Save);
    }
    KillTimer(IDT_RECORD_ELAPSED);
    m_recordView.OnRecordingError(message ? *message : std::wstring{});
    SetStatusIdle();
    m_tray.SetRecording(false);
    return 0;
}

LRESULT MainWindow::OnDevicesChanged(UINT /*msg*/, WPARAM /*wParam*/, LPARAM /*lParam*/, BOOL& handled) {
    handled = TRUE;
    m_recordView.OnDevicesChanged();
    return 0;
}

LRESULT MainWindow::OnDeviceSwitched(UINT /*msg*/, WPARAM /*wParam*/, LPARAM lParam, BOOL& handled) {
    handled = TRUE;
    std::unique_ptr<std::wstring> message(reinterpret_cast<std::wstring*>(lParam));
    if (message && !message->empty()) {
        m_tray.ShowBalloon(L"MeetingBuddy", message->c_str());
        m_recordView.SetWarning(*message);
        Log::Warn(*message);
    }
    return 0;
}

LRESULT MainWindow::OnLowDisk(UINT /*msg*/, WPARAM /*wParam*/, LPARAM /*lParam*/, BOOL& handled) {
    handled = TRUE;
    // Stop finalizes and posts WM_APP_RECORDING_FINISHED (which writes the sidecar).
    if (m_session.IsActive()) {
        m_session.Stop(RecordingSession::StopMode::Save);
    }
    KillTimer(IDT_RECORD_ELAPSED);
    m_recordView.OnRecordingFinished({});
    SetStatusIdle();
    m_tray.SetRecording(false);
    m_tray.ShowBalloon(L"MeetingBuddy", L"Recording stopped: low disk space");
    RefreshRecordingsList();
    return 0;
}

LRESULT MainWindow::OnRecordingRescued(UINT /*msg*/, WPARAM /*wParam*/, LPARAM lParam, BOOL& handled) {
    handled = TRUE;
    std::unique_ptr<std::wstring> path(reinterpret_cast<std::wstring*>(lParam));
    if (m_session.IsActive()) {
        m_session.Stop(RecordingSession::StopMode::Save);
    }
    KillTimer(IDT_RECORD_ELAPSED);
    if (path && !path->empty()) {
        WriteSidecarForPath(*path);
        const std::wstring text = L"Recording folder unavailable. Saved to:\n" + *path;
        m_tray.ShowBalloon(L"MeetingBuddy", text.c_str());
        ::MessageBoxW(m_hWnd, text.c_str(), L"MeetingBuddy", MB_OK | MB_ICONWARNING);
        Log::Warn(text);
    }
    m_recordView.OnRecordingFinished(path ? *path : std::wstring{});
    SetStatusIdle();
    m_tray.SetRecording(false);
    RefreshRecordingsList();
    return 0;
}

void MainWindow::ShowTrayContextMenu() {
    CMenu menu;
    menu.LoadMenu(IDR_TRAY_MENU);
    CMenuHandle popup = menu.GetSubMenu(0);
    if (popup.m_hMenu == nullptr) {
        return;
    }

    const bool recording = m_session.IsActive();
    popup.EnableMenuItem(ID_TRAY_STARTSTOP, MF_BYCOMMAND | MF_ENABLED);
    popup.ModifyMenu(
        ID_TRAY_STARTSTOP,
        MF_BYCOMMAND | MF_STRING,
        ID_TRAY_STARTSTOP,
        recording ? L"&Stop recording" : L"&Start recording");

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
    m_recordView.ToggleRecording();
}

LRESULT MainWindow::OnTabSelChange(LPNMHDR /*header*/) {
    ShowSelectedTab();
    return 0;
}

LRESULT MainWindow::OnTranscribeQueued(UINT /*msg*/, WPARAM /*wParam*/, LPARAM lParam, BOOL& handled) {
    handled = TRUE;
    std::unique_ptr<TranscribeNotify> notify(reinterpret_cast<TranscribeNotify*>(lParam));
    SetTranscribeStatusText(L"Queued");
    RefreshRecordingsList();
    (void)notify;
    return 0;
}

LRESULT MainWindow::OnTranscribeProgress(UINT /*msg*/, WPARAM /*wParam*/, LPARAM lParam, BOOL& handled) {
    handled = TRUE;
    std::unique_ptr<TranscribeNotify> notify(reinterpret_cast<TranscribeNotify*>(lParam));
    if (notify) {
        wchar_t buf[64]{};
        std::swprintf(buf, 64, L"Transcribing %d%%", notify->percent);
        SetTranscribeStatusText(buf);
        if (m_recordingsView.IsWindow()) {
            m_recordingsView.SetTranscriptProgress(notify->audioPath, notify->percent);
        }
    }
    return 0;
}

LRESULT MainWindow::OnTranscribeFinished(UINT /*msg*/, WPARAM /*wParam*/, LPARAM lParam, BOOL& handled) {
    handled = TRUE;
    std::unique_ptr<TranscribeNotify> notify(reinterpret_cast<TranscribeNotify*>(lParam));
    SetTranscribeStatusText(L"Ready");
    RefreshRecordingsList();
    if (notify && !notify->audioPath.empty()) {
        const std::wstring mdPath = RecordingLibrary::MdPathForAudio(notify->audioPath);
        m_balloonTranscriptPath = mdPath;
        m_tray.ShowBalloon(L"MeetingBuddy", L"Transcript ready — click to open");
        if (IsWindowVisible() && !IsIconic()) {
            TranscriptWindow::OpenOrActivate(mdPath);
        }
    }
    return 0;
}

LRESULT MainWindow::OnTranscribeFailed(UINT /*msg*/, WPARAM /*wParam*/, LPARAM lParam, BOOL& handled) {
    handled = TRUE;
    std::unique_ptr<TranscribeNotify> notify(reinterpret_cast<TranscribeNotify*>(lParam));
    if (notify && !notify->message.empty()) {
        SetTranscribeStatusText(notify->message);
    } else {
        SetTranscribeStatusText(L"Failed");
    }
    RefreshRecordingsList();
    return 0;
}
