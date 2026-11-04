#include "views/MainWindow.hpp"

#include "Version.hpp"
#include "infrastructure/common/Log.hpp"
#include "infrastructure/common/Paths.hpp"
#include "infrastructure/common/PostOwned.hpp"
#include "infrastructure/common/Scoped.hpp"
#include "infrastructure/common/Text.hpp"
#include "infrastructure/files/M4aEncoder.hpp"
#include "infrastructure/files/WavWriter.hpp"
#include "infrastructure/storage/RecordingLibrary.hpp"
#include "infrastructure/storage/RecordingSidecar.hpp"
#include "infrastructure/system/Autostart.hpp"
#include "infrastructure/system/Hotkey.hpp"
#include "ui/Theme.hpp"
#include "views/ModelPrompt.hpp"
#include "views/SettingsDialog.hpp"

#include <cstdio>
#include <iterator>
#include <memory>
#include <thread>

namespace {

RECT MakeRect(int x, int y, int width, int height) {
    RECT rect{x, y, x + width, y + height};
    return rect;
}

auto FormatElapsed(uint64_t elapsedQpc, uint64_t frequency) -> std::wstring {
    if (frequency == 0) {
        frequency = 1;
    }
    return Text::FormatHms(elapsedQpc / frequency);
}

} // namespace

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

    RegisterConfiguredHotkey();

    m_created = true;
    LayoutChildren();
    return 0;
}

