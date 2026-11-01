#pragma once

#include "pch.hpp"

class LevelMeter : public CWindowImpl<LevelMeter> {
public:
    DECLARE_WND_CLASS(L"MeetingBuddy.LevelMeter")

    BEGIN_MSG_MAP_EX(LevelMeter)
        MSG_WM_PAINT(OnPaint)
        MSG_WM_ERASEBKGND(OnEraseBkgnd)
    END_MSG_MAP()

    void SetLevel(float peak);

private:
    void OnPaint(CDCHandle /*dc*/);
    BOOL OnEraseBkgnd(CDCHandle /*dc*/);

    float m_display = 0.0f;
};
