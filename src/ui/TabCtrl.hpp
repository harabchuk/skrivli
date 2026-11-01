#pragma once

#include "pch.hpp"

// Flat tab strip painted in the page background color: white like the tab
// pages, with an accent label and underline on the selected tab. Keeps the
// native control for layout, hit-testing and keyboard navigation.
class TabCtrl : public CWindowImpl<TabCtrl, WTL::CTabCtrl> {
public:
    DECLARE_WND_SUPERCLASS(L"MeetingBuddy.TabCtrl", WTL::CTabCtrl::GetWndClassName())

    BEGIN_MSG_MAP_EX(TabCtrl)
        MSG_WM_PAINT(OnPaint)
        MSG_WM_PRINTCLIENT(OnPrintClient)
        MSG_WM_ERASEBKGND(OnEraseBkgnd)
        MSG_WM_MOUSEMOVE(OnMouseMove)
        MSG_WM_MOUSELEAVE(OnMouseLeave)
        MSG_WM_SETFONT(OnSetFont)
        MSG_WM_SETFOCUS(OnSetFocus)
        MSG_WM_KILLFOCUS(OnKillFocus)
    END_MSG_MAP()

private:
    void OnPaint(CDCHandle /*dc*/);
    void OnPrintClient(CDCHandle dc, UINT /*flags*/);
    BOOL OnEraseBkgnd(CDCHandle /*dc*/);
    void OnMouseMove(UINT flags, CPoint point);
    void OnMouseLeave();
    void OnSetFont(HFONT font, BOOL redraw);
    void OnSetFocus(CWindow focused);
    void OnKillFocus(CWindow focused);

    void PaintTo(CDCHandle dc);
    void TrackMouse();
    void InvalidateTab(int index);

    bool m_tracking = false;
    int m_hover = -1;
};
