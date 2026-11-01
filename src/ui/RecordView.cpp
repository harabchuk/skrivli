#include "ui/RecordView.hpp"

#include "storage/Settings.hpp"
#include "ui/Theme.hpp"
#include "util/Log.hpp"
#include "util/Paths.hpp"

#include <commctrl.h>
#include <shellapi.h>
#include <shobjidl.h>
#include <uxtheme.h>
#include <wrl/client.h>

#include <algorithm>
#include <cmath>
#include <cstdlib>
#include <cstdio>

namespace {

constexpr COLORREF kRecordingRed = RGB(0xC4, 0x2B, 0x1C);
constexpr COLORREF kWarningAmber = RGB(0x9A, 0x5B, 0x00);
constexpr int kMicGainSliderMin = 0;
constexpr int kMicGainSliderMax = 40; // 0.0 .. 4.0 in 0.1 steps
constexpr float kMicGainStep = 0.1f;

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

std::wstring FormatDb(float db) {
    if (db <= -59.5f) {
        return L"-inf dB";
    }
    wchar_t buffer[32]{};
    std::swprintf(buffer, 32, L"%+.0f dB", db);
    return buffer;
}

std::wstring FormatGainMultiplier(float gain) {
    wchar_t buffer[32]{};
    std::swprintf(buffer, 32, L"%.1f\u00D7", static_cast<double>(gain));
    return buffer;
}

HFONT TryCreateFont(const wchar_t* face, int heightPx, int weight) {
    LOGFONTW lf{};
    lf.lfHeight = -heightPx;
    lf.lfWeight = weight;
    lf.lfCharSet = DEFAULT_CHARSET;
    lf.lfQuality = CLEARTYPE_QUALITY;
    wcsncpy_s(lf.lfFaceName, face, _TRUNCATE);
    return ::CreateFontIndirectW(&lf);
}

}  // namespace

BOOL RecordView::OnInitDialog(CWindow /*focus*/, LPARAM /*initParam*/) {
    m_micCombo.Attach(GetDlgItem(IDC_MIC_DEVICE));
    m_systemCombo.Attach(GetDlgItem(IDC_SYSTEM_DEVICE));
    m_folderEdit.Attach(GetDlgItem(IDC_FOLDER_PATH));
    m_recordButton.SubclassWindow(GetDlgItem(IDC_RECORD_BUTTON));
    m_elapsedLabel.Attach(GetDlgItem(IDC_ELAPSED));
    m_recIndicator.Attach(GetDlgItem(IDC_REC_INDICATOR));
    m_warningLabel.Attach(GetDlgItem(IDC_DEVICE_WARNING));
    m_micDbLabel.Attach(GetDlgItem(IDC_MIC_DB));
    m_systemDbLabel.Attach(GetDlgItem(IDC_SYSTEM_DB));
    m_micGainLabel.Attach(GetDlgItem(IDC_MIC_GAIN_LABEL));
    m_micGainValue.Attach(GetDlgItem(IDC_MIC_GAIN_VALUE));
    m_micGain.Attach(GetDlgItem(IDC_MIC_GAIN));
    m_micAgc.Attach(GetDlgItem(IDC_MIC_GAIN_AUTO));
    m_inputSection.Attach(GetDlgItem(IDC_INPUT_SECTION));
    m_outputSection.Attach(GetDlgItem(IDC_OUTPUT_SECTION));

    m_micMeter.SubclassWindow(GetDlgItem(IDC_METER_MIC));
    m_systemMeter.SubclassWindow(GetDlgItem(IDC_METER_SYSTEM));

    m_micGain.SetRange(kMicGainSliderMin, kMicGainSliderMax);
    m_micGain.SetPageSize(1);
    m_micGain.SetLineSize(1);

    m_folderEdit.SetCueBannerText(L"Choose a folder for recordings");

    LARGE_INTEGER freq{};
    ::QueryPerformanceFrequency(&freq);
    m_qpcFrequency = static_cast<uint64_t>(freq.QuadPart);
    m_dpi = ::GetDpiForWindow(m_hWnd);

    InitTooltips();
    RebuildSpecialFonts(m_dpi);
    ApplyControlFonts();

    SyncControlsToSettings();
    RefreshDevices();
    UpdateRecordButton();
    SetWarning({});
    m_recIndicator.ShowWindow(SW_HIDE);
    m_micDbLabel.SetWindowText(L"-inf dB");
    m_systemDbLabel.SetWindowText(L"-inf dB");
    SetTimer(IDT_RECORD_METERS, 33);

    // Snapshot after the combo and folder rows have their final heights.
    DlgResize_Init(false, false);
    return TRUE;
}

