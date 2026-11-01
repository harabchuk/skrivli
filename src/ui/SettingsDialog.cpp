#include "ui/SettingsDialog.hpp"

#include "transcription/ModelManager.hpp"
#include "util/Hotkey.hpp"
#include "util/Paths.hpp"

#include <shobjidl.h>
#include <uxtheme.h>
#include <wrl/client.h>

#include <cmath>
#include <cstdio>
#include <string>

namespace {

float ParseFloat(const CString& text, float fallback) {
    if (text.IsEmpty()) {
        return fallback;
    }
    wchar_t* end = nullptr;
    const float value = std::wcstof(text.GetString(), &end);
    if (end == text.GetString()) {
        return fallback;
    }
    return value;
}

int ParseInt(const CString& text, int fallback) {
    if (text.IsEmpty()) {
        return fallback;
    }
    wchar_t* end = nullptr;
    const long value = std::wcstol(text.GetString(), &end, 10);
    if (end == text.GetString()) {
        return fallback;
    }
    return static_cast<int>(value);
}

std::wstring FormatGain(float value) {
    wchar_t buf[32]{};
    std::swprintf(buf, 32, L"%.2f", static_cast<double>(value));
    return buf;
}

}  // namespace

SettingsDialog::SettingsDialog(DeviceManager* devices)
    : m_devices(devices) {}

BOOL SettingsDialog::OnInitDialog(CWindow /*focus*/, LPARAM /*initParam*/) {
    CenterWindow(GetParent());

    m_folder.Attach(GetDlgItem(IDC_SET_FOLDER));
    m_mic.Attach(GetDlgItem(IDC_SET_MIC_DEVICE));
    m_sys.Attach(GetDlgItem(IDC_SET_SYS_DEVICE));
    m_micGain.Attach(GetDlgItem(IDC_SET_MIC_GAIN));
    m_sysGain.Attach(GetDlgItem(IDC_SET_SYS_GAIN));
    m_micAgc.Attach(GetDlgItem(IDC_SET_MIC_AGC));
    m_saveStems.Attach(GetDlgItem(IDC_SET_SAVE_STEMS));
    m_compress.Attach(GetDlgItem(IDC_SET_COMPRESS_M4A));
    m_keepWav.Attach(GetDlgItem(IDC_SET_KEEP_WAV));
    m_comms.Attach(GetDlgItem(IDC_SET_COMMS));
    m_model.Attach(GetDlgItem(IDC_SET_MODEL));
    m_language.Attach(GetDlgItem(IDC_SET_LANGUAGE));
    m_threads.Attach(GetDlgItem(IDC_SET_THREADS));
    m_autoTranscribe.Attach(GetDlgItem(IDC_SET_AUTO_TRANSCRIBE));
    m_useVad.Attach(GetDlgItem(IDC_SET_USE_VAD));
    m_closeTray.Attach(GetDlgItem(IDC_SET_CLOSE_TRAY));
    m_startWindows.Attach(GetDlgItem(IDC_SET_START_WINDOWS));
    m_startMinimized.Attach(GetDlgItem(IDC_SET_START_MINIMIZED));
    m_hotkeyEnable.Attach(GetDlgItem(IDC_SET_HOTKEY_ENABLE));
    m_hotkey.Attach(GetDlgItem(IDC_SET_HOTKEY));

    // Same flat dropdown look as the Record view.
    for (WTL::CComboBox* combo : { &m_mic, &m_sys, &m_model, &m_language }) {
        if (combo->IsWindow()) {
            ::SetWindowTheme(combo->m_hWnd, L"CFD", nullptr);
        }
    }

    FillLanguageCombo();
    FillModelCombo();
    LoadFromSettings();
    UpdateDependentEnabled();
    return TRUE;
}

void SettingsDialog::FillLanguageCombo() {
    m_language.ResetContent();
    static const wchar_t* kLangs[] = {
        L"auto", L"en", L"de", L"fr", L"es", L"it", L"pt", L"nl", L"pl", L"ru",
        L"uk", L"zh", L"ja", L"ko", L"sv", L"no", L"da", L"fi", L"tr", L"cs"};
    for (const wchar_t* lang : kLangs) {
        m_language.AddString(lang);
    }
}

