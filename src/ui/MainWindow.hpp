#pragma once

#include "pch.hpp"
#include "Ids.hpp"
#include "audio/DeviceManager.hpp"
#include "audio/RecordingSession.hpp"
#include "storage/Settings.hpp"
#include "ui/RecordView.hpp"
#include "ui/RecordingsView.hpp"
#include "ui/TrayIcon.hpp"

class MainWindow : public WTL::CFrameWindowImpl<MainWindow> {
public:
    DECLARE_FRAME_WND_CLASS(L"MeetingBuddy.MainWindow", IDR_MAINFRAME)

    BEGIN_MSG_MAP_EX(MainWindow)
        MSG_WM_CREATE(OnCreate)
        MSG_WM_SIZE(OnSize)
        MSG_WM_CLOSE(OnClose)
        MSG_WM_DESTROY(OnDestroy)
        MSG_WM_GETMINMAXINFO(OnGetMinMaxInfo)
        MSG_WM_SYSCOMMAND(OnSysCommand)
        MSG_WM_DPICHANGED(OnDpiChanged)
        MSG_WM_EXITSIZEMOVE(OnExitSizeMove)
        MSG_WM_TIMER(OnTimer)
        MESSAGE_RANGE_HANDLER(0xC000, 0xFFFF, OnRegisteredMessage)
        MESSAGE_HANDLER(WM_APP_TRAYICON, OnTrayIcon)
        MESSAGE_HANDLER(WM_APP_RECORDING_STARTED, OnRecordingStarted)
        MESSAGE_HANDLER(WM_APP_RECORDING_FINISHED, OnRecordingFinished)
        MESSAGE_HANDLER(WM_APP_RECORDING_ERROR, OnRecordingError)
        MESSAGE_HANDLER(WM_APP_DEVICES_CHANGED, OnDevicesChanged)
        COMMAND_ID_HANDLER_EX(ID_APP_EXIT, OnFileExit)
        COMMAND_ID_HANDLER_EX(ID_FILE_OPENFOLDER, OnOpenRecordingsFolder)
        COMMAND_ID_HANDLER_EX(ID_FILE_SETTINGS, OnSettings)
        COMMAND_ID_HANDLER_EX(ID_APP_ABOUT, OnAbout)
        COMMAND_ID_HANDLER_EX(ID_TRAY_SHOW, OnTrayShow)
        COMMAND_ID_HANDLER_EX(ID_TRAY_STARTSTOP, OnTrayStartStop)
        COMMAND_ID_HANDLER_EX(ID_TRAY_OPENFOLDER, OnOpenRecordingsFolder)
        COMMAND_ID_HANDLER_EX(ID_TRAY_SETTINGS, OnSettings)
        COMMAND_ID_HANDLER_EX(ID_TRAY_EXIT, OnFileExit)
        NOTIFY_HANDLER_EX(IDC_TAB_MAIN, TCN_SELCHANGE, OnTabSelChange)
        CHAIN_MSG_MAP(WTL::CFrameWindowImpl<MainWindow>)
    END_MSG_MAP()

    void RequestExit();
    void ShowFromTray();
    void RestoreWindowPlacement();

private:
    static constexpr int kMinClientWidth = 560;
    static constexpr int kMinClientHeight = 420;
    static constexpr int kDefaultClientWidth = 720;
    static constexpr int kDefaultClientHeight = 520;

    LRESULT OnCreate(LPCREATESTRUCT createStruct);
    void OnSize(UINT type, CSize size);
    void OnClose();
    void OnDestroy();
    void OnGetMinMaxInfo(LPMINMAXINFO minMaxInfo);
    void OnSysCommand(UINT id, CPoint point);
    void OnDpiChanged(UINT dpiX, UINT dpiY, PRECT suggested);
    void OnExitSizeMove();
    void OnTimer(UINT_PTR id);
    LRESULT OnRegisteredMessage(UINT msg, WPARAM wParam, LPARAM lParam, BOOL& handled);
    LRESULT OnTrayIcon(UINT msg, WPARAM wParam, LPARAM lParam, BOOL& handled);
    LRESULT OnRecordingStarted(UINT msg, WPARAM wParam, LPARAM lParam, BOOL& handled);
    LRESULT OnRecordingFinished(UINT msg, WPARAM wParam, LPARAM lParam, BOOL& handled);
    LRESULT OnRecordingError(UINT msg, WPARAM wParam, LPARAM lParam, BOOL& handled);
    LRESULT OnDevicesChanged(UINT msg, WPARAM wParam, LPARAM lParam, BOOL& handled);
    void OnFileExit(UINT code, int id, HWND control);
    void OnOpenRecordingsFolder(UINT code, int id, HWND control);
    void OnSettings(UINT code, int id, HWND control);
    void OnAbout(UINT code, int id, HWND control);
    void OnTrayShow(UINT code, int id, HWND control);
    void OnTrayStartStop(UINT code, int id, HWND control);
    LRESULT OnTabSelChange(LPNMHDR header);

    void CreateStatusBar();
    void CreateTabs();
    void ApplyDpiFonts(UINT dpi);
    void LayoutChildren();
    void ShowSelectedTab();
    void HideToTray(bool fromClose);
    void PersistWindowPlacement();
    bool IsPlacementOnAnyMonitor(const RECT& windowRect) const;
    void ShowTrayContextMenu();
    void SetStatusIdle();
    void SetStatusRecording();
    void UpdateTrayRecordingTip();

    WTL::CTabCtrl m_tabs;
    WTL::CMultiPaneStatusBarCtrl m_status;
    WTL::CFont m_uiFont;
    RecordView m_recordView;
    RecordingsView m_recordingsView;
    TrayIcon m_tray;
    DeviceManager m_devices;
    RecordingSession m_session;

    bool m_exiting = false;
    bool m_created = false;
    UINT m_taskbarCreatedMsg = 0;
    UINT m_showYourselfMsg = 0;
};