void RecordView::OnSize(UINT type, CSize size) {
    if (type != SIZE_MINIMIZED) {
        DlgResize_UpdateLayout(size.cx, size.cy);
    }
    LayoutFieldRows();
}

void RecordView::InitTooltips() {
    if (m_tooltips.IsWindow()) {
        return;
    }
    m_tooltips.Create(m_hWnd, nullptr, nullptr, TTS_ALWAYSTIP | TTS_NOPREFIX);
    if (!m_tooltips.IsWindow()) {
        return;
    }
    m_tooltips.SetMaxTipWidth(400);
    m_tooltips.AddTool(m_micCombo.m_hWnd, L"");
    m_tooltips.AddTool(m_systemCombo.m_hWnd, L"");
    m_tooltips.AddTool(m_recordButton.m_hWnd, L"Start or stop recording");
    if (m_micGain.IsWindow()) {
        m_tooltips.AddTool(m_micGain.m_hWnd, L"Microphone mix gain (0.0\u20134.0)");
    }
    if (m_micAgc.IsWindow()) {
        m_tooltips.AddTool(
            m_micAgc.m_hWnd,
            L"Automatic gain control targets speech around -18 dBFS (0.5\u00D7\u20134.0\u00D7)");
    }
    m_tooltips.Activate(TRUE);
}

void RecordView::UpdateDeviceTooltips() {
    if (!m_tooltips.IsWindow()) {
        return;
    }
    CString micText;
    const int micSel = m_micCombo.GetCurSel();
    if (micSel >= 0) {
        m_micCombo.GetLBText(micSel, micText);
    }
    m_tooltips.UpdateTipText(micText.IsEmpty() ? L"Microphone device" : micText.GetString(), m_micCombo.m_hWnd);

    CString sysText;
    const int sysSel = m_systemCombo.GetCurSel();
    if (sysSel >= 0) {
        m_systemCombo.GetLBText(sysSel, sysText);
    }
    m_tooltips.UpdateTipText(sysText.IsEmpty() ? L"System audio device" : sysText.GetString(), m_systemCombo.m_hWnd);
}

void RecordView::RebuildSpecialFonts(UINT dpi) {
    if (dpi == 0) {
        dpi = USER_DEFAULT_SCREEN_DPI;
    }
    m_dpi = dpi;

    const int timerPx = ::MulDiv(28, static_cast<int>(dpi), 96);
    HFONT timer = TryCreateFont(L"Segoe UI Variable Display", timerPx, FW_SEMIBOLD);
    if (timer == nullptr) {
        timer = TryCreateFont(L"Segoe UI Semibold", timerPx, FW_SEMIBOLD);
    }
    if (timer == nullptr) {
        timer = TryCreateFont(L"Segoe UI", timerPx, FW_SEMIBOLD);
    }
    if (timer != nullptr) {
        const HFONT previous = m_timerFont.Detach();
        m_timerFont.Attach(timer);
        if (previous != nullptr) {
            ::DeleteObject(previous);
        }
    }

    const int sectionPx = ::MulDiv(13, static_cast<int>(dpi), 96);
    HFONT section = TryCreateFont(L"Segoe UI Semibold", sectionPx, FW_SEMIBOLD);
    if (section == nullptr) {
        section = TryCreateFont(L"Segoe UI", sectionPx, FW_SEMIBOLD);
    }
    if (section != nullptr) {
        const HFONT previous = m_sectionFont.Detach();
        m_sectionFont.Attach(section);
        if (previous != nullptr) {
            ::DeleteObject(previous);
        }
    }
}

