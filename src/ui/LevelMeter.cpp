#include "ui/LevelMeter.hpp"

#include <algorithm>

void LevelMeter::SetLevel(float peak) {
    const float clamped = std::clamp(peak, 0.0f, 1.0f);
    // Smooth decay so speech stays visible between peaks.
    constexpr float attack = 0.55f;
    constexpr float release = 0.12f;
    if (clamped >= m_display) {
        m_display = m_display + (clamped - m_display) * attack;
    } else {
        m_display = m_display + (clamped - m_display) * release;
    }
    if (IsWindow()) {
        Invalidate(FALSE);
    }
}

BOOL LevelMeter::OnEraseBkgnd(CDCHandle /*dc*/) {
    return TRUE;
}

void LevelMeter::OnPaint(CDCHandle /*dc*/) {
    CPaintDC paint(m_hWnd);
    CRect rect;
    GetClientRect(&rect);

    paint.FillSolidRect(&rect, RGB(32, 32, 32));

    CRect fill = rect;
    fill.right = fill.left + static_cast<LONG>(rect.Width() * m_display);
    if (fill.right > fill.left) {
        const COLORREF color = (m_display > 0.9f) ? RGB(220, 64, 64)
            : (m_display > 0.7f) ? RGB(220, 180, 48)
                                 : RGB(64, 180, 96);
        paint.FillSolidRect(&fill, color);
    }

    paint.Draw3dRect(&rect, RGB(64, 64, 64), RGB(16, 16, 16));
}
