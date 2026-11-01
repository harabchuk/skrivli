#include "ui/RecordView.hpp"

#include "storage/Settings.hpp"
#include "util/Log.hpp"
#include "util/Paths.hpp"

#include <commctrl.h>
#include <shellapi.h>
#include <shobjidl.h>
#include <wrl/client.h>

#include <cmath>
#include <cstdlib>
#include <cstdio>
#include <filesystem>

namespace {

std::wstring FormatElapsed(uint64_t elapsedQpc, uint64_t frequency) {
    if (frequency == 0) {
        frequency = 1;
    }
    const uint64_t totalSeconds = elapsedQpc / frequency;
    const uint64_t hours = totalSeconds / 3600;
    const uint64_t minutes = (totalSeconds / 60) % 60;
    const uint64_t seconds = totalSeconds % 60;
    wchar_t buffer[32]{};
    std::swprintf(buffer, 32, L"%02llu:%02llu:%02llu", hours, minutes, seconds);
    return buffer;
}

}  // namespace

BOOL RecordView::OnInitDialog(CWindow /*focus*/, LPARAM /*initParam*/) {
    DlgResize_Init(false, false);

    m_micCombo.Attach(GetDlgItem(IDC_MIC_DEVICE));
    m_systemCombo.Attach(GetDlgItem(IDC_SYSTEM_DEVICE));
    m_folderEdit.Attach(GetDlgItem(IDC_FOLDER_PATH));
    m_recordButton.Attach(GetDlgItem(IDC_RECORD_BUTTON));
    m_elapsedLabel.Attach(GetDlgItem(IDC_ELAPSED));
    m_warningLabel.Attach(GetDlgItem(IDC_DEVICE_WARNING));
    m_lastRecordingLabel.Attach(GetDlgItem(IDC_LAST_RECORDING));

    m_micMeter.SubclassWindow(GetDlgItem(IDC_METER_MIC));
    m_systemMeter.SubclassWindow(GetDlgItem(IDC_METER_SYSTEM));

    LARGE_INTEGER freq{};
    ::QueryPerformanceFrequency(&freq);
    m_qpcFrequency = static_cast<uint64_t>(freq.QuadPart);

    SyncControlsToSettings();
    RefreshDevices();
    UpdateRecordButton();
    SetWarning({});
    HidePostRecordPrompt();
    return TRUE;
}

void RecordView::ClearComboItemData(WTL::CComboBox& combo) {
    for (int i = 0; i < combo.GetCount(); ++i) {
        wchar_t* itemId = static_cast<wchar_t*>(combo.GetItemDataPtr(i));
        if (itemId != nullptr) {
            free(itemId);
            combo.SetItemDataPtr(i, nullptr);
        }
    }
}

void RecordView::OnDestroy() {
    KillTimer(IDT_RECORD_ELAPSED);
    KillTimer(IDT_RECORD_METERS);
    if (m_micCombo.IsWindow()) {
        ClearComboItemData(m_micCombo);
    }
    if (m_systemCombo.IsWindow()) {
        ClearComboItemData(m_systemCombo);
    }
    SetMsgHandled(FALSE);
}

void RecordView::SyncControlsToSettings() {
    const std::wstring folder = Settings::Instance().RecordingsFolder();
    m_folderEdit.SetWindowText(folder.c_str());
}

void RecordView::SetWarning(const std::wstring& text) {
    if (m_warningLabel.IsWindow()) {
        m_warningLabel.SetWindowText(text.c_str());
    }
}

void RecordView::RefreshDevices() {
    if (m_devices == nullptr || !m_micCombo.IsWindow() || !m_systemCombo.IsWindow()) {
        return;
    }

    const std::wstring selectedMic = Settings::Instance().Data().micDeviceId;
    const std::wstring selectedSys = Settings::Instance().Data().systemDeviceId;

    ClearComboItemData(m_micCombo);
    m_micCombo.ResetContent();
    const auto capture = m_devices->EnumerateCapture();
    for (const auto& device : capture) {
        const int index = m_micCombo.AddString(device.name.c_str());
        m_micCombo.SetItemDataPtr(index, _wcsdup(device.id.c_str()));
    }
    SelectDeviceId(IDC_MIC_DEVICE, selectedMic);

    ClearComboItemData(m_systemCombo);
    m_systemCombo.ResetContent();
    const auto render = m_devices->EnumerateRender();
    for (const auto& device : render) {
        const int index = m_systemCombo.AddString(device.name.c_str());
        m_systemCombo.SetItemDataPtr(index, _wcsdup(device.id.c_str()));
    }
    SelectDeviceId(IDC_SYSTEM_DEVICE, selectedSys);

    if (!IsRecording()) {
        std::wstring warning;
        if (capture.size() <= 1) {
            warning = L"No microphone detected — system-only recording is available.";
        } else if (render.size() <= 1) {
            warning = L"No system audio device — microphone-only recording is available.";
        }
        SetWarning(warning);
    }
}

