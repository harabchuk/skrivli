#include "ui/RecordButton.hpp"

namespace {

constexpr COLORREF kAccentRed = RGB(0xC4, 0x2B, 0x1C);
constexpr COLORREF kAccentRedHot = RGB(0xD1, 0x3B, 0x2C);
constexpr COLORREF kAccentRedPressed = RGB(0xA3, 0x22, 0x15);
constexpr COLORREF kStopFill = RGB(0x2B, 0x2B, 0x2B);
constexpr COLORREF kStopFillHot = RGB(0x3A, 0x3A, 0x3A);
constexpr COLORREF kStopFillPressed = RGB(0x1E, 0x1E, 0x1E);
constexpr COLORREF kDisabledFill = RGB(0xC0, 0xC0, 0xC0);
constexpr COLORREF kFocusRing = RGB(0x00, 0x63, 0xB1);

void FillRoundRect(CDCHandle dc, const CRect& rect, int radius, COLORREF color) {
    WTL::CBrush brush;
    brush.CreateSolidBrush(color);
    WTL::CPen pen;
    pen.CreatePen(PS_SOLID, 1, color);
    const HBRUSH oldBrush = dc.SelectBrush(brush);
    const HPEN oldPen = dc.SelectPen(pen);
    dc.RoundRect(rect.left, rect.top, rect.right, rect.bottom, radius, radius);
    dc.SelectBrush(oldBrush);
    dc.SelectPen(oldPen);
}

void DrawFocusRing(CDCHandle dc, const CRect& rect, int radius) {
    WTL::CPen pen;
    pen.CreatePen(PS_SOLID, 2, kFocusRing);
    const HBRUSH oldBrush = dc.SelectStockBrush(NULL_BRUSH);
    const HPEN oldPen = dc.SelectPen(pen);
    CRect ring = rect;
    ring.InflateRect(2, 2);
    dc.RoundRect(ring.left, ring.top, ring.right, ring.bottom, radius + 2, radius + 2);
    dc.SelectBrush(oldBrush);
    dc.SelectPen(oldPen);
}

}  // namespace

void RecordButton::SetRecording(bool recording) {
    if (m_recording == recording) {
        return;
    }
    m_recording = recording;
    SetWindowText(recording ? L"&Stop" : L"&Start recording");
    if (IsWindow()) {
        Invalidate(FALSE);
    }
}

COLORREF RecordButton::FillColor() const {
    if (!IsWindowEnabled()) {
        return kDisabledFill;
    }
    if (m_recording) {
        if (m_pressed) {
            return kStopFillPressed;
        }
        if (m_hot) {
            return kStopFillHot;
        }
        return kStopFill;
    }
    if (m_pressed) {
        return kAccentRedPressed;
    }
    if (m_hot) {
        return kAccentRedHot;
    }
    return kAccentRed;
}

COLORREF RecordButton::TextColor() const {
    if (!IsWindowEnabled()) {
        return RGB(0x80, 0x80, 0x80);
    }
    return RGB(255, 255, 255);
}