void RecordView::ApplyControlFonts() {
    if (!IsWindow()) {
        return;
    }

    const HFONT ui = m_uiFont;
    for (HWND child = ::GetWindow(m_hWnd, GW_CHILD); child != nullptr; child = ::GetWindow(child, GW_HWNDNEXT)) {
        const int id = ::GetDlgCtrlID(child);
        if (id == IDC_ELAPSED || id == IDC_INPUT_SECTION || id == IDC_OUTPUT_SECTION) {
            continue;
        }
        if (ui != nullptr) {
            ::SendMessageW(child, WM_SETFONT, reinterpret_cast<WPARAM>(ui), FALSE);
        }
    }

    if (m_timerFont.m_hFont != nullptr && m_elapsedLabel.IsWindow()) {
        m_elapsedLabel.SetFont(m_timerFont);
    }
    if (m_sectionFont.m_hFont != nullptr) {
        if (m_inputSection.IsWindow()) {
            m_inputSection.SetFont(m_sectionFont);
        }
        if (m_outputSection.IsWindow()) {
            m_outputSection.SetFont(m_sectionFont);
        }
    }

    ApplyComboStyle(m_micCombo);
    ApplyComboStyle(m_systemCombo);
    LayoutFieldRows();

    if (m_inputSection.IsWindow()) {
        m_inputSection.Invalidate(FALSE);
    }
    if (m_outputSection.IsWindow()) {
        m_outputSection.Invalidate(FALSE);
    }
    if (m_recordButton.IsWindow()) {
        m_recordButton.Invalidate(FALSE);
    }
    if (m_micMeter.IsWindow()) {
        m_micMeter.Invalidate(FALSE);
    }
    if (m_systemMeter.IsWindow()) {
        m_systemMeter.Invalidate(FALSE);
    }
    Invalidate(FALSE);
}

void RecordView::ApplyUiFont(HFONT font) {
    m_uiFont = font;
    if (!IsWindow()) {
        return;
    }
    if (font != nullptr) {
        SetFont(font);
    }
    RebuildSpecialFonts(::GetDpiForWindow(m_hWnd));
    ApplyControlFonts();
}

void RecordView::OnDpiChanged(UINT dpi) {
    RebuildSpecialFonts(dpi);
    ApplyControlFonts();
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
    StopMeterPreview();
    if (m_micCombo.IsWindow()) {
        ClearComboItemData(m_micCombo);
    }
    if (m_systemCombo.IsWindow()) {
        ClearComboItemData(m_systemCombo);
    }
    if (m_tooltips.IsWindow()) {
        m_tooltips.DestroyWindow();
    }
    SetMsgHandled(FALSE);
}

HBRUSH RecordView::OnCtlColorDlg(CDCHandle dc, CWindow /*wnd*/) {
    dc.SetBkColor(ui::WindowColor());
    return ui::WindowBrush();
}

HBRUSH RecordView::OnCtlColorStatic(CDCHandle dc, CStatic wndStatic) {
    const int id = wndStatic.GetDlgCtrlID();
    // The read-only path field shares the white input look of the combos.
    if (id == IDC_FOLDER_PATH) {
        dc.SetBkColor(ui::WindowColor());
        dc.SetTextColor(
            wndStatic.IsWindowEnabled()
                ? ui::TextColor()
                : ui::SecondaryTextColor());
        return ui::WindowBrush();
    }

    const COLORREF backgroundColor = ui::WindowColor();
    const HBRUSH background = ui::WindowBrush();
    dc.SetBkColor(backgroundColor);
    if (id == IDC_ELAPSED) {
        dc.SetTextColor(IsRecording() ? kRecordingRed : ::GetSysColor(COLOR_WINDOWTEXT));
    } else if (id == IDC_REC_INDICATOR) {
        dc.SetTextColor(kRecordingRed);
    } else if (id == IDC_DEVICE_WARNING) {
        dc.SetTextColor(kWarningAmber);
    } else if (id == IDC_MIC_DB || id == IDC_SYSTEM_DB || id == IDC_MIC_GAIN_VALUE) {
        dc.SetTextColor(::GetSysColor(COLOR_GRAYTEXT));
    } else {
        dc.SetTextColor(::GetSysColor(COLOR_WINDOWTEXT));
    }
    return background;
}

