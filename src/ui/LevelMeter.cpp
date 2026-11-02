#include "ui/LevelMeter.hpp"

#include "ui/Theme.hpp"

#include <algorithm>
#include <cmath>

namespace {

constexpr float kMinDb = -60.0f;
constexpr float kMaxDb = 0.0f;
constexpr int kPeakHoldTicks = 30; // ~1 s at 30 Hz

COLORREF LerpColor(COLORREF a, COLORREF b, float t) {
    t = std::clamp(t, 0.0f, 1.0f);
    return RGB(
        static_cast<int>(GetRValue(a) + (GetRValue(b) - GetRValue(a)) * t),
        static_cast<int>(GetGValue(a) + (GetGValue(b) - GetGValue(a)) * t),
        static_cast<int>(GetBValue(a) + (GetBValue(b) - GetBValue(a)) * t));
}

COLORREF ColorAtNormalized(float norm) {
    // 0..0.7 green, 0.7..0.9 amber, 0.9..1 red
    if (norm < 0.7f) {
        return LerpColor(RGB(64, 180, 96), RGB(180, 200, 64), norm / 0.7f);
    }
    if (norm < 0.9f) {
        return LerpColor(RGB(180, 200, 64), RGB(220, 180, 48), (norm - 0.7f) / 0.2f);
    }
    return LerpColor(RGB(220, 180, 48), RGB(220, 64, 64), (norm - 0.9f) / 0.1f);
}

COLORREF DarkenSysColor(int index, int amount) {
    const COLORREF c = ::GetSysColor(index);
    return RGB(
        std::max(0, GetRValue(c) - amount),
        std::max(0, GetGValue(c) - amount),
        std::max(0, GetBValue(c) - amount));
}

}  // namespace

float LevelMeter::PeakToDb(float peak) {
    const float clamped = std::clamp(peak, 0.0f, 1.0f);
    if (clamped <= 1.0e-6f) {
        return kMinDb;
    }
    return std::clamp(20.0f * std::log10(clamped), kMinDb, kMaxDb);
}

float LevelMeter::DbToNormalized(float db) {
    return std::clamp((db - kMinDb) / (kMaxDb - kMinDb), 0.0f, 1.0f);
}

void LevelMeter::SetLevel(float peak) {
    const float clamped = std::clamp(peak, 0.0f, 1.0f);
    constexpr float attack = 0.55f;
    constexpr float release = 0.12f;
    if (clamped >= m_display) {
        m_display = m_display + (clamped - m_display) * attack;
    } else {
        m_display = m_display + (clamped - m_display) * release;
    }

    m_displayDb = PeakToDb(m_display);
    const float norm = DbToNormalized(m_displayDb);

    if (norm >= m_peakHold) {
        m_peakHold = norm;
        m_peakHoldTicks = kPeakHoldTicks;
    } else if (m_peakHoldTicks > 0) {
        --m_peakHoldTicks;
    } else {
        m_peakHold = std::max(0.0f, m_peakHold - 0.02f);
    }

    if (IsWindow()) {
        Invalidate(FALSE);
    }
}

BOOL LevelMeter::OnEraseBkgnd(CDCHandle /*dc*/) {
    return TRUE;
}

void LevelMeter::OnSize(UINT /*type*/, CSize /*size*/) {
    Invalidate(FALSE);
}

void LevelMeter::PaintTo(CDCHandle dc, const CRect& rect) {
    const UINT dpi = IsWindow() ? ::GetDpiForWindow(m_hWnd) : USER_DEFAULT_SCREEN_DPI;
    const int radius = ui::Scale(4, dpi);
    const int segmentGap = ui::Scale(2, dpi);
    const int segmentWidth = ui::Scale(4, dpi);

    const COLORREF track = DarkenSysColor(COLOR_WINDOW, 28);
    const COLORREF parentBg = ::GetSysColor(COLOR_WINDOW);
    dc.FillSolidRect(&rect, parentBg);

    WTL::CBrush trackBrush;
    trackBrush.CreateSolidBrush(track);
    WTL::CPen trackPen;
    trackPen.CreatePen(PS_SOLID, 1, track);
    HBRUSH oldBrush = dc.SelectBrush(trackBrush);
    HPEN oldPen = dc.SelectPen(trackPen);
    dc.RoundRect(rect.left, rect.top, rect.right, rect.bottom, radius * 2, radius * 2);
    dc.SelectBrush(oldBrush);
    dc.SelectPen(oldPen);

    CRect inner = rect;
    inner.DeflateRect(ui::Scale(2, dpi), ui::Scale(2, dpi));
    if (inner.Width() <= 0 || inner.Height() <= 0) {
        return;
    }

    const float fillNorm = DbToNormalized(m_displayDb);
    const int fillWidth = static_cast<int>(inner.Width() * fillNorm);
    if (fillWidth > 0) {
        const int stride = segmentWidth + segmentGap;
        for (int x = 0; x < fillWidth; x += stride) {
            const int segRight = std::min(x + segmentWidth, fillWidth);
            CRect seg(
                inner.left + x,
                inner.top,
                inner.left + segRight,
                inner.bottom);
            if (seg.Width() <= 0) {
                continue;
            }
            const float mid = (static_cast<float>(x) + static_cast<float>(seg.Width()) * 0.5f) /
                static_cast<float>(inner.Width());
            dc.FillSolidRect(&seg, ColorAtNormalized(mid));
        }
    }

    if (m_peakHold > 0.01f) {
        const int peakX = inner.left + static_cast<int>(inner.Width() * m_peakHold);
        const int tickW = std::max(2, ui::Scale(2, dpi));
        CRect tick(peakX - tickW / 2, inner.top, peakX - tickW / 2 + tickW, inner.bottom);
        tick.left = std::clamp(tick.left, inner.left, inner.right - tickW);
        tick.right = tick.left + tickW;
        dc.FillSolidRect(&tick, ColorAtNormalized(m_peakHold));
    }
}

void LevelMeter::OnPaint(CDCHandle /*dc*/) {
    CPaintDC paint(m_hWnd);
    CRect rect;
    GetClientRect(&rect);
    CMemoryDC mem(paint, rect);
    PaintTo(mem.m_hDC, rect);
}