void RecordButton::TrackMouse() {
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

void RecordButton::PaintTo(CDCHandle dc) {
    CRect rect;
    GetClientRect(&rect);

    const COLORREF parentBg = ::GetSysColor(COLOR_BTNFACE);
    dc.FillSolidRect(&rect, parentBg);

    const UINT dpi = ::GetDpiForWindow(m_hWnd);
    const int radius = ::MulDiv(18, static_cast<int>(dpi), 96);
    const int pad = ::MulDiv(1, static_cast<int>(dpi), 96);

    CRect pill = rect;
    pill.DeflateRect(pad + 2, pad + 2);

    FillRoundRect(dc, pill, radius, FillColor());

    const bool showFocus = (GetFocus() == m_hWnd) &&
        ((::SendMessageW(m_hWnd, WM_QUERYUISTATE, 0, 0) & UISF_HIDEFOCUS) == 0);
    if (showFocus) {
        DrawFocusRing(dc, pill, radius);
    }

    CString text;
    GetWindowText(text);
    // Strip mnemonic ampersand for drawing.
    text.Replace(L"&&", L"\x0001");
    text.Remove(L'&');
    text.Replace(L"\x0001", L"&");

    dc.SetBkMode(TRANSPARENT);
    dc.SetTextColor(TextColor());
    HFONT font = GetFont();
    HFONT oldFont = nullptr;
    if (font != nullptr) {
        oldFont = dc.SelectFont(font);
    }

    const int glyphSize = ::MulDiv(10, static_cast<int>(dpi), 96);
    CSize textSize{};
    dc.GetTextExtent(text, text.GetLength(), &textSize);

    const int gap = ::MulDiv(8, static_cast<int>(dpi), 96);
    const int contentWidth = glyphSize + gap + textSize.cx;
    int x = pill.left + (pill.Width() - contentWidth) / 2;
    const int midY = pill.top + pill.Height() / 2;

    if (m_recording) {
        CRect square(
            x,
            midY - glyphSize / 2,
            x + glyphSize,
            midY + glyphSize / 2);
        dc.FillSolidRect(&square, TextColor());
    } else {
        WTL::CBrush brush;
        brush.CreateSolidBrush(TextColor());
        const HBRUSH oldBrush = dc.SelectBrush(brush);
        const HPEN oldPen = dc.SelectStockPen(NULL_PEN);
        const int r = glyphSize / 2;
        dc.Ellipse(x, midY - r, x + glyphSize, midY + r);
        dc.SelectBrush(oldBrush);
        dc.SelectPen(oldPen);
    }
    x += glyphSize + gap;

    CRect textRect(x, pill.top, pill.right - ::MulDiv(8, static_cast<int>(dpi), 96), pill.bottom);
    dc.DrawText(text, text.GetLength(), &textRect, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);

    if (oldFont != nullptr) {
        dc.SelectFont(oldFont);
    }
}

void RecordButton::OnPaint(CDCHandle /*dc*/) {
    CPaintDC paint(m_hWnd);
    CRect rect;
    GetClientRect(&rect);
    CMemoryDC mem(paint, rect);
    PaintTo(mem.m_hDC);
}

void RecordButton::OnPrintClient(CDCHandle dc, UINT /*flags*/) {
    PaintTo(dc);
}

BOOL RecordButton::OnEraseBkgnd(CDCHandle /*dc*/) {
    return TRUE;
}

void RecordButton::OnMouseMove(UINT /*flags*/, CPoint /*point*/) {
    TrackMouse();
    if (!m_hot) {
        m_hot = true;
        Invalidate(FALSE);
    }
}

void RecordButton::OnMouseLeave() {
    m_tracking = false;
    if (m_hot) {
        m_hot = false;
        Invalidate(FALSE);
    }
}

void RecordButton::OnLButtonDown(UINT /*flags*/, CPoint /*point*/) {
    SetCapture();
    m_pressed = true;
    Invalidate(FALSE);
    SetFocus();
}

void RecordButton::OnLButtonUp(UINT /*flags*/, CPoint point) {
    const bool wasPressed = m_pressed;
    m_pressed = false;
    if (GetCapture() == m_hWnd) {
        ReleaseCapture();
    }
    Invalidate(FALSE);

    CRect rect;
    GetClientRect(&rect);
    if (wasPressed && rect.PtInRect(point) && IsWindowEnabled()) {
        ::SendMessageW(GetParent(), WM_COMMAND, MAKEWPARAM(GetDlgCtrlID(), BN_CLICKED), reinterpret_cast<LPARAM>(m_hWnd));
    }
}

void RecordButton::OnCaptureChanged(CWindow /*newWindow*/) {
    if (m_pressed) {
        m_pressed = false;
        Invalidate(FALSE);
    }
}

void RecordButton::OnSetFocus(CWindow /*oldWindow*/) {
    Invalidate(FALSE);
}

void RecordButton::OnKillFocus(CWindow /*newWindow*/) {
    Invalidate(FALSE);
}

void RecordButton::OnEnable(BOOL /*enable*/) {
    Invalidate(FALSE);
    SetMsgHandled(FALSE);
}

LRESULT RecordButton::OnUpdateUiState(UINT /*action*/, UINT /*state*/) {
    Invalidate(FALSE);
    SetMsgHandled(FALSE);
    return 0;
}

void RecordButton::OnKeyDown(UINT virtualKey, UINT /*repeat*/, UINT /*flags*/) {
    if (virtualKey == VK_SPACE || virtualKey == VK_RETURN) {
        if (!m_pressed) {
            m_pressed = true;
            Invalidate(FALSE);
        }
        return;
    }
    SetMsgHandled(FALSE);
}

void RecordButton::OnKeyUp(UINT virtualKey, UINT /*repeat*/, UINT /*flags*/) {
    if (virtualKey == VK_SPACE || virtualKey == VK_RETURN) {
        const bool wasPressed = m_pressed;
        m_pressed = false;
        Invalidate(FALSE);
        if (wasPressed && IsWindowEnabled()) {
            ::SendMessageW(
                GetParent(),
                WM_COMMAND,
                MAKEWPARAM(GetDlgCtrlID(), BN_CLICKED),
                reinterpret_cast<LPARAM>(m_hWnd));
        }
        return;
    }
    SetMsgHandled(FALSE);
}