void RecordView::OnDrawItem(int /*id*/, LPDRAWITEMSTRUCT drawItem) {
    if (drawItem == nullptr || drawItem->CtlType != ODT_STATIC) {
        SetMsgHandled(FALSE);
        return;
    }
    if (drawItem->CtlID != IDC_INPUT_SECTION && drawItem->CtlID != IDC_OUTPUT_SECTION) {
        SetMsgHandled(FALSE);
        return;
    }
    DrawSectionHeader(drawItem);
}

void RecordView::DrawSectionHeader(LPDRAWITEMSTRUCT drawItem) {
    CDCHandle dc(drawItem->hDC);
    const CRect rect(drawItem->rcItem);
    dc.FillSolidRect(&rect, ui::WindowColor());

    wchar_t text[64]{};
    ::GetWindowTextW(drawItem->hwndItem, text, 64);

    const HFONT font = m_sectionFont.m_hFont != nullptr ? m_sectionFont.m_hFont : m_uiFont;
    const HFONT previousFont = font != nullptr ? dc.SelectFont(font) : nullptr;
    dc.SetBkMode(TRANSPARENT);
    dc.SetTextColor(ui::kAccent);
    CRect textRect = rect;
    dc.DrawTextW(text, -1, &textRect, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);

    CSize textSize{};
    dc.GetTextExtent(text, static_cast<int>(wcslen(text)), &textSize);
    const int gap = ::MulDiv(8, static_cast<int>(m_dpi), 96);
    const int lineLeft = rect.left + textSize.cx + gap;
    if (lineLeft < rect.right) {
        const int lineY = rect.top + rect.Height() / 2;
        WTL::CPen pen;
        pen.CreatePen(PS_SOLID, 1, ui::kDivider);
        const HPEN previousPen = dc.SelectPen(pen);
        dc.MoveTo(lineLeft, lineY);
        dc.LineTo(rect.right, lineY);
        dc.SelectPen(previousPen);
    }

    if (previousFont != nullptr) {
        dc.SelectFont(previousFont);
    }
}

void RecordView::ApplyComboStyle(WTL::CComboBox& combo) {
    if (!combo.IsWindow()) {
        return;
    }
    ::SetWindowTheme(combo, L"CFD", nullptr);
    const int field = ::MulDiv(24, static_cast<int>(m_dpi), 96);
    const int item = ::MulDiv(22, static_cast<int>(m_dpi), 96);
    combo.SetItemHeight(-1, static_cast<UINT>(field));
    combo.SetItemHeight(0, static_cast<UINT>(item));
    ::SendMessageW(combo, CB_SETMINVISIBLE, 8, 0);
}

int RecordView::ClosedComboHeight(HWND combo) const {
    COMBOBOXINFO info{};
    info.cbSize = sizeof(info);
    if (combo == nullptr || !::GetComboBoxInfo(combo, &info)) {
        return 0;
    }
    RECT window{};
    ::GetWindowRect(combo, &window);
    POINT bottom{info.rcButton.left, info.rcButton.bottom};
    ::ClientToScreen(combo, &bottom);
    return bottom.y - window.top;
}

void RecordView::MatchControlHeight(HWND control, int height) {
    if (control == nullptr || height <= 0) {
        return;
    }
    CRect rect;
    ::GetWindowRect(control, &rect);
    ScreenToClient(&rect);
    if (rect.Height() == height) {
        return;
    }
    ::SetWindowPos(
        control,
        nullptr,
        rect.left,
        rect.top,
        rect.Width(),
        height,
        SWP_NOZORDER | SWP_NOACTIVATE);
}

