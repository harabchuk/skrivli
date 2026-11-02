#include "ui/TabCtrl.hpp"

#include "ui/Theme.hpp"

#include <uxtheme.h>
#include <vsstyle.h>

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

void TabCtrl::OnSetFocus(CWindow /*focused*/) {
    Invalidate(FALSE);
    SetMsgHandled(FALSE);
}

void TabCtrl::OnKillFocus(CWindow /*focused*/) {
    Invalidate(FALSE);
    SetMsgHandled(FALSE);
}

void TabCtrl::DrawLabel(CDCHandle dc, const CRect& rect, const wchar_t* text) {
    if (text == nullptr || text[0] == L'\0') {
        return;
    }
    const HFONT font = GetFont();
    const HFONT oldFont = font != nullptr ? dc.SelectFont(font) : nullptr;
    dc.SetBkMode(TRANSPARENT);
    dc.SetTextColor(ui::TextColor());
    CRect textRect = rect;
    textRect.DeflateRect(2, 1);
    dc.DrawTextW(text, -1, &textRect, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
    if (oldFont != nullptr) {
        dc.SelectFont(oldFont);
    }
}

void TabCtrl::PaintTo(CDCHandle dc) {
    CRect client{};
    GetClientRect(&client);
    dc.FillSolidRect(&client, ui::WindowColor());

    const int count = GetItemCount();
    const int selected = GetCurSel();
    const HTHEME theme = ::OpenThemeData(m_hWnd, VSCLASS_TAB);

    for (int i = 0; i < count; ++i) {
        if (i == selected) {
            continue;
        }
        CRect item{};
        if (!GetItemRect(i, &item)) {
            continue;
        }

        int part = TABP_TABITEM;
        if (count == 1) {
            part = TABP_TABITEMBOTHEDGE;
        } else if (i == 0) {
            part = TABP_TABITEMLEFTEDGE;
        } else if (i == count - 1) {
            part = TABP_TABITEMRIGHTEDGE;
        }
        if (theme != nullptr) {
            ::DrawThemeBackground(theme, dc, part, TIS_NORMAL, &item, nullptr);
        } else {
            dc.FillSolidRect(&item, ::GetSysColor(COLOR_BTNFACE));
            dc.DrawEdge(&item, EDGE_RAISED, BF_RECT);
        }

        wchar_t text[128]{};
        TCITEMW info{};
        info.mask = TCIF_TEXT;
        info.pszText = text;
        info.cchTextMax = 128;
        if (GetItem(i, &info)) {
            DrawLabel(dc, item, text);
        }
    }

    if (theme != nullptr) {
        ::CloseThemeData(theme);
    }

    const int baseline = client.bottom - 1;
    if (baseline >= client.top) {
        dc.FillSolidRect(client.left, baseline, client.Width(), 1, ui::kDivider);
    }

    if (selected < 0) {
        return;
    }
    CRect item{};
    if (!GetItemRect(selected, &item)) {
        return;
    }

    // Cover the baseline under the selected button so its fill meets the page.
    CRect button = item;
    button.bottom = client.bottom;
    dc.FillSolidRect(&button, ui::WindowColor());
    dc.FillSolidRect(button.left, button.top, button.Width(), 1, ui::kDivider);
    dc.FillSolidRect(button.left, button.top, 1, button.Height(), ui::kDivider);
    if (button.Width() > 1) {
        dc.FillSolidRect(button.right - 1, button.top, 1, button.Height(), ui::kDivider);
    }

    wchar_t text[128]{};
    TCITEMW info{};
    info.mask = TCIF_TEXT;
    info.pszText = text;
    info.cchTextMax = 128;
    if (GetItem(selected, &info)) {
        DrawLabel(dc, item, text);
    }
    if (GetFocus() == m_hWnd) {
        CRect focus = item;
        focus.DeflateRect(3, 2);
        if (focus.Width() > 0 && focus.Height() > 0) {
            dc.DrawFocusRect(&focus);
        }
    }
}
