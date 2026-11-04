#pragma once

#include "pch.hpp"

class LevelMeter : public CWindowImpl<LevelMeter> {
  public:
    DECLARE_WND_CLASS(L"Skrivli.LevelMeter")

    BEGIN_MSG_MAP_EX(LevelMeter)
    MSG_WM_PAINT(OnPaint)
    MSG_WM_ERASEBKGND(OnEraseBkgnd)
    MSG_WM_SIZE(OnSize)
    END_MSG_MAP()

    // peak is linear 0..1 amplitude.
    void SetLevel(float peak);
    float DisplayDb() const {
        return m_displayDb;
    }

  private:
    void OnPaint(CDCHandle /*dc*/);
    BOOL OnEraseBkgnd(CDCHandle /*dc*/);
    void OnSize(UINT type, CSize size);

    void PaintTo(CDCHandle dc, const CRect& rect);
    static float PeakToDb(float peak);
    static float DbToNormalized(float db);

    float m_display = 0.0f;
    float m_displayDb = -60.0f;
    float m_peakHold = 0.0f;
    int m_peakHoldTicks = 0;
};