void RecordView::CenterLabelOnRow(int labelId, HWND row, int rowHeight) {
    HWND label = GetDlgItem(labelId);
    if (label == nullptr || row == nullptr || rowHeight <= 0) {
        return;
    }
    CRect rowRect;
    ::GetWindowRect(row, &rowRect);
    ScreenToClient(&rowRect);
    CRect labelRect;
    ::GetWindowRect(label, &labelRect);
    ScreenToClient(&labelRect);
    const int y = rowRect.top + (rowHeight - labelRect.Height()) / 2;
    if (labelRect.top == y) {
        return;
    }
    ::SetWindowPos(label, nullptr, labelRect.left, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
}

void RecordView::LayoutFieldRows() {
    if (!m_micCombo.IsWindow() || !m_systemCombo.IsWindow()) {
        return;
    }
    const int comboHeight = ClosedComboHeight(m_micCombo);
    if (comboHeight <= 0) {
        return;
    }
    MatchControlHeight(m_folderEdit, comboHeight);
    MatchControlHeight(GetDlgItem(IDC_BROWSE_FOLDER), comboHeight);
    CenterLabelOnRow(IDC_MIC_LABEL, m_micCombo, comboHeight);
    CenterLabelOnRow(IDC_SYSTEM_LABEL, m_systemCombo, comboHeight);
    CenterLabelOnRow(IDC_FOLDER_LABEL, m_folderEdit, comboHeight);
}

void RecordView::SyncControlsToSettings() {
    const auto& settings = Settings::Instance().Data();
    const std::wstring folder = Settings::Instance().RecordingsFolder();
    m_folderEdit.SetWindowText(folder.c_str());
    SetMicGainSlider(settings.micGain);
    if (m_micAgc.IsWindow()) {
        m_micAgc.SetCheck(settings.micAgc ? BST_CHECKED : BST_UNCHECKED);
    }
    UpdateMicGainControls();
    ApplyMicGain(settings.micGain);
    ApplyMicAgc(settings.micAgc);
}

float RecordView::MicGainFromSlider() const {
    if (!m_micGain.IsWindow()) {
        return 1.0f;
    }
    const int pos = std::clamp(m_micGain.GetPos(), kMicGainSliderMin, kMicGainSliderMax);
    return static_cast<float>(pos) * kMicGainStep;
}

bool RecordView::MicAgcChecked() const {
    return m_micAgc.IsWindow() && m_micAgc.GetCheck() == BST_CHECKED;
}

void RecordView::UpdateMicGainLabel(float gain) {
    if (m_micGainValue.IsWindow()) {
        m_micGainValue.SetWindowText(FormatGainMultiplier(gain).c_str());
    }
}

void RecordView::UpdateMicGainControls() {
    const bool autoGain = MicAgcChecked();
    if (m_micGain.IsWindow()) {
        m_micGain.EnableWindow(autoGain ? FALSE : TRUE);
    }
    if (autoGain) {
        if (IsRecording() && m_session != nullptr) {
            UpdateMicGainLabel(m_session->EffectiveMicGain());
        } else if (m_micGainValue.IsWindow()) {
            m_micGainValue.SetWindowText(L"Auto");
        }
    } else {
        UpdateMicGainLabel(MicGainFromSlider());
    }
}

void RecordView::SetMicGainSlider(float gain) {
    if (!m_micGain.IsWindow()) {
        return;
    }
    const float clamped = std::clamp(gain, 0.0f, 4.0f);
    const int pos = std::clamp(
        static_cast<int>(std::lround(static_cast<double>(clamped) / static_cast<double>(kMicGainStep))),
        kMicGainSliderMin,
        kMicGainSliderMax);
    if (m_micGain.GetPos() != pos) {
        m_micGain.SetPos(pos);
    }
}

void RecordView::ApplyMicGain(float gain) {
    if (m_session != nullptr && m_session->IsActive()) {
        m_session->SetMicGain(gain);
    }
}

void RecordView::ApplyMicAgc(bool enabled) {
    if (m_session != nullptr && m_session->IsActive()) {
        m_session->SetMicAgc(enabled);
    }
}

void RecordView::PersistMicGain() {
    if (MicAgcChecked()) {
        return;
    }
    const float gain = MicGainFromSlider();
    UpdateMicGainLabel(gain);
    Settings::Instance().Data().micGain = gain;
    Settings::Instance().Save();
    ApplyMicGain(gain);
}

void RecordView::PersistMicAgc() {
    const bool enabled = MicAgcChecked();
    Settings::Instance().Data().micAgc = enabled;
    Settings::Instance().Save();
    UpdateMicGainControls();
    ApplyMicAgc(enabled);
    if (!enabled) {
        ApplyMicGain(MicGainFromSlider());
    }
}

void RecordView::OnMicAgcClicked(UINT /*code*/, int /*id*/, HWND /*control*/) {
    PersistMicAgc();
}

void RecordView::OnHScroll(UINT /*code*/, UINT /*pos*/, CScrollBar bar) {
    if (!m_micGain.IsWindow() || bar.m_hWnd != m_micGain.m_hWnd) {
        SetMsgHandled(FALSE);
        return;
    }
    PersistMicGain();
}

void RecordView::SetWarning(const std::wstring& text) {
    if (!m_warningLabel.IsWindow()) {
        return;
    }
    m_warningLabel.SetWindowText(text.c_str());
    m_warningLabel.ShowWindow(text.empty() ? SW_HIDE : SW_SHOW);
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
        StartMeterPreview();
    }
    UpdateDeviceTooltips();
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
    UpdateDeviceTooltips();
}

