#pragma once

#include "pch.hpp"
#include "Ids.hpp"
#include "audio/DeviceManager.hpp"
#include "audio/RecordingSession.hpp"
#include "ui/LevelMeter.hpp"

#include <string>

class RecordView
    : public CDialogImpl<RecordView>
    , public WTL::CDialogResize<RecordView> {
public:
    enum { IDD = IDD_RECORD_PAGE };

    BEGIN_MSG_MAP_EX(RecordView)
        MSG_WM_INITDIALOG(OnInitDialog)
        MSG_WM_TIMER(OnTimer)
        MSG_WM_DESTROY(OnDestroy)
        COMMAND_HANDLER_EX(IDC_RECORD_BUTTON, BN_CLICKED, OnRecordClicked)
        COMMAND_HANDLER_EX(IDC_BROWSE_FOLDER, BN_CLICKED, OnBrowseFolder)
        COMMAND_HANDLER_EX(IDC_TRANSCRIBE_NOW, BN_CLICKED, OnTranscribeNow)
        COMMAND_HANDLER_EX(IDC_OPEN_LAST_FOLDER, BN_CLICKED, OnOpenLastFolder)
        COMMAND_HANDLER_EX(IDC_MIC_DEVICE, CBN_SELCHANGE, OnMicSelChange)
        COMMAND_HANDLER_EX(IDC_SYSTEM_DEVICE, CBN_SELCHANGE, OnSystemSelChange)
        CHAIN_MSG_MAP(WTL::CDialogResize<RecordView>)
    END_MSG_MAP()

    BEGIN_DLGRESIZE_MAP(RecordView)
        DLGRESIZE_CONTROL(IDC_MIC_DEVICE, DLSZ_SIZE_X)
        DLGRESIZE_CONTROL(IDC_SYSTEM_DEVICE, DLSZ_SIZE_X)
        DLGRESIZE_CONTROL(IDC_FOLDER_PATH, DLSZ_SIZE_X)
        DLGRESIZE_CONTROL(IDC_BROWSE_FOLDER, DLSZ_MOVE_X)
        DLGRESIZE_CONTROL(IDC_METER_MIC, DLSZ_SIZE_X)
        DLGRESIZE_CONTROL(IDC_METER_SYSTEM, DLSZ_SIZE_X)
        DLGRESIZE_CONTROL(IDC_DEVICE_WARNING, DLSZ_SIZE_X)
        DLGRESIZE_CONTROL(IDC_LAST_RECORDING, DLSZ_SIZE_X)
    END_DLGRESIZE_MAP()

    using TranscribeRequestFn = void (*)(void* context, const std::wstring& audioPath);

    void SetSession(RecordingSession* session) { m_session = session; }
    void SetDeviceManager(DeviceManager* devices) { m_devices = devices; }
    void SetTranscribeHandler(TranscribeRequestFn fn, void* context) {
        m_transcribeFn = fn;
        m_transcribeContext = context;
    }

    void RefreshDevices();
    void OnRecordingStarted();
    void OnRecordingFinished(const std::wstring& path);
    void OnRecordingError(const std::wstring& message);
    void OnDevicesChanged();
    void SetWarning(const std::wstring& text);
    void HidePostRecordPrompt();

    bool ToggleRecording();
    bool IsRecording() const;

private:
    BOOL OnInitDialog(CWindow focus, LPARAM initParam);
    void OnDestroy();
    void OnTimer(UINT_PTR id);
    void OnRecordClicked(UINT code, int id, HWND control);
    void OnBrowseFolder(UINT code, int id, HWND control);
    void OnTranscribeNow(UINT code, int id, HWND control);
    void OnOpenLastFolder(UINT code, int id, HWND control);
    void OnMicSelChange(UINT code, int id, HWND control);
    void OnSystemSelChange(UINT code, int id, HWND control);
    void ShowPostRecordPrompt(const std::wstring& path);

    void SyncControlsToSettings();
    void PersistDeviceSelection();
    void PersistFolder();
    void SetInputsEnabled(bool enabled);
    void UpdateRecordButton();
    void UpdateElapsed();
    void UpdateMeters();
    bool BrowseForFolder(std::wstring& path);
    bool FolderIsWritable(const std::wstring& folder) const;
    std::wstring SelectedDeviceId(int comboId) const;
    void SelectDeviceId(int comboId, const std::wstring& id);
    void ClearComboItemData(WTL::CComboBox& combo);
    void ShowPrivacyDeniedDialog();

    RecordingSession* m_session = nullptr;
    DeviceManager* m_devices = nullptr;
    TranscribeRequestFn m_transcribeFn = nullptr;
    void* m_transcribeContext = nullptr;

    WTL::CComboBox m_micCombo;
    WTL::CComboBox m_systemCombo;
    WTL::CEdit m_folderEdit;
    WTL::CButton m_recordButton;
    WTL::CStatic m_elapsedLabel;
    WTL::CStatic m_warningLabel;
    WTL::CStatic m_lastRecordingLabel;
    LevelMeter m_micMeter;
    LevelMeter m_systemMeter;

    uint64_t m_startedQpc = 0;
    uint64_t m_qpcFrequency = 1;
    std::wstring m_lastRecordingPath;
};
