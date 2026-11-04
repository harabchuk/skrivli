#pragma once

#include "pch.hpp"

class RecordButton : public CWindowImpl<RecordButton, WTL::CButton> {
  public:
    DECLARE_WND_SUPERCLASS(L"Skrivli.RecordButton", CButton::GetWndClassName())

    BEGIN_MSG_MAP_EX(RecordButton)
    MSG_WM_PAINT(OnPaint)
    MSG_WM_PRINTCLIENT(OnPrintClient)
    MSG_WM_ERASEBKGND(OnEraseBkgnd)
    MSG_WM_MOUSEMOVE(OnMouseMove)
    MSG_WM_MOUSELEAVE(OnMouseLeave)
    MSG_WM_LBUTTONDOWN(OnLButtonDown)
    MSG_WM_LBUTTONUP(OnLButtonUp)
    MSG_WM_CAPTURECHANGED(OnCaptureChanged)
    MSG_WM_SETFOCUS(OnSetFocus)
    MSG_WM_KILLFOCUS(OnKillFocus)
    MSG_WM_ENABLE(OnEnable)
    MSG_WM_UPDATEUISTATE(OnUpdateUiState)
    MSG_WM_KEYDOWN(OnKeyDown)
    MSG_WM_KEYUP(OnKeyUp)
    END_MSG_MAP()

    void SetRecording(bool recording);
    bool IsRecordingState() const {
        return m_recording;
    }

  private:
    void OnPaint(CDCHandle /*dc*/);
    void OnPrintClient(CDCHandle dc, UINT /*flags*/);
    BOOL OnEraseBkgnd(CDCHandle /*dc*/);
    void OnMouseMove(UINT flags, CPoint point);
    void OnMouseLeave();
    void OnLButtonDown(UINT flags, CPoint point);
    void OnLButtonUp(UINT flags, CPoint point);
    void OnCaptureChanged(CWindow newWindow);
    void OnSetFocus(CWindow oldWindow);
    void OnKillFocus(CWindow newWindow);
    void OnEnable(BOOL enable);
    LRESULT OnUpdateUiState(UINT action, UINT state);
    void OnKeyDown(UINT virtualKey, UINT repeat, UINT flags);
    void OnKeyUp(UINT virtualKey, UINT repeat, UINT flags);

    void PaintTo(CDCHandle dc);
    void TrackMouse();
    COLORREF FillColor() const;
    COLORREF TextColor() const;

    bool m_recording = false;
    bool m_hot = false;
    bool m_pressed = false;
    bool m_tracking = false;
};