void MainWindow::RecoverPartialRecordings() {
    const auto folder = Settings::Instance().RecordingsFolder();
    auto recovered = WavWriter::RecoverPartialsInFolder(folder);
    auto rescued = WavWriter::RecoverPartialsInFolder(Paths::RescueDir());
    recovered.insert(recovered.end(), rescued.begin(), rescued.end());
    for (const auto& path : recovered) {
        auto side = Sidecar::ForAudio(path);
        side.deviceNotes.push_back("Recovered from partial file");
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

    auto side = Sidecar::ForAudio(audioPath);
    side.startLocal = Sidecar::FormatLocalTimestamp(m_session.StartLocalTime());
    if (side.durationSeconds <= 0.0) {
        side.durationSeconds = m_session.ElapsedSeconds();
    }
    side.micDevice = Text::WideToUtf8(m_session.MicDeviceName());
    side.systemDevice = Text::WideToUtf8(m_session.SystemDeviceName());
    for (const auto& note : m_session.TakeDeviceNotes()) {
        side.deviceNotes.push_back(Text::WideToUtf8(note));
    }
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

    // Left→right: state, elapsed, then stretching transcription progress.
    int panes[] = {IDC_STATUS_PANE_STATE, IDC_STATUS_PANE_TIME, IDC_STATUS_PANE_TRANSCRIBE};
    m_status.SetPanes(panes, static_cast<int>(std::size(panes)), false);
    ApplyStatusPaneWidths();
}

void MainWindow::ApplyStatusPaneWidths() {
    if (!m_status.IsWindow()) {
        return;
    }

    const auto dpi = ::GetDpiForWindow(m_hWnd);
    auto measure = [&](const wchar_t* text) -> int {
        CClientDC dc(m_status.m_hWnd);
        const auto old = dc.SelectFont(m_status.GetFont());
        SIZE size{};
        dc.GetTextExtent(text, static_cast<int>(wcslen(text)), &size);
        dc.SelectFont(old);
        // Borders + padding so labels are not tight against pane edges.
        return size.cx + ui::Scale(24, dpi);
    };

    m_status.SetPaneWidth(IDC_STATUS_PANE_STATE, measure(L"Recording"));
    m_status.SetPaneWidth(IDC_STATUS_PANE_TIME, measure(L"00:00:00"));
}

void MainWindow::CreateTabs() {
    const DWORD style = WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS | WS_CLIPCHILDREN | TCS_TABS;
    m_tabs.Create(m_hWnd, rcDefault, nullptr, style, 0, IDC_TAB_MAIN);
    const auto dpi = ::GetDpiForWindow(m_hWnd);
    m_tabs.SetPadding(CSize(ui::Scale(14, dpi), ui::Scale(6, dpi)));

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

    m_uiFont.Attach(newFont.Detach());

    const HFONT font = m_uiFont;
    if (m_tabs.IsWindow()) {
        const auto tabDpi = ::GetDpiForWindow(m_tabs.m_hWnd);
        m_tabs.SetPadding(CSize(ui::Scale(14, tabDpi), ui::Scale(6, tabDpi)));
        m_tabs.SetFont(font);
    }
    if (m_status.IsWindow()) {
        m_status.SetFont(font);
        ApplyStatusPaneWidths();
    }
    if (m_recordView.IsWindow()) {
        m_recordView.ApplyUiFont(font);
    }
    if (m_recordingsView.IsWindow()) {
        m_recordingsView.ApplyUiFont(font);
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

void MainWindow::UpdateRecordingUi() {
    const auto start = m_session.StartQpc();
    if (start == 0) {
        return;
    }
    LARGE_INTEGER now{};
    LARGE_INTEGER freq{};
    ::QueryPerformanceCounter(&now);
    ::QueryPerformanceFrequency(&freq);
    const auto elapsed = FormatElapsed(static_cast<uint64_t>(now.QuadPart) - start,
                                       static_cast<uint64_t>(freq.QuadPart));
    const auto tip = L"Recording " + elapsed;
    m_tray.SetTip(tip.c_str());
    m_status.SetPaneText(IDC_STATUS_PANE_TIME, elapsed.c_str());
}

void MainWindow::EnterIdleUi(const std::wstring& path, bool refreshList,
                             const std::wstring* error) {
    KillTimer(IDT_RECORD_ELAPSED);
    if (error != nullptr) {
        m_recordView.OnRecordingError(*error);
    } else {
        m_recordView.OnRecordingFinished(path);
    }
    SetStatusIdle();
    m_tray.SetRecording(false);
    if (refreshList) {
        RefreshRecordingsList();
    }
}

void MainWindow::LayoutChildren() {
    if (!m_created) {
        return;
    }

    UpdateLayout();

    CRect client;
    GetClientRect(&client);

    if (m_hWndStatusBar != nullptr && ::IsWindow(m_hWndStatusBar) &&
        ::IsWindowVisible(m_hWndStatusBar)) {
        CRect statusRect;
        ::GetWindowRect(m_hWndStatusBar, &statusRect);
        client.bottom -= statusRect.Height();
    }

    if (m_tabs.IsWindow()) {
        // The stock control left-aligns its items, so size it to the strip and center that window.
        m_tabs.SetWindowPos(nullptr, client.left, client.top, client.Width(), client.Height(),
                            SWP_NOZORDER | SWP_NOACTIVATE);

        CRect span{};
        const int count = m_tabs.GetItemCount();
        for (int i = 0; i < count; ++i) {
            CRect item{};
            if (!m_tabs.GetItemRect(i, &item)) {
                continue;
            }
            if (span.IsRectEmpty()) {
                span = item;
            } else {
                span.UnionRect(&span, &item);
            }
        }

        int tabX = client.left;
        int tabWidth = client.Width();
        int tabHeight = client.Height();
        if (!span.IsRectEmpty()) {
            const int edge = span.left > 2 ? static_cast<int>(span.left) : 2;
            tabWidth = span.Width() + edge * 2;
            // One pixel under the buttons is the baseline the selected tab covers.
            tabHeight = static_cast<int>(span.bottom) + 1;
            if (tabWidth < client.Width()) {
                tabX = client.left + (client.Width() - tabWidth) / 2;
            } else {
                tabX = client.left;
                tabWidth = client.Width();
            }
            if (tabHeight > client.Height()) {
                tabHeight = client.Height();
            }
        }

        m_tabs.SetWindowPos(nullptr, tabX, client.top, tabWidth, tabHeight,
                            SWP_NOZORDER | SWP_NOACTIVATE);

        const CRect pageRect(client.left, client.top + tabHeight, client.right, client.bottom);
        CRect strip(client.left, client.top, client.right, pageRect.top);
        InvalidateRect(&strip, TRUE);
        if (m_recordView.IsWindow()) {
            m_recordView.SetWindowPos(HWND_TOP, pageRect.left, pageRect.top, pageRect.Width(),
                                      pageRect.Height(), SWP_NOACTIVATE);
        }
        if (m_recordingsView.IsWindow()) {
            m_recordingsView.SetWindowPos(HWND_TOP, pageRect.left, pageRect.top, pageRect.Width(),
                                          pageRect.Height(), SWP_NOACTIVATE);
        }
    }
}

void MainWindow::OnPaint(CDCHandle /*dc*/) {
    CPaintDC paint(m_hWnd);
    DrawTabBaseline(paint.m_hDC);
}

void MainWindow::DrawTabBaseline(CDCHandle dc) {
    if (!m_tabs.IsWindow()) {
        return;
    }

    CRect tabs;
    m_tabs.GetWindowRect(&tabs);
    ScreenToClient(&tabs);
    if (tabs.IsRectEmpty()) {
        return;
    }

    CRect client;
    GetClientRect(&client);
    const int y = tabs.bottom - 1;
    if (y < client.top || y >= client.bottom) {
        return;
    }
    if (tabs.left > client.left) {
        dc.FillSolidRect(client.left, y, tabs.left - client.left, 1, ui::kDivider);
    }
    if (tabs.right < client.right) {
        dc.FillSolidRect(tabs.right, y, client.right - tabs.right, 1, ui::kDivider);
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
        HideToTray();
        return;
    }
    SetMsgHandled(FALSE);
    (void)point;
}

void MainWindow::OnDpiChanged(UINT dpiX, UINT /*dpiY*/, PRECT suggested) {
    ApplyDpiFonts(dpiX);
    if (m_recordView.IsWindow()) {
        m_recordView.OnDpiChanged(dpiX);
    }
    if (suggested != nullptr) {
        SetWindowPos(nullptr, suggested->left, suggested->top, suggested->right - suggested->left,
                     suggested->bottom - suggested->top, SWP_NOZORDER | SWP_NOACTIVATE);
    }
    LayoutChildren();
}

void MainWindow::OnExitSizeMove() {
    PersistWindowPlacement();
}

void MainWindow::OnTimer(UINT_PTR id) {
    if (id == IDT_RECORD_ELAPSED && m_session.IsActive()) {
        UpdateRecordingUi();
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

    const int answer = ui::ShowMessage(m_hWnd, L"Stop and save recording?",
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
        HideToTray();
        return;
    }

    if (!ConfirmStopRecordingForExit()) {
        return;
    }
    FinishExit();
}

void MainWindow::OnDestroy() {
    KillTimer(IDT_RECORD_ELAPSED);
    UnregisterConfiguredHotkey();
    if (m_session.IsActive()) {
        m_session.Stop(RecordingSession::StopMode::Save);
    }
    TranscriptionService::Instance().Stop();
    m_devices.Shutdown();
    m_tray.Remove();
    PostQuitMessage(0);
}

void MainWindow::ApplyStartupVisibility(bool startHidden) {
    if (startHidden) {
        ShowWindow(SW_HIDE);
    }
}

void MainWindow::RegisterConfiguredHotkey() {
    UnregisterConfiguredHotkey();
    const auto& settings = Settings::Instance().Data();
    if (!settings.hotkeyEnabled) {
        return;
    }
    const auto chord = Hotkey::Parse(settings.hotkey);
    if (!chord.valid) {
        if (!m_hotkeyWarned) {
            m_hotkeyWarned = true;
            ui::ShowMessage(m_hWnd,
                            L"The configured hotkey is invalid. Recording remains available from "
                            L"the window and tray.",
                            MB_OK | MB_ICONWARNING);
        }
        return;
    }
    if (!Hotkey::Register(m_hWnd, ID_HOTKEY_TOGGLE_RECORD, chord)) {
        if (!m_hotkeyWarned) {
            m_hotkeyWarned = true;
            ui::ShowMessage(m_hWnd,
                            L"Could not register the global hotkey (it may already be in use). "
                            L"Recording remains available from the window and tray.",
                            MB_OK | MB_ICONWARNING);
        }
        return;
    }
    m_hotkeyRegistered = true;
    m_hotkeyWarned = false;
}

void MainWindow::UnregisterConfiguredHotkey() {
    if (m_hotkeyRegistered) {
        Hotkey::Unregister(m_hWnd, ID_HOTKEY_TOGGLE_RECORD);
        m_hotkeyRegistered = false;
    }
}

void MainWindow::OnHotKey(int id, UINT /*modifiers*/, UINT /*vk*/) {
    if (id == ID_HOTKEY_TOGGLE_RECORD) {
        m_recordView.ToggleRecording();
    }
}

void MainWindow::StartM4aCompression(const std::wstring& wavPath) {
    if (wavPath.empty() || m_compressingM4a) {
        return;
    }
    m_compressingM4a = true;
    m_pendingCompressWav = wavPath;
    SetTranscribeStatusText(L"Compressing…");
    const HWND hwnd = m_hWnd;
    const bool keepWav = Settings::Instance().Data().keepWav;
    std::thread([hwnd, wavPath, keepWav]() {
        ScopedComInit com(COINIT_MULTITHREADED);

        std::wstring m4aPath;
        std::wstring error;
        const bool ok = M4aEncoder::EncodeWavToM4a(wavPath, m4aPath, error);
        if (ok) {
            if (!keepWav) {
                ::DeleteFileW(wavPath.c_str());
            }
            PostOwned(hwnd, WM_APP_M4A_FINISHED, std::make_unique<std::wstring>(m4aPath));
        } else {
            PostOwned(
                hwnd, WM_APP_M4A_FAILED,
                std::make_unique<std::wstring>(error.empty() ? L"M4A compression failed." : error));
        }
    }).detach();
}

void MainWindow::FinishRecordingUi(const std::wstring& audioPath) {
    if (Settings::Instance().Data().autoTranscribe && !audioPath.empty()) {
        EnqueueTranscription(audioPath);
    }
    EnterIdleUi(audioPath, true);
}

void MainWindow::SetTranscribeStatusText(const std::wstring& text) {
    m_status.SetPaneText(IDC_STATUS_PANE_TRANSCRIBE, text.c_str());
}

bool MainWindow::EnqueueTranscription(const std::wstring& audioPath) {
    if (audioPath.empty()) {
        return false;
    }
    const auto& settings = Settings::Instance().Data();
    const auto modelFile =
        settings.model.empty() ? std::wstring(kDefaultWhisperModel) : settings.model;
    const auto modelPath = ModelPrompt::EnsureModel(m_hWnd, modelFile);
    if (modelPath.empty()) {
        return false;
    }
    return TranscriptionService::Instance().Enqueue(audioPath, modelPath);
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

void MainWindow::HideToTray() {
    PersistWindowPlacement();
    ShowWindow(SW_HIDE);

    auto& settings = Settings::Instance().Data();
    if (!settings.firstRunTrayHintShown) {
        m_tray.ShowBalloon(L"MeetingBuddy", L"MeetingBuddy is still running in the tray.");
        settings.firstRunTrayHintShown = true;
        Settings::Instance().Save();
    }
}

void MainWindow::ToggleFromTrayClick() {
    // Collapse press+release (and any echoed legacy) notifications from one click.
    constexpr DWORD kMinToggleIntervalMs = 250;
    const auto now = ::GetTickCount();
    if (now - m_lastTrayToggleTick < kMinToggleIntervalMs) {
        return;
    }
    m_lastTrayToggleTick = now;

    if (IsWindowVisible()) {
        HideToTray();
    } else {
        ShowFromTray();
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
    if (!IsPlacementOnAnyMonitor(rect) || placement->width < kMinClientWidth ||
        placement->height < kMinClientHeight) {
        ResizeClient(kDefaultClientWidth, kDefaultClientHeight);
        CenterWindow();
        return;
    }

    SetWindowPos(nullptr, placement->x, placement->y, placement->width, placement->height,
                 SWP_NOZORDER | SWP_NOACTIVATE);
}

bool MainWindow::IsPlacementOnAnyMonitor(const RECT& windowRect) const {
    const auto monitor = ::MonitorFromRect(&windowRect, MONITOR_DEFAULTTONULL);
    return monitor != nullptr;
}

LRESULT MainWindow::OnRegisteredMessage(UINT msg, WPARAM /*wParam*/, LPARAM /*lParam*/,
                                        BOOL& handled) {
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
        // NOTIFYICON_VERSION_4 can deliver NIN_SELECT for both button-down and
        // button-up of a single click. Collapse those into one toggle.
        ToggleFromTrayClick();
        break;
    case WM_LBUTTONUP:
        // Legacy callback path only (NIM_SETVERSION failed). With version 4,
        // NIN_SELECT already handled the click — handling this too would
        // show then immediately hide.
        if (!m_tray.UsesVersion4()) {
            ToggleFromTrayClick();
        }
        break;
    case NIN_BALLOONUSERCLICK:
        ShowFromTray();
        if (!m_balloonAudioPath.empty()) {
            if (m_tabs.IsWindow()) {
                m_tabs.SetCurSel(1);
                ShowSelectedTab();
                LayoutChildren();
            }
            if (m_recordingsView.IsWindow()) {
                m_recordingsView.SelectByAudioPath(m_balloonAudioPath);
            }
            m_balloonAudioPath.clear();
        }
        break;
    case WM_CONTEXTMENU:
        ShowTrayContextMenu();
        break;
    case WM_RBUTTONUP:
        if (!m_tray.UsesVersion4()) {
            ShowTrayContextMenu();
        }
        break;
    default:
        break;
    }
    return 0;
}

LRESULT MainWindow::OnRecordingStarted(UINT /*msg*/, WPARAM /*wParam*/, LPARAM /*lParam*/,
                                       BOOL& handled) {
    handled = TRUE;
    m_recordView.OnRecordingStarted();
    SetStatusRecording();
    m_tray.SetRecording(true);
    UpdateRecordingUi();
    SetTimer(IDT_RECORD_ELAPSED, 250);
    return 0;
}

LRESULT MainWindow::OnRecordingFinished(UINT /*msg*/, WPARAM /*wParam*/, LPARAM lParam,
                                        BOOL& handled) {
    handled = TRUE;
    const auto path = TakeOwned<std::wstring>(lParam);
    if (path && !path->empty()) {
        WriteSidecarForPath(*path);
        Log::Info(L"Recording saved: " + *path);
        if (Settings::Instance().Data().compressToM4a) {
            EnterIdleUi(*path, false);
            StartM4aCompression(*path);
            return 0;
        }
    }
    FinishRecordingUi(path ? *path : std::wstring{});
    return 0;
}

LRESULT MainWindow::OnM4aFinished(UINT /*msg*/, WPARAM /*wParam*/, LPARAM lParam, BOOL& handled) {
    handled = TRUE;
    m_compressingM4a = false;
    const auto path = TakeOwned<std::wstring>(lParam);
    m_pendingCompressWav.clear();
    SetTranscribeStatusText(L"");
    if (path && !path->empty()) {
        // Sidecar stays on the shared stem; point the UI at the M4A.
        Log::Info(L"Recording compressed: " + *path);
        FinishRecordingUi(*path);
    } else {
        FinishRecordingUi({});
    }
    return 0;
}

LRESULT MainWindow::OnM4aFailed(UINT /*msg*/, WPARAM /*wParam*/, LPARAM lParam, BOOL& handled) {
    handled = TRUE;
    m_compressingM4a = false;
    const auto message = TakeOwned<std::wstring>(lParam);
    SetTranscribeStatusText(L"Compress failed");
    const auto wav = m_pendingCompressWav;
    m_pendingCompressWav.clear();
    if (message && !message->empty()) {
        m_tray.ShowBalloon(L"MeetingBuddy", message->c_str());
        ui::ShowMessage(m_hWnd, message->c_str(), MB_OK | MB_ICONWARNING);
    }
    FinishRecordingUi(wav);
    return 0;
}

LRESULT MainWindow::OnRecordingError(UINT /*msg*/, WPARAM /*wParam*/, LPARAM lParam,
                                     BOOL& handled) {
    handled = TRUE;
    const auto message = TakeOwned<std::wstring>(lParam);
    if (m_session.IsActive()) {
        m_session.Stop(RecordingSession::StopMode::Save);
    }
    const auto text = message ? *message : std::wstring{};
    EnterIdleUi({}, false, &text);
    return 0;
}

LRESULT MainWindow::OnDevicesChanged(UINT /*msg*/, WPARAM /*wParam*/, LPARAM /*lParam*/,
                                     BOOL& handled) {
    handled = TRUE;
    m_recordView.OnDevicesChanged();
    return 0;
}

LRESULT MainWindow::OnDeviceSwitched(UINT /*msg*/, WPARAM /*wParam*/, LPARAM lParam,
                                     BOOL& handled) {
    handled = TRUE;
    const auto message = TakeOwned<std::wstring>(lParam);
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
    EnterIdleUi({}, false);
    m_tray.ShowBalloon(L"MeetingBuddy", L"Recording stopped: low disk space");
    RefreshRecordingsList();
    return 0;
}

LRESULT MainWindow::OnRecordingRescued(UINT /*msg*/, WPARAM /*wParam*/, LPARAM lParam,
                                       BOOL& handled) {
    handled = TRUE;
    const auto path = TakeOwned<std::wstring>(lParam);
    if (m_session.IsActive()) {
        m_session.Stop(RecordingSession::StopMode::Save);
    }
    // The modal message box below pumps messages, so the elapsed timer must already be dead.
    KillTimer(IDT_RECORD_ELAPSED);
    if (path && !path->empty()) {
        WriteSidecarForPath(*path);
        const auto text = L"Recording folder unavailable. Saved to:\n" + *path;
        m_tray.ShowBalloon(L"MeetingBuddy", text.c_str());
        ui::ShowMessage(m_hWnd, text.c_str(), MB_OK | MB_ICONWARNING);
        Log::Warn(text);
    }
    EnterIdleUi(path ? *path : std::wstring{}, true);
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
    popup.ModifyMenu(ID_TRAY_STARTSTOP, MF_BYCOMMAND | MF_STRING, ID_TRAY_STARTSTOP,
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
    const auto folder = Settings::Instance().RecordingsFolder();
    Paths::EnsureDirectory(folder);
    ::ShellExecuteW(m_hWnd, L"explore", folder.c_str(), nullptr, nullptr, SW_SHOWNORMAL);
}

void MainWindow::OnSettings(UINT /*code*/, int /*id*/, HWND /*control*/) {
    SettingsDialog dialog(&m_devices);
    if (dialog.DoModal(m_hWnd) != IDOK) {
        return;
    }

    Autostart::Apply(Settings::Instance().Data().startWithWindows,
                     Settings::Instance().Data().startMinimized);
    RegisterConfiguredHotkey();

    if (m_recordView.IsWindow()) {
        m_recordView.SyncControlsToSettings();
        if (!m_session.IsActive()) {
            m_recordView.RefreshDevices();
        }
    }
    if (dialog.FolderChanged() || m_recordingsView.IsWindow()) {
        m_recordingsView.EnsureWatching();
        RefreshRecordingsList();
    }
}

void MainWindow::OnAbout(UINT /*code*/, int /*id*/, HWND /*control*/) {
    auto text = std::wstring(kAppName) + L" " + kAppVersion + L"\n\n" +
                L"Local meeting recorder and optional transcription.\n\n" +
                L"Transcription is local. The only network use is a model download you start.";
    ::MessageBoxW(m_hWnd, text.c_str(), L"About MeetingBuddy", MB_OK | MB_ICONINFORMATION);
}

void MainWindow::OnTrayShow(UINT /*code*/, int /*id*/, HWND /*control*/) {
    ShowFromTray();
}

void MainWindow::OnTrayStartStop(UINT /*code*/, int /*id*/, HWND /*control*/) {
    m_recordView.ToggleRecording();
}

LRESULT MainWindow::OnTabSelChange(LPNMHDR /*header*/) {
    ShowSelectedTab();
    LayoutChildren();
    return 0;
}

LRESULT MainWindow::OnTranscribeQueued(UINT /*msg*/, WPARAM /*wParam*/, LPARAM lParam,
                                       BOOL& handled) {
    handled = TRUE;
    const auto notify = TakeOwned<TranscribeNotify>(lParam);
    SetTranscribeStatusText(L"Queued");
    RefreshRecordingsList();
    (void)notify;
    return 0;
}

LRESULT MainWindow::OnTranscribeProgress(UINT /*msg*/, WPARAM /*wParam*/, LPARAM lParam,
                                         BOOL& handled) {
    handled = TRUE;
    const auto notify = TakeOwned<TranscribeNotify>(lParam);
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

LRESULT MainWindow::OnTranscribeFinished(UINT /*msg*/, WPARAM /*wParam*/, LPARAM lParam,
                                         BOOL& handled) {
    handled = TRUE;
    const auto notify = TakeOwned<TranscribeNotify>(lParam);
    SetTranscribeStatusText(L"Ready");
    RefreshRecordingsList();
    if (notify && !notify->audioPath.empty()) {
        m_balloonAudioPath = notify->audioPath;
        m_tray.ShowBalloon(L"MeetingBuddy", L"Transcript ready — click to open");
        if (IsWindowVisible() && !IsIconic()) {
            if (m_tabs.IsWindow()) {
                m_tabs.SetCurSel(1);
                ShowSelectedTab();
                LayoutChildren();
            }
            if (m_recordingsView.IsWindow()) {
                m_recordingsView.SelectByAudioPath(notify->audioPath);
            }
        }
    }
    return 0;
}

LRESULT MainWindow::OnTranscribeFailed(UINT /*msg*/, WPARAM /*wParam*/, LPARAM lParam,
                                       BOOL& handled) {
    handled = TRUE;
    const auto notify = TakeOwned<TranscribeNotify>(lParam);
    if (notify && !notify->message.empty()) {
        SetTranscribeStatusText(notify->message);
    } else {
        SetTranscribeStatusText(L"Failed");
    }
    RefreshRecordingsList();
    return 0;
}
