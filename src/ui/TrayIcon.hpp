#pragma once

#include "pch.hpp"
#include "Ids.hpp"

class TrayIcon {
public:
    TrayIcon() = default;
    ~TrayIcon();

    TrayIcon(const TrayIcon&) = delete;
    auto operator=(const TrayIcon&) -> TrayIcon& = delete;

    bool Add(HWND owner);
    void Remove();
    bool ReAdd();
    void ShowBalloon(const wchar_t* title, const wchar_t* text);
    void SetRecording(bool recording);
    void SetTip(const wchar_t* tip);
    bool UsesVersion4() const { return m_version4; }

private:
    void FillBase(NOTIFYICONDATAW& data) const;
    void Modify();
    static void Modify(NOTIFYICONDATAW& data);

    HWND m_owner = nullptr;
    bool m_added = false;
    bool m_version4 = false;
    bool m_recording = false;
    HICON m_idleIcon = nullptr;
    HICON m_recordingIcon = nullptr;
    wchar_t m_tip[128] = L"MeetingBuddy";

    static constexpr GUID kIconGuid = {
        0xb8e7a41c,
        0x5d3f,
        0x4a2e,
        {0x9c, 0x1b, 0x8f, 0x6e, 0x2d, 0x4a, 0x7b, 0x09}};
};
