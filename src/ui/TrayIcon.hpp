#pragma once

#include "pch.hpp"
#include "Ids.hpp"

class TrayIcon {
public:
    TrayIcon() = default;
    ~TrayIcon();

    TrayIcon(const TrayIcon&) = delete;
    TrayIcon& operator=(const TrayIcon&) = delete;

    bool Add(HWND owner);
    void Remove();
    bool ReAdd();
    void ShowBalloon(const wchar_t* title, const wchar_t* text);
    void SetRecording(bool recording);

private:
    void FillBase(NOTIFYICONDATAW& data) const;
    HICON LoadIdleIcon() const;
    HICON LoadRecordingIcon() const;

    HWND m_owner = nullptr;
    bool m_added = false;
    bool m_recording = false;
    HICON m_idleIcon = nullptr;
    HICON m_recordingIcon = nullptr;

    static constexpr GUID kIconGuid = {
        0xb8e7a41c,
        0x5d3f,
        0x4a2e,
        {0x9c, 0x1b, 0x8f, 0x6e, 0x2d, 0x4a, 0x7b, 0x09}};
};