void SettingsDialog::FillModelCombo() {
    m_model.ResetContent();
    for (const auto& info : ModelManager::OfferedModels()) {
        std::wstring label = info.displayName;
        if (ModelManager::IsPresent(info.fileName)) {
            label += L" (installed)";
        }
        const int index = m_model.AddString(label.c_str());
        auto* heap = new std::wstring(info.fileName);
        m_model.SetItemDataPtr(index, heap);
    }
}

void SettingsDialog::ClearComboItemData(WTL::CComboBox& combo) {
    const int count = combo.GetCount();
    for (int i = 0; i < count; ++i) {
        auto* ptr = static_cast<std::wstring*>(combo.GetItemDataPtr(i));
        delete ptr;
        combo.SetItemDataPtr(i, nullptr);
    }
}

void SettingsDialog::FillDeviceCombo(
    WTL::CComboBox& combo,
    const std::vector<AudioDeviceInfo>& devices,
    const std::wstring& selectedId,
    bool capture) {
    ClearComboItemData(combo);
    combo.ResetContent();

    const wchar_t* defaultLabel = capture
        ? L"Default communications device"
        : L"Default output device";
    int index = combo.AddString(defaultLabel);
    combo.SetItemDataPtr(index, new std::wstring());

    for (const auto& device : devices) {
        std::wstring label = device.name;
        if (device.isDefault) {
            label += L" (default)";
        }
        index = combo.AddString(label.c_str());
        combo.SetItemDataPtr(index, new std::wstring(device.id));
    }
    SelectDeviceId(combo, selectedId);
}

void SettingsDialog::SelectDeviceId(WTL::CComboBox& combo, const std::wstring& id) {
    const int count = combo.GetCount();
    for (int i = 0; i < count; ++i) {
        const auto* stored = static_cast<const std::wstring*>(combo.GetItemDataPtr(i));
        if (stored != nullptr && *stored == id) {
            combo.SetCurSel(i);
            return;
        }
    }
    combo.SetCurSel(0);
}

std::wstring SettingsDialog::SelectedDeviceId(const WTL::CComboBox& combo) const {
    const int sel = combo.GetCurSel();
    if (sel < 0) {
        return {};
    }
    const auto* stored = static_cast<const std::wstring*>(combo.GetItemDataPtr(sel));
    return stored != nullptr ? *stored : std::wstring{};
}

void SettingsDialog::LoadFromSettings() {
    const auto& data = Settings::Instance().Data();
    m_folder.SetWindowText(Settings::Instance().RecordingsFolder().c_str());

    if (m_devices != nullptr) {
        FillDeviceCombo(m_mic, m_devices->EnumerateCapture(), data.micDeviceId, true);
        FillDeviceCombo(m_sys, m_devices->EnumerateRender(), data.systemDeviceId, false);
    }

    m_micGain.SetWindowText(FormatGain(data.micGain).c_str());
    m_sysGain.SetWindowText(FormatGain(data.systemGain).c_str());
    m_micAgc.SetCheck(data.micAgc ? BST_CHECKED : BST_UNCHECKED);
    m_saveStems.SetCheck(data.saveStems ? BST_CHECKED : BST_UNCHECKED);
    m_compress.SetCheck(data.compressToM4a ? BST_CHECKED : BST_UNCHECKED);
    m_keepWav.SetCheck(data.keepWav ? BST_CHECKED : BST_UNCHECKED);
    m_comms.SetCheck(data.communicationsProcessing ? BST_CHECKED : BST_UNCHECKED);

    int modelSel = 0;
    for (int i = 0; i < m_model.GetCount(); ++i) {
        const auto* file = static_cast<const std::wstring*>(m_model.GetItemDataPtr(i));
        if (file != nullptr && *file == data.model) {
            modelSel = i;
            break;
        }
    }
    m_model.SetCurSel(modelSel);

    int langSel = m_language.FindStringExact(0, data.language.c_str());
    if (langSel < 0) {
        langSel = 0;
    }
    m_language.SetCurSel(langSel);

    m_threads.SetWindowText(std::to_wstring(data.threads).c_str());
    m_autoTranscribe.SetCheck(data.autoTranscribe ? BST_CHECKED : BST_UNCHECKED);
    m_useVad.SetCheck(data.useVad ? BST_CHECKED : BST_UNCHECKED);

    m_closeTray.SetCheck(data.closeToTray ? BST_CHECKED : BST_UNCHECKED);
    m_startWindows.SetCheck(data.startWithWindows ? BST_CHECKED : BST_UNCHECKED);
    m_startMinimized.SetCheck(data.startMinimized ? BST_CHECKED : BST_UNCHECKED);
    m_hotkeyEnable.SetCheck(data.hotkeyEnabled ? BST_CHECKED : BST_UNCHECKED);
    m_hotkey.SetWindowText(data.hotkey.c_str());
}