void RecordView::PersistFolder() {
    CString text;
    m_folderEdit.GetWindowText(text);
    Settings::Instance().Data().folder = text.GetString();
    Settings::Instance().Save();
}

void RecordView::OnMicSelChange(UINT /*code*/, int /*id*/, HWND /*control*/) {
    PersistDeviceSelection();
    if (!IsRecording()) {
        StartMeterPreview();
    }
}

void RecordView::OnSystemSelChange(UINT /*code*/, int /*id*/, HWND /*control*/) {
    PersistDeviceSelection();
    if (!IsRecording()) {
        StartMeterPreview();
    }
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

void RecordView::StartMeterPreview() {
    if (m_devices == nullptr || IsRecording()) {
        return;
    }
    const auto& settings = Settings::Instance().Data();
    m_levelMonitor.Start(
        settings.micDeviceId,
        settings.systemDeviceId,
        settings.communicationsProcessing);
}

void RecordView::StopMeterPreview() {
    m_levelMonitor.Stop();
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
        SetWarning(L"That folder is not writable. Choose another location.");
        return;
    }
    SetWarning({});
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
    m_recordButton.SetRecording(IsRecording());
    if (m_tooltips.IsWindow() && m_recordButton.IsWindow()) {
        m_tooltips.UpdateTipText(
            IsRecording() ? L"Stop recording" : L"Start recording",
            m_recordButton.m_hWnd);
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

void RecordView::ShowErrorDialog(const std::wstring& message) {
    TASKDIALOGCONFIG config{};
    config.cbSize = sizeof(config);
    config.hwndParent = m_hWnd;
    config.dwFlags = TDF_ALLOW_DIALOG_CANCELLATION | TDF_SIZE_TO_CONTENT;
    config.dwCommonButtons = TDCBF_OK_BUTTON;
    config.pszWindowTitle = L"MeetingBuddy";
    config.pszMainIcon = TD_ERROR_ICON;
    config.pszMainInstruction = L"Recording failed";
    config.pszContent = message.c_str();
    ::TaskDialogIndirect(&config, nullptr, nullptr, nullptr);
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
        SetWarning(L"The recordings folder is missing or not writable.");
        return false;
    }

    PersistFolder();
    PersistDeviceSelection();
    SetWarning({});

    // Release WASAPI clients so the recording session can open them.
    StopMeterPreview();

    const auto& settings = Settings::Instance().Data();
    const RecordingStartResult result = m_session->Start(
        folder,
        settings.micDeviceId,
        settings.systemDeviceId,
        settings.micGain,
        settings.micAgc,
        settings.systemGain,
        settings.communicationsProcessing,
        settings.saveStems);

    if (result.privacyDenied) {
        ShowPrivacyDeniedDialog();
        StartMeterPreview();
        return false;
    }
    if (!result.started) {
        if (!result.error.empty()) {
            ShowErrorDialog(result.error);
        }
        StartMeterPreview();
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
    StopMeterPreview();
    m_startedQpc = m_session != nullptr ? m_session->StartQpc() : 0;
    SetInputsEnabled(false);
    UpdateRecordButton();
    m_recBlinkTicks = 0;
    m_recBlinkOn = true;
    UpdateRecIndicator();
    SetTimer(IDT_RECORD_ELAPSED, 250);
    UpdateElapsed();
    m_elapsedLabel.Invalidate(FALSE);
}

void RecordView::OnRecordingFinished(const std::wstring& /*path*/) {
    KillTimer(IDT_RECORD_ELAPSED);
    SetInputsEnabled(true);
    UpdateRecordButton();
    m_elapsedLabel.SetWindowText(L"00:00:00");
    m_elapsedLabel.Invalidate(FALSE);
    m_recBlinkOn = false;
    m_recIndicator.ShowWindow(SW_HIDE);
    RefreshDevices();
}

void RecordView::OnRecordingError(const std::wstring& message) {
    KillTimer(IDT_RECORD_ELAPSED);
    SetInputsEnabled(true);
    UpdateRecordButton();
    m_recBlinkOn = false;
    m_recIndicator.ShowWindow(SW_HIDE);
    m_elapsedLabel.Invalidate(FALSE);
    StartMeterPreview();
    if (!message.empty()) {
        ShowErrorDialog(message);
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

void RecordView::UpdateRecIndicator() {
    if (!m_recIndicator.IsWindow()) {
        return;
    }
    if (!IsRecording()) {
        m_recIndicator.ShowWindow(SW_HIDE);
        return;
    }
    m_recIndicator.SetWindowText(m_recBlinkOn ? L"\x25CF REC" : L"");
    m_recIndicator.ShowWindow(SW_SHOW);
}

void RecordView::UpdateMeters() {
    float micPeak = 0.0f;
    float sysPeak = 0.0f;
    if (IsRecording() && m_session != nullptr) {
        micPeak = m_session->PeakMic();
        sysPeak = m_session->PeakSys();
    } else {
        micPeak = m_levelMonitor.PeakMic();
        sysPeak = m_levelMonitor.PeakSys();
    }
    // Show post-gain mic level so the gain controls under the meter are meaningful.
    const auto& settings = Settings::Instance().Data();
    float micGain = settings.micGain;
    if (settings.micAgc) {
        micGain = (IsRecording() && m_session != nullptr) ? m_session->EffectiveMicGain() : 1.0f;
        if (IsRecording()) {
            UpdateMicGainLabel(micGain);
        }
    }
    m_micMeter.SetLevel(micPeak * micGain);
    m_systemMeter.SetLevel(sysPeak);
    if (m_micDbLabel.IsWindow()) {
        m_micDbLabel.SetWindowText(FormatDb(m_micMeter.DisplayDb()).c_str());
    }
    if (m_systemDbLabel.IsWindow()) {
        m_systemDbLabel.SetWindowText(FormatDb(m_systemMeter.DisplayDb()).c_str());
    }
}

void RecordView::OnTimer(UINT_PTR id) {
    if (id == IDT_RECORD_ELAPSED) {
        UpdateElapsed();
        ++m_recBlinkTicks;
        if (m_recBlinkTicks >= 2) {
            m_recBlinkTicks = 0;
            m_recBlinkOn = !m_recBlinkOn;
            UpdateRecIndicator();
        }
    } else if (id == IDT_RECORD_METERS) {
        UpdateMeters();
    }
}
