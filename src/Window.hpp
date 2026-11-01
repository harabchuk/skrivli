#pragma once

#include <atlbase.h>
#include <atlapp.h>
#include <atlctrls.h>
#include <atlframe.h>
#include <atlcrack.h>
#include <atlmisc.h>
#include <atlgdi.h>

#include "Ids.hpp"

class Window : public WTL::CFrameWindowImpl<Window>
{
public:
    DECLARE_FRAME_WND_CLASS(L"MeetingBuddy.MainWindow", IDR_MAINFRAME)

    BEGIN_MSG_MAP_EX(Window)
        MSG_WM_CREATE(OnCreate)
        MSG_WM_SIZE(OnSize)
        MSG_WM_CLOSE(OnClose)
        MSG_WM_DESTROY(OnDestroy)
        MSG_WM_ERASEBKGND(OnEraseBkgnd)
        COMMAND_ID_HANDLER_EX(IDC_RECORD, OnRecordClick)
        CHAIN_MSG_MAP(WTL::CFrameWindowImpl<Window>)
    END_MSG_MAP()

    LRESULT OnCreate(LPCREATESTRUCT lpcs);
    void OnSize(UINT nType, CSize size);
    void OnClose();
    void OnDestroy();
    LRESULT OnEraseBkgnd(HDC hdc);
    void OnRecordClick(UINT uCode, int nID, HWND hwndCtrl);

private:
    void LayoutControls();

    WTL::CButton m_recordButton;
    bool m_recording = false;
};