void SettingsDialog::UpdateDependentEnabled() {
    const bool compress = m_compress.GetCheck() == BST_CHECKED;
    m_keepWav.EnableWindow(compress ? TRUE : FALSE);
    const bool hotkey = m_hotkeyEnable.GetCheck() == BST_CHECKED;
    m_hotkey.EnableWindow(hotkey ? TRUE : FALSE);
    const bool micAgc = m_micAgc.GetCheck() == BST_CHECKED;
    m_micGain.EnableWindow(micAgc ? FALSE : TRUE);
}

void SettingsDialog::OnCompressToggled(UINT /*code*/, int /*id*/, HWND /*control*/) {
    UpdateDependentEnabled();
}

void SettingsDialog::OnMicAgcToggled(UINT /*code*/, int /*id*/, HWND /*control*/) {
    UpdateDependentEnabled();
}

void SettingsDialog::OnHotkeyToggled(UINT /*code*/, int /*id*/, HWND /*control*/) {
    UpdateDependentEnabled();
}

bool SettingsDialog::BrowseForFolder(std::wstring& path) {
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

void SettingsDialog::OnBrowseFolder(UINT /*code*/, int /*id*/, HWND /*control*/) {
    std::wstring path;
    if (BrowseForFolder(path)) {
        m_folder.SetWindowText(path.c_str());
    }
}

void SettingsDialog::OnModelDownload(UINT /*code*/, int /*id*/, HWND /*control*/) {
    const int sel = m_model.GetCurSel();
    if (sel < 0) {
        return;
    }
    const auto* file = static_cast<const std::wstring*>(m_model.GetItemDataPtr(sel));
    if (file == nullptr || file->empty()) {
        return;
    }
    const ModelInfo* info = ModelManager::Find(*file);
    if (info == nullptr) {
        return;
    }
    std::wstring error;
    if (!ModelManager::Download(m_hWnd, *info, error)) {
        if (!error.empty()) {
            ::MessageBoxW(m_hWnd, error.c_str(), L"MeetingBuddy", MB_OK | MB_ICONWARNING);
        }
        return;
    }
    ClearComboItemData(m_model);
    FillModelCombo();
    for (int i = 0; i < m_model.GetCount(); ++i) {
        const auto* stored = static_cast<const std::wstring*>(m_model.GetItemDataPtr(i));
        if (stored != nullptr && *stored == *file) {
            m_model.SetCurSel(i);
            break;
        }
    }
}

void SettingsDialog::OnModelBrowse(UINT /*code*/, int /*id*/, HWND /*control*/) {
    std::wstring installed;
    if (!ModelManager::BrowseAndInstall(m_hWnd, installed) || installed.empty()) {
        return;
    }
    ClearComboItemData(m_model);
    FillModelCombo();
    bool found = false;
    for (int i = 0; i < m_model.GetCount(); ++i) {
        const auto* stored = static_cast<const std::wstring*>(m_model.GetItemDataPtr(i));
        if (stored != nullptr && *stored == installed) {
            m_model.SetCurSel(i);
            found = true;
            break;
        }
    }
    if (!found) {
        const int index = m_model.AddString(installed.c_str());
        m_model.SetItemDataPtr(index, new std::wstring(installed));
        m_model.SetCurSel(index);
    }
}

bool SettingsDialog::CollectTo(SettingsData& out, std::wstring& error) const {
    error.clear();
    out = Settings::Instance().Data();

    CString folderText;
    m_folder.GetWindowText(folderText);
    out.folder = folderText.GetString();
    if (out.folder.empty()) {
        error = L"Choose a recordings folder.";
        return false;
    }
    if (!Paths::EnsureDirectory(out.folder)) {
        error = L"The recordings folder could not be created.";
        return false;
    }

    out.micDeviceId = SelectedDeviceId(m_mic);
    out.systemDeviceId = SelectedDeviceId(m_sys);

    CString micGainText;
    CString sysGainText;
    m_micGain.GetWindowText(micGainText);
    m_sysGain.GetWindowText(sysGainText);
    out.micGain = ParseFloat(micGainText, 1.0f);
    out.systemGain = ParseFloat(sysGainText, 0.8f);
    out.micAgc = m_micAgc.GetCheck() == BST_CHECKED;
    if (!std::isfinite(out.micGain) || out.micGain < 0.0f || out.micGain > 4.0f
        || !std::isfinite(out.systemGain) || out.systemGain < 0.0f || out.systemGain > 4.0f) {
        error = L"Gains must be between 0 and 4.";
        return false;
    }

    out.saveStems = m_saveStems.GetCheck() == BST_CHECKED;
    out.compressToM4a = m_compress.GetCheck() == BST_CHECKED;
    out.keepWav = m_keepWav.GetCheck() == BST_CHECKED;
    out.communicationsProcessing = m_comms.GetCheck() == BST_CHECKED;

    const int modelSel = m_model.GetCurSel();
    if (modelSel >= 0) {
        const auto* file = static_cast<const std::wstring*>(m_model.GetItemDataPtr(modelSel));
        if (file != nullptr) {
            out.model = *file;
        }
    }

    CString lang;
    const int langSel = m_language.GetCurSel();
    if (langSel >= 0) {
        m_language.GetLBText(langSel, lang);
        out.language = lang.GetString();
    }

    CString threadsText;
    m_threads.GetWindowText(threadsText);
    out.threads = ParseInt(threadsText, 0);
    if (out.threads < 0 || out.threads > 64) {
        error = L"Thread count must be between 0 (auto) and 64.";
        return false;
    }

    out.autoTranscribe = m_autoTranscribe.GetCheck() == BST_CHECKED;
    out.useVad = m_useVad.GetCheck() == BST_CHECKED;
    out.closeToTray = m_closeTray.GetCheck() == BST_CHECKED;
    out.startWithWindows = m_startWindows.GetCheck() == BST_CHECKED;
    out.startMinimized = m_startMinimized.GetCheck() == BST_CHECKED;
    out.hotkeyEnabled = m_hotkeyEnable.GetCheck() == BST_CHECKED;

    CString hotkeyText;
    m_hotkey.GetWindowText(hotkeyText);
    out.hotkey = hotkeyText.GetString();
    if (out.hotkeyEnabled) {
        const HotkeyChord chord = Hotkey::Parse(out.hotkey);
        if (!chord.valid) {
            error = L"Hotkey must look like Ctrl+Alt+R.";
            return false;
        }
        out.hotkey = Hotkey::Format(chord);
    }

    return true;
}

void SettingsDialog::OnOk(UINT /*code*/, int /*id*/, HWND /*control*/) {
    SettingsData next{};
    std::wstring error;
    if (!CollectTo(next, error)) {
        ::MessageBoxW(m_hWnd, error.c_str(), L"MeetingBuddy", MB_OK | MB_ICONWARNING);
        return;
    }

    const std::wstring previousFolder = Settings::Instance().RecordingsFolder();
    Settings::Instance().Data() = next;
    if (!Settings::Instance().Save()) {
        ::MessageBoxW(m_hWnd, L"Failed to write settings.ini.", L"MeetingBuddy", MB_OK | MB_ICONERROR);
        return;
    }

    m_applied = next;
    m_folderChanged = _wcsicmp(previousFolder.c_str(), Settings::Instance().RecordingsFolder().c_str()) != 0;
    ClearComboItemData(m_mic);
    ClearComboItemData(m_sys);
    ClearComboItemData(m_model);
    EndDialog(IDOK);
}

void SettingsDialog::OnCancel(UINT /*code*/, int /*id*/, HWND /*control*/) {
    ClearComboItemData(m_mic);
    ClearComboItemData(m_sys);
    ClearComboItemData(m_model);
    EndDialog(IDCANCEL);
}
