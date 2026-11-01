#include "ui/TabCtrl.hpp"

#include "ui/Theme.hpp"

void TabCtrl::OnPaint(CDCHandle /*dc*/) {
    CPaintDC paint(m_hWnd);
    CRect rect;
    GetClientRect(&rect);
    CMemoryDC mem(paint, rect);
    PaintTo(mem.m_hDC);
}

void TabCtrl::OnPrintClient(CDCHandle dc, UINT /*flags*/) {
    PaintTo(dc);
}

BOOL TabCtrl::OnEraseBkgnd(CDCHandle /*dc*/) {
    return TRUE;
}

void TabCtrl::OnMouseMove(UINT /*flags*/, CPoint point) {
    TrackMouse();
    TCHITTESTINFO hit{};
    hit.pt = point;
    hit.flags = TCHT_ONITEM;
    const int index = static_cast<int>(HitTest(&hit));
    if (index != m_hover) {
        InvalidateTab(m_hover);
        InvalidateTab(index);
        m_hover = index;
    }
    SetMsgHandled(FALSE);
}

void TabCtrl::OnMouseLeave() {
    m_tracking = false;
    InvalidateTab(m_hover);
    m_hover = -1;
}

void TabCtrl::OnSetFont(HFONT /*font*/, BOOL /*redraw*/) {
    SetMsgHandled(FALSE);
    Invalidate(FALSE);
}

void TabCtrl::OnSetFocus(CWindow /*focused*/) {
    Invalidate(FALSE);
    SetMsgHandled(FALSE);
}

void TabCtrl::OnKillFocus(CWindow /*focused*/) {
    Invalidate(FALSE);
    SetMsgHandled(FALSE);
}

void TabCtrl::PaintTo(CDCHandle dc) {
    CRect client{};
    GetClientRect(&client);
    dc.FillSolidRect(&client, ui::WindowColor());

    const int count = GetItemCount();
    if (count <= 0) {
        return;
    }

    CRect strip{};
    for (int i = 0; i < count; ++i) {
        CRect item{};
        if (!GetItemRect(i, &item)) {
            continue;
        }
        if (strip.IsRectEmpty()) {
            strip = item;
        } else {
            strip.UnionRect(strip, item);
        }
    }
    if (strip.IsRectEmpty()) {
        return;
    }

    const HFONT font = GetFont();
    HFONT oldFont = font != nullptr ? dc.SelectFont(font) : nullptr;
    dc.SetBkMode(TRANSPARENT);

    const int selected = GetCurSel();
    const UINT dpi = ::GetDpiForWindow(m_hWnd);
    const int underline = std::max(2, ::MulDiv(3, static_cast<int>(dpi), 96));

    for (int i = 0; i < count; ++i) {
        CRect item{};
        if (!GetItemRect(i, &item)) {
            continue;
        }

        wchar_t text[128]{};
        TCITEMW itemInfo{};
        itemInfo.mask = TCIF_TEXT;
        itemInfo.pszText = text;
        itemInfo.cchTextMax = 128;
        if (!GetItem(i, &itemInfo)) {
            continue;
        }

        if (i == selected) {
            dc.SetTextColor(ui::kAccent);
        } else if (i == m_hover) {
            dc.SetTextColor(ui::kHotText);
        } else {
            dc.SetTextColor(ui::SecondaryTextColor());
        }
        CRect textRect = item;
        dc.DrawTextW(text, -1, &textRect, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);

        if (i == selected) {
            CRect bar(item.left, strip.bottom - underline, item.right, strip.bottom);
            dc.FillSolidRect(&bar, ui::kAccent);
            if (GetFocus() == m_hWnd) {
                CRect focus = item;
                focus.InflateRect(-2, -1);
                dc.DrawFocusRect(&focus);
            }
        }
    }

    // Hairline along the strip so the page edge stays visible on white.
    CRect divider(0, strip.bottom, client.right, strip.bottom + 1);
    dc.FillSolidRect(&divider, ui::kDivider);

    if (oldFont != nullptr) {
        dc.SelectFont(oldFont);
    }
}

void TabCtrl::TrackMouse() {
    if (m_tracking) {
        return;
    }
    TRACKMOUSEEVENT track{};
    track.cbSize = sizeof(track);
    track.dwFlags = TME_LEAVE;
    track.hwndTrack = m_hWnd;
    if (::TrackMouseEvent(&track)) {
        m_tracking = true;
    }
}

void TabCtrl::InvalidateTab(int index) {
    if (index < 0) {
        return;
    }
    CRect item{};
    if (GetItemRect(index, &item)) {
        InvalidateRect(&item, FALSE);
    }
}
