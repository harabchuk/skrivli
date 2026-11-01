#pragma once

#include "pch.hpp"
#include "Ids.hpp"

class MainWindow : public WTL::CFrameWindowImpl<MainWindow> {
public:
    DECLARE_FRAME_WND_CLASS(L"MeetingBuddy.MainWindow", IDR_MAINFRAME)

    BEGIN_MSG_MAP_EX(MainWindow)
        MSG_WM_CREATE(OnCreate)
        MSG_WM_SIZE(OnSize)
        MSG_WM_CLOSE(OnClose)
        MSG_WM_DESTROY(OnDestroy)
        MSG_WM_ERASEBKGND(OnEraseBkgnd)
        COMMAND_ID_HANDLER_EX(IDC_RECORD, OnRecordClick)
        CHAIN_MSG_MAP(WTL::CFrameWindowImpl<MainWindow>)
    END_MSG_MAP()

private:
    static constexpr int kButtonWidth = 140;
    static constexpr int kButtonHeight = 44;

    WTL::CButton m_recordButton;
    bool m_recording = false;

    LRESULT OnCreate(LPCREATESTRUCT createStruct);
    void OnSize(UINT type, CSize size);
    void OnClose();
    void OnDestroy();
    LRESULT OnEraseBkgnd(HDC hdc);
    void OnRecordClick(UINT code, int id, HWND control);
    void LayoutControls();
};