void RecordView::SelectDeviceId(int comboId, const std::wstring& id) {
    WTL::CComboBox combo(GetDlgItem(comboId));
    int fallback = 0;
    for (int i = 0; i < combo.GetCount(); ++i) {
        const wchar_t* itemId = static_cast<const wchar_t*>(combo.GetItemDataPtr(i));
        const std::wstring value = itemId != nullptr ? itemId : L"";
        if (value == id) {
            combo.SetCurSel(i);
            return;
        }
        if (value.empty()) {
            fallback = i;
        }
    }
    combo.SetCurSel(fallback);
}

std::wstring RecordView::SelectedDeviceId(int comboId) const {
    WTL::CComboBox combo(GetDlgItem(comboId));
    const int sel = combo.GetCurSel();
    if (sel < 0) {
        return {};
    }
    const wchar_t* itemId = static_cast<const wchar_t*>(combo.GetItemDataPtr(sel));
    return itemId != nullptr ? itemId : L"";
}

void RecordView::PersistDeviceSelection() {
    Settings::Instance().Data().micDeviceId = SelectedDeviceId(IDC_MIC_DEVICE);
    Settings::Instance().Data().systemDeviceId = SelectedDeviceId(IDC_SYSTEM_DEVICE);
    Settings::Instance().Save();
}

void RecordView::PersistFolder() {
    CString text;
    m_folderEdit.GetWindowText(text);
    Settings::Instance().Data().folder = text.GetString();
    Settings::Instance().Save();
}

void RecordView::OnMicSelChange(UINT /*code*/, int /*id*/, HWND /*control*/) {
    PersistDeviceSelection();
}

void RecordView::OnSystemSelChange(UINT /*code*/, int /*id*/, HWND /*control*/) {
    PersistDeviceSelection();
}

void RecordView::OnDevicesChanged() {
    if (IsRecording()) {
        if (m_session != nullptr) {
            m_session->OnDevicesChanged();
        }
        return;
    }
    RefreshDevices();
}

bool RecordView::FolderIsWritable(const std::wstring& folder) const {
    if (!Paths::EnsureDirectory(folder)) {
        return false;
    }
    const std::wstring probe = folder + L"\\.meetingbuddy_write_test";
    HANDLE file = ::CreateFileW(
        probe.c_str(),
        GENERIC_WRITE,
        0,
        nullptr,
        CREATE_ALWAYS,
        FILE_ATTRIBUTE_TEMPORARY | FILE_FLAG_DELETE_ON_CLOSE,
        nullptr);
    if (file == INVALID_HANDLE_VALUE) {
        return false;
    }
    ::CloseHandle(file);
    return true;
}

bool RecordView::BrowseForFolder(std::wstring& path) {
    Microsoft::WRL::ComPtr<IFileOpenDialog> dialog;
    HRESULT hr = ::CoCreateInstance(
        CLSID_FileOpenDialog,
        nullptr,
        CLSCTX_ALL,
        IID_PPV_ARGS(&dialog));
    if (FAILED(hr) || !dialog) {
        return false;
    }

    DWORD options = 0;
    dialog->GetOptions(&options);
    dialog->SetOptions(options | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM);

    hr = dialog->Show(m_hWnd);
    if (hr == HRESULT_FROM_WIN32(ERROR_CANCELLED)) {
        return false;
    }
    if (FAILED(hr)) {
        return false;
    }

    Microsoft::WRL::ComPtr<IShellItem> item;
    hr = dialog->GetResult(&item);
    if (FAILED(hr) || !item) {
        return false;
    }

    PWSTR filePath = nullptr;
    hr = item->GetDisplayName(SIGDN_FILESYSPATH, &filePath);
    if (FAILED(hr) || filePath == nullptr) {
        return false;
    }
    path = filePath;
    ::CoTaskMemFree(filePath);
    return true;
}

