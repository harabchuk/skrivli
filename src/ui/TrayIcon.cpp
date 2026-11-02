#include "ui/TrayIcon.hpp"

#include "util/Log.hpp"

TrayIcon::~TrayIcon() {
    Remove();
    // Icons are loaded with LR_SHARED; do not DestroyIcon them.
    m_idleIcon = nullptr;
    m_recordingIcon = nullptr;
}

HICON TrayIcon::LoadIdleIcon() const {
    return static_cast<HICON>(::LoadImageW(
        _Module.GetResourceInstance(),
        MAKEINTRESOURCEW(IDI_TRAY_IDLE),
        IMAGE_ICON,
        0,
        0,
        LR_DEFAULTSIZE | LR_SHARED));
}

HICON TrayIcon::LoadRecordingIcon() const {
    return static_cast<HICON>(::LoadImageW(
        _Module.GetResourceInstance(),
        MAKEINTRESOURCEW(IDI_TRAY_RECORDING),
        IMAGE_ICON,
        0,
        0,
        LR_DEFAULTSIZE | LR_SHARED));
}

void TrayIcon::FillBase(NOTIFYICONDATAW& data) const {
    data = {};
    data.cbSize = sizeof(data);
    data.hWnd = m_owner;
    data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP | NIF_GUID;
    data.uCallbackMessage = WM_APP_TRAYICON;
    data.guidItem = kIconGuid;
    data.uVersion = NOTIFYICON_VERSION_4;
    wcsncpy_s(data.szTip, m_tip, _TRUNCATE);
    data.hIcon = m_recording ? m_recordingIcon : m_idleIcon;
}

bool TrayIcon::Add(HWND owner) {
    if (owner == nullptr) {
        return false;
    }

    m_owner = owner;
    if (m_idleIcon == nullptr) {
        m_idleIcon = LoadIdleIcon();
    }
    if (m_recordingIcon == nullptr) {
        m_recordingIcon = LoadRecordingIcon();
    }
    if (m_idleIcon == nullptr) {
        Log::Error(L"Failed to load tray idle icon");
        return false;
    }

    NOTIFYICONDATAW data{};
    FillBase(data);

    if (!::Shell_NotifyIconW(NIM_ADD, &data)) {
        // Icon may already exist after an unclean exit; try replace.
        if (!::Shell_NotifyIconW(NIM_DELETE, &data) || !::Shell_NotifyIconW(NIM_ADD, &data)) {
            Log::Error(L"Shell_NotifyIcon NIM_ADD failed");
            return false;
        }
    }

    data.uVersion = NOTIFYICON_VERSION_4;
    m_version4 = ::Shell_NotifyIconW(NIM_SETVERSION, &data) != FALSE;
    if (!m_version4) {
        Log::Warn(L"Shell_NotifyIcon NIM_SETVERSION failed");
    }

    m_added = true;
    return true;
}

void TrayIcon::Remove() {
    if (!m_added || m_owner == nullptr) {
        return;
    }

    NOTIFYICONDATAW data{};
    FillBase(data);
    ::Shell_NotifyIconW(NIM_DELETE, &data);
    m_added = false;
    m_version4 = false;
}

bool TrayIcon::ReAdd() {
    if (m_owner == nullptr) {
        return false;
    }
    Remove();
    return Add(m_owner);
}

void TrayIcon::ShowBalloon(const wchar_t* title, const wchar_t* text) {
    if (!m_added || m_owner == nullptr) {
        return;
    }

    NOTIFYICONDATAW data{};
    FillBase(data);
    data.uFlags |= NIF_INFO;
    data.dwInfoFlags = NIIF_INFO;
    if (title != nullptr) {
        wcsncpy_s(data.szInfoTitle, title, _TRUNCATE);
    }
    if (text != nullptr) {
        wcsncpy_s(data.szInfo, text, _TRUNCATE);
    }
    ::Shell_NotifyIconW(NIM_MODIFY, &data);
}

void TrayIcon::SetRecording(bool recording) {
    m_recording = recording;
    if (!recording) {
        wcsncpy_s(m_tip, L"MeetingBuddy", _TRUNCATE);
    }
    if (!m_added) {
        return;
    }

    NOTIFYICONDATAW data{};
    FillBase(data);
    ::Shell_NotifyIconW(NIM_MODIFY, &data);
}

void TrayIcon::SetTip(const wchar_t* tip) {
    if (tip == nullptr || tip[0] == L'\0') {
        wcsncpy_s(m_tip, L"MeetingBuddy", _TRUNCATE);
    } else {
        wcsncpy_s(m_tip, tip, _TRUNCATE);
    }
    if (!m_added) {
        return;
    }

    NOTIFYICONDATAW data{};
    FillBase(data);
    ::Shell_NotifyIconW(NIM_MODIFY, &data);
}
