#pragma once

#include "pch.hpp"
#include "Ids.hpp"
#include "audio/DeviceManager.hpp"
#include "storage/Settings.hpp"

class SettingsDialog : public CDialogImpl<SettingsDialog> {
public:
    enum { IDD = IDD_SETTINGS };

    explicit SettingsDialog(DeviceManager* devices);

    BEGIN_MSG_MAP_EX(SettingsDialog)
        MSG_WM_INITDIALOG(OnInitDialog)
        COMMAND_ID_HANDLER_EX(IDOK, OnOk)
        COMMAND_ID_HANDLER_EX(IDCANCEL, OnCancel)
        COMMAND_HANDLER_EX(IDC_SET_BROWSE_FOLDER, BN_CLICKED, OnBrowseFolder)
        COMMAND_HANDLER_EX(IDC_SET_MODEL_DOWNLOAD, BN_CLICKED, OnModelDownload)
        COMMAND_HANDLER_EX(IDC_SET_MODEL_BROWSE, BN_CLICKED, OnModelBrowse)
        COMMAND_HANDLER_EX(IDC_SET_COMPRESS_M4A, BN_CLICKED, OnCompressToggled)
        COMMAND_HANDLER_EX(IDC_SET_MIC_AGC, BN_CLICKED, OnMicAgcToggled)
        COMMAND_HANDLER_EX(IDC_SET_HOTKEY_ENABLE, BN_CLICKED, OnHotkeyToggled)
    END_MSG_MAP()

    bool FolderChanged() const { return m_folderChanged; }
    const SettingsData& Applied() const { return m_applied; }

private:
    BOOL OnInitDialog(CWindow focus, LPARAM initParam);
    void OnOk(UINT code, int id, HWND control);
    void OnCancel(UINT code, int id, HWND control);
    void OnBrowseFolder(UINT code, int id, HWND control);
    void OnModelDownload(UINT code, int id, HWND control);
    void OnModelBrowse(UINT code, int id, HWND control);
    void OnCompressToggled(UINT code, int id, HWND control);
    void OnMicAgcToggled(UINT code, int id, HWND control);
    void OnHotkeyToggled(UINT code, int id, HWND control);

    void LoadFromSettings();
    bool CollectTo(SettingsData& out, std::wstring& error) const;
    void FillDeviceCombo(WTL::CComboBox& combo, const std::vector<AudioDeviceInfo>& devices, const std::wstring& selectedId, bool capture);
    void FillModelCombo();
    void FillLanguageCombo();
    void UpdateDependentEnabled();
    std::wstring SelectedDeviceId(const WTL::CComboBox& combo) const;
    void SelectDeviceId(WTL::CComboBox& combo, const std::wstring& id);
    void ClearComboItemData(WTL::CComboBox& combo);
    bool BrowseForFolder(std::wstring& path);

    DeviceManager* m_devices = nullptr;
    SettingsData m_applied{};
    bool m_folderChanged = false;

    WTL::CEdit m_folder;
    WTL::CComboBox m_mic;
    WTL::CComboBox m_sys;
    WTL::CEdit m_micGain;
    WTL::CEdit m_sysGain;
    WTL::CButton m_micAgc;
    WTL::CButton m_saveStems;
    WTL::CButton m_compress;
    WTL::CButton m_keepWav;
    WTL::CButton m_comms;
    WTL::CComboBox m_model;
    WTL::CComboBox m_language;
    WTL::CEdit m_threads;
    WTL::CButton m_autoTranscribe;
    WTL::CButton m_useVad;
    WTL::CButton m_closeTray;
    WTL::CButton m_startWindows;
    WTL::CButton m_startMinimized;
    WTL::CButton m_hotkeyEnable;
    WTL::CEdit m_hotkey;
};
