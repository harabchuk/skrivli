#include "ui/SettingsDialog.hpp"

#include "transcription/ModelManager.hpp"
#include "ui/ShellDialogs.hpp"
#include "ui/Theme.hpp"
#include "util/Hotkey.hpp"
#include "util/Paths.hpp"
#include "util/Text.hpp"

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
    for (WTL::CComboBox* combo : { &m_mic.Combo(), &m_sys.Combo(), &m_model.Combo(), &m_language }) {
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
    std::vector<DeviceCombo::Item> items;
    for (const auto& info : ModelManager::OfferedModels()) {
        std::wstring label = info.displayName;
        if (ModelManager::IsPresent(info.fileName)) {
            label += L" (installed)";
        }
        items.push_back({std::move(label), info.fileName});
    }
    m_model.Fill(items);
}

void SettingsDialog::FillDeviceCombo(
    DeviceCombo& combo,
    const std::vector<AudioDeviceInfo>& devices,
    const std::wstring& selectedId,
    bool capture) {
    std::vector<DeviceCombo::Item> items;
    items.push_back({capture ? L"Default communications device" : L"Default output device", {}});
    for (const auto& device : devices) {
        std::wstring label = device.name;
        if (device.isDefault) {
            label += L" (default)";
        }
        items.push_back({std::move(label), device.id});
    }
    combo.Fill(items);
    combo.Select(selectedId);
}

std::array<SettingsDialog::CheckboxBinding, SettingsDialog::kCheckboxCount> SettingsDialog::Checkboxes() {
    return {{
        {&m_micAgc, &SettingsData::micAgc},
        {&m_saveStems, &SettingsData::saveStems},
        {&m_compress, &SettingsData::compressToM4a},
        {&m_keepWav, &SettingsData::keepWav},
        {&m_comms, &SettingsData::communicationsProcessing},
        {&m_autoTranscribe, &SettingsData::autoTranscribe},
        {&m_useVad, &SettingsData::useVad},
        {&m_closeTray, &SettingsData::closeToTray},
        {&m_startWindows, &SettingsData::startWithWindows},
        {&m_startMinimized, &SettingsData::startMinimized},
        {&m_hotkeyEnable, &SettingsData::hotkeyEnabled},
    }};
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
    for (const auto& [button, field] : Checkboxes()) {
        button->SetCheck(data.*field ? BST_CHECKED : BST_UNCHECKED);
    }

    m_model.Select(data.model);

    int langSel = m_language.FindStringExact(0, data.language.c_str());
    if (langSel < 0) {
        langSel = 0;
    }
    m_language.SetCurSel(langSel);

    m_threads.SetWindowText(std::to_wstring(data.threads).c_str());
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

void SettingsDialog::OnDependentToggled(UINT /*code*/, int /*id*/, HWND /*control*/) {
    UpdateDependentEnabled();
}

void SettingsDialog::OnBrowseFolder(UINT /*code*/, int /*id*/, HWND /*control*/) {
    if (const auto path = ui::PickFolder(m_hWnd)) {
        m_folder.SetWindowText(path->c_str());
    }
}

void SettingsDialog::OnModelDownload(UINT /*code*/, int /*id*/, HWND /*control*/) {
    const std::wstring file = m_model.SelectedId();
    if (file.empty()) {
        return;
    }
    const ModelInfo* info = ModelManager::Find(file);
    if (info == nullptr) {
        return;
    }
    std::wstring error;
    if (!ModelManager::Download(m_hWnd, *info, error)) {
        if (!error.empty()) {
            ui::ShowMessage(m_hWnd, error.c_str(), MB_OK | MB_ICONWARNING);
        }
        return;
    }
    FillModelCombo();
    m_model.Select(file);
}

void SettingsDialog::OnModelBrowse(UINT /*code*/, int /*id*/, HWND /*control*/) {
    std::wstring installed;
    if (!ModelManager::BrowseAndInstall(m_hWnd, installed) || installed.empty()) {
        return;
    }
    FillModelCombo();
    if (!m_model.Select(installed)) {
        m_model.Combo().SetCurSel(m_model.Add(installed, installed));
    }
}

bool SettingsDialog::CollectTo(SettingsData& out, std::wstring& error) {
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

    out.micDeviceId = m_mic.SelectedId();
    out.systemDeviceId = m_sys.SelectedId();

    for (const auto& [button, field] : Checkboxes()) {
        out.*field = button->GetCheck() == BST_CHECKED;
    }

    CString micGainText;
    CString sysGainText;
    m_micGain.GetWindowText(micGainText);
    m_sysGain.GetWindowText(sysGainText);
    out.micGain = ParseFloat(micGainText, 1.0f);
    out.systemGain = ParseFloat(sysGainText, 0.8f);
    if (!std::isfinite(out.micGain) || out.micGain < 0.0f || out.micGain > 4.0f
        || !std::isfinite(out.systemGain) || out.systemGain < 0.0f || out.systemGain > 4.0f) {
        error = L"Gains must be between 0 and 4.";
        return false;
    }

    const std::wstring modelFile = m_model.SelectedId();
    if (!modelFile.empty()) {
        out.model = modelFile;
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
        ui::ShowMessage(m_hWnd, error.c_str(), MB_OK | MB_ICONWARNING);
        return;
    }

    const std::wstring previousFolder = Settings::Instance().RecordingsFolder();
    Settings::Instance().Data() = next;
    if (!Settings::Instance().Save()) {
        ui::ShowMessage(m_hWnd, L"Failed to save settings.", MB_OK | MB_ICONERROR);
        return;
    }

    m_folderChanged = !Text::PathEquals(previousFolder, Settings::Instance().RecordingsFolder());
    EndDialog(IDOK);
}

void SettingsDialog::OnCancel(UINT /*code*/, int /*id*/, HWND /*control*/) {
    EndDialog(IDCANCEL);
}