void RecordView::OnBrowseFolder(UINT /*code*/, int /*id*/, HWND /*control*/) {
    std::wstring path;
    if (!BrowseForFolder(path)) {
        return;
    }
    if (!FolderIsWritable(path)) {
        ::MessageBoxW(
            m_hWnd,
            L"That folder is not writable. Choose another location.",
            L"MeetingBuddy",
            MB_OK | MB_ICONWARNING);
        return;
    }
    m_folderEdit.SetWindowText(path.c_str());
    PersistFolder();
}

void RecordView::SetInputsEnabled(bool enabled) {
    m_micCombo.EnableWindow(enabled);
    m_systemCombo.EnableWindow(enabled);
    m_folderEdit.EnableWindow(enabled);
    ::EnableWindow(GetDlgItem(IDC_BROWSE_FOLDER), enabled ? TRUE : FALSE);
}

void RecordView::UpdateRecordButton() {
    if (IsRecording()) {
        m_recordButton.SetWindowText(L"Stop");
    } else {
        m_recordButton.SetWindowText(L"Record");
    }
}

bool RecordView::IsRecording() const {
    return m_session != nullptr && m_session->IsActive();
}

void RecordView::ShowPrivacyDeniedDialog() {
    TASKDIALOGCONFIG config{};
    config.cbSize = sizeof(config);
    config.hwndParent = m_hWnd;
    config.dwFlags = TDF_ALLOW_DIALOG_CANCELLATION;
    config.dwCommonButtons = TDCBF_CANCEL_BUTTON;
    config.pszWindowTitle = L"MeetingBuddy";
    config.pszMainIcon = TD_WARNING_ICON;
    config.pszMainInstruction = L"Microphone access is turned off";
    config.pszContent =
        L"Windows is blocking microphone access for desktop apps.\n\n"
        L"Open Privacy settings to allow MeetingBuddy to record.";
    config.pszFooter = L"Recording was not started.";

    TASKDIALOG_BUTTON buttons[] = {
        { 1001, L"Open microphone settings" },
    };
    config.cButtons = 1;
    config.pButtons = buttons;
    config.nDefaultButton = 1001;

    int clicked = 0;
    if (SUCCEEDED(::TaskDialogIndirect(&config, &clicked, nullptr, nullptr)) && clicked == 1001) {
        ::ShellExecuteW(m_hWnd, L"open", L"ms-settings:privacy-microphone", nullptr, nullptr, SW_SHOWNORMAL);
    }
}

bool RecordView::ToggleRecording() {
    if (m_session == nullptr) {
        return false;
    }

    if (IsRecording()) {
        m_session->Stop(RecordingSession::StopMode::Save);
        return true;
    }

    CString folderText;
    m_folderEdit.GetWindowText(folderText);
    std::wstring folder = folderText.GetString();
    if (folder.empty()) {
        folder = Paths::DefaultRecordingsDir();
        m_folderEdit.SetWindowText(folder.c_str());
    }
    if (!FolderIsWritable(folder)) {
        ::MessageBoxW(
            m_hWnd,
            L"The recordings folder is missing or not writable.",
            L"MeetingBuddy",
            MB_OK | MB_ICONWARNING);
        return false;
    }

    PersistFolder();
    PersistDeviceSelection();

    const auto& settings = Settings::Instance().Data();
    const RecordingStartResult result = m_session->Start(
        folder,
        settings.micDeviceId,
        settings.systemDeviceId,
        settings.micGain,
        settings.systemGain,
        settings.communicationsProcessing);

    if (result.privacyDenied) {
        ShowPrivacyDeniedDialog();
        return false;
    }
    if (!result.started) {
        if (!result.error.empty()) {
            ::MessageBoxW(m_hWnd, result.error.c_str(), L"MeetingBuddy", MB_OK | MB_ICONERROR);
        }
        return false;
    }
    if (!result.warning.empty()) {
        SetWarning(result.warning);
    }
    return true;
}

void RecordView::OnRecordClicked(UINT /*code*/, int /*id*/, HWND /*control*/) {
    ToggleRecording();
}

void RecordView::OnRecordingStarted() {
    m_startedQpc = m_session != nullptr ? m_session->StartQpc() : 0;
    SetInputsEnabled(false);
    UpdateRecordButton();
    SetTimer(IDT_RECORD_ELAPSED, 250);
    SetTimer(IDT_RECORD_METERS, 33);
    UpdateElapsed();
}

