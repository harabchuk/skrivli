#pragma once

#include "pch.hpp"

// Stock tab layout and input, with the selected button painted white and open
// along the bottom so the strip's baseline can run into its sides.
class TabCtrl : public CWindowImpl<TabCtrl, WTL::CTabCtrl> {
public:
    DECLARE_WND_SUPERCLASS(L"MeetingBuddy.TabCtrl", WTL::CTabCtrl::GetWndClassName())

    BEGIN_MSG_MAP_EX(TabCtrl)
        MSG_WM_PAINT(OnPaint)
        MSG_WM_PRINTCLIENT(OnPrintClient)
        MSG_WM_ERASEBKGND(OnEraseBkgnd)
        MSG_WM_SETFOCUS(OnSetFocus)
        MSG_WM_KILLFOCUS(OnKillFocus)
    END_MSG_MAP()

private:
    void OnPaint(CDCHandle /*dc*/);
    void OnPrintClient(CDCHandle dc, UINT /*flags*/);
    BOOL OnEraseBkgnd(CDCHandle /*dc*/);
    void OnSetFocus(CWindow /*focused*/);
    void OnKillFocus(CWindow /*focused*/);

    void PaintTo(CDCHandle dc);
    void DrawLabel(CDCHandle dc, const CRect& rect, const wchar_t* text);
};