void RecordView::OnRecordingFinished(const std::wstring& path) {
    KillTimer(IDT_RECORD_ELAPSED);
    KillTimer(IDT_RECORD_METERS);
    SetInputsEnabled(true);
    UpdateRecordButton();
    m_elapsedLabel.SetWindowText(L"00:00:00");
    m_micMeter.SetLevel(0.0f);
    m_systemMeter.SetLevel(0.0f);
    RefreshDevices();
    if (!path.empty() && !Settings::Instance().Data().autoTranscribe) {
        ShowPostRecordPrompt(path);
    } else {
        HidePostRecordPrompt();
        if (!path.empty()) {
            m_lastRecordingPath = path;
        }
    }
}

void RecordView::ShowPostRecordPrompt(const std::wstring& path) {
    m_lastRecordingPath = path;
    const std::wstring leaf = std::filesystem::path(path).filename().wstring();
    m_lastRecordingLabel.SetWindowText((L"Last recording: " + leaf).c_str());
    m_lastRecordingLabel.ShowWindow(SW_SHOW);
    ::EnableWindow(GetDlgItem(IDC_TRANSCRIBE_NOW), TRUE);
    ::EnableWindow(GetDlgItem(IDC_OPEN_LAST_FOLDER), TRUE);
    ::ShowWindow(GetDlgItem(IDC_TRANSCRIBE_NOW), SW_SHOW);
    ::ShowWindow(GetDlgItem(IDC_OPEN_LAST_FOLDER), SW_SHOW);
}

void RecordView::HidePostRecordPrompt() {
    if (m_lastRecordingLabel.IsWindow()) {
        m_lastRecordingLabel.SetWindowText(L"");
        m_lastRecordingLabel.ShowWindow(SW_HIDE);
    }
    ::EnableWindow(GetDlgItem(IDC_TRANSCRIBE_NOW), FALSE);
    ::EnableWindow(GetDlgItem(IDC_OPEN_LAST_FOLDER), FALSE);
    ::ShowWindow(GetDlgItem(IDC_TRANSCRIBE_NOW), SW_HIDE);
    ::ShowWindow(GetDlgItem(IDC_OPEN_LAST_FOLDER), SW_HIDE);
}

void RecordView::OnTranscribeNow(UINT /*code*/, int /*id*/, HWND /*control*/) {
    if (m_lastRecordingPath.empty()) {
        return;
    }
    // Hide the prompt (Not now is the same as dismissing).
    ::ShowWindow(GetDlgItem(IDC_TRANSCRIBE_NOW), SW_HIDE);
    ::EnableWindow(GetDlgItem(IDC_TRANSCRIBE_NOW), FALSE);
    if (m_transcribeFn != nullptr) {
        m_transcribeFn(m_transcribeContext, m_lastRecordingPath);
    }
}

void RecordView::OnOpenLastFolder(UINT /*code*/, int /*id*/, HWND /*control*/) {
    if (m_lastRecordingPath.empty()) {
        return;
    }
    const std::wstring params = L"/select,\"" + m_lastRecordingPath + L"\"";
    ::ShellExecuteW(m_hWnd, L"open", L"explorer.exe", params.c_str(), nullptr, SW_SHOWNORMAL);
}

void RecordView::OnRecordingError(const std::wstring& message) {
    KillTimer(IDT_RECORD_ELAPSED);
    KillTimer(IDT_RECORD_METERS);
    SetInputsEnabled(true);
    UpdateRecordButton();
    if (!message.empty()) {
        ::MessageBoxW(m_hWnd, message.c_str(), L"MeetingBuddy", MB_OK | MB_ICONERROR);
    }
}

void RecordView::UpdateElapsed() {
    if (m_startedQpc == 0) {
        return;
    }
    LARGE_INTEGER now{};
    ::QueryPerformanceCounter(&now);
    const uint64_t elapsed = static_cast<uint64_t>(now.QuadPart) - m_startedQpc;
    m_elapsedLabel.SetWindowText(FormatElapsed(elapsed, m_qpcFrequency).c_str());
}

void RecordView::UpdateMeters() {
    if (m_session == nullptr) {
        return;
    }
    m_micMeter.SetLevel(m_session->PeakMic());
    m_systemMeter.SetLevel(m_session->PeakSys());
}

void RecordView::OnTimer(UINT_PTR id) {
    if (id == IDT_RECORD_ELAPSED) {
        UpdateElapsed();
    } else if (id == IDT_RECORD_METERS) {
        UpdateMeters();
    }
}
