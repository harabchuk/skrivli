#include "ui/RecordingsView.hpp"

#include "storage/Settings.hpp"
#include "transcription/TranscriptionService.hpp"
#include "ui/Theme.hpp"
#include "ui/TranscriptWindow.hpp"
#include "util/Log.hpp"
#include "util/Paths.hpp"
#include "util/Text.hpp"

#include <shellapi.h>

#include <algorithm>
#include <cmath>
#include <cstdio>
#include <filesystem>

namespace {

class RenameDialog : public CDialogImpl<RenameDialog> {
public:
    enum { IDD = IDD_RENAME_DIALOG };

    explicit RenameDialog(std::wstring& name)
        : m_name(name) {}

    BEGIN_MSG_MAP_EX(RenameDialog)
        MSG_WM_INITDIALOG(OnInitDialog)
        COMMAND_ID_HANDLER_EX(IDOK, OnOk)
        COMMAND_ID_HANDLER_EX(IDCANCEL, OnCancel)
    END_MSG_MAP()

private:
    BOOL OnInitDialog(CWindow /*focus*/, LPARAM /*initParam*/) {
        CenterWindow(GetParent());
        SetDlgItemText(IDC_RENAME_EDIT, m_name.c_str());
        const HWND edit = GetDlgItem(IDC_RENAME_EDIT);
        if (edit != nullptr) {
            ::SetFocus(edit);
            ::SendMessageW(edit, EM_SETSEL, 0, -1);
        }
        return FALSE;
    }

    void OnOk(UINT /*code*/, int /*id*/, HWND /*control*/) {
        CString text;
        GetDlgItemText(IDC_RENAME_EDIT, text);
        m_name = text.GetString();
        EndDialog(IDOK);
    }

    void OnCancel(UINT /*code*/, int /*id*/, HWND /*control*/) {
        EndDialog(IDCANCEL);
    }

    std::wstring& m_name;
};

}  // namespace

BOOL RecordingsView::OnInitDialog(CWindow /*focus*/, LPARAM /*initParam*/) {
    DlgResize_Init(false, false);

    m_list.Attach(GetDlgItem(IDC_RECORDINGS_LIST));
    m_list.SetExtendedListViewStyle(
        LVS_EX_FULLROWSELECT | LVS_EX_LABELTIP | LVS_EX_INFOTIP | LVS_EX_DOUBLEBUFFER);

    m_list.InsertColumn(ColName, L"Name", LVCFMT_LEFT, 180);
    m_list.InsertColumn(ColDate, L"Date", LVCFMT_LEFT, 130);
    m_list.InsertColumn(ColDuration, L"Duration", LVCFMT_RIGHT, 70);
    m_list.InsertColumn(ColSize, L"Size", LVCFMT_RIGHT, 70);
    m_list.InsertColumn(ColTranscript, L"Transcript", LVCFMT_LEFT, 80);

    // Same white page color as the Record view, list included.
    m_list.SetBkColor(ui::WindowColor());
    m_list.SetTextBkColor(ui::WindowColor());

    ::EnableWindow(GetDlgItem(IDC_REC_VIEW), FALSE);

    EnsureWatching();
    Refresh();
    return TRUE;
}

void RecordingsView::OnSize(UINT type, CSize size) {
    if (type != SIZE_MINIMIZED) {
        DlgResize_UpdateLayout(size.cx, size.cy);
        FitColumns();
    }
}

HBRUSH RecordingsView::OnCtlColorDlg(CDCHandle dc, CWindow /*wnd*/) {
    // Match the Record page: pure window white instead of the dialog default.
    dc.SetBkColor(ui::WindowColor());
    return ui::WindowBrush();
}

HBRUSH RecordingsView::OnCtlColorStatic(CDCHandle dc, CStatic /*wndStatic*/) {
    dc.SetBkColor(ui::WindowColor());
    dc.SetTextColor(ui::TextColor());
    return ui::WindowBrush();
}

void RecordingsView::SetTranscriptProgress(const std::wstring& audioPath, int percent) {
    m_progressPath = audioPath;
    m_progressPercent = percent;
    for (size_t i = 0; i < m_entries.size(); ++i) {
        if (!Text::PathEquals(m_entries[i].audioPath, audioPath)) {
            continue;
        }
        m_entries[i].transcriptStatus = TranscriptStatus::Transcribing;
        m_entries[i].transcriptPercent = percent;
        for (int row = 0; row < m_list.GetItemCount(); ++row) {
            if (static_cast<size_t>(m_list.GetItemData(row)) == i) {
                m_list.SetItemText(row, ColTranscript, TranscriptLabel(m_entries[i]).c_str());
                break;
            }
        }
        break;
    }
}

void RecordingsView::OnDestroy() {
    m_watcher.Stop();
    if (m_list.IsWindow()) {
        m_list.Detach();
    }
    SetMsgHandled(FALSE);
}

void RecordingsView::EnsureWatching() {
    const std::wstring folder = Settings::Instance().RecordingsFolder();
    Paths::EnsureDirectory(folder);
    if (m_watcher.Folder() != folder) {
        m_watcher.Start(folder, m_hWnd, WM_APP_LIBRARY_CHANGED);
    }
}

void RecordingsView::ApplyUiFont(HFONT font) {
    if (font == nullptr || !IsWindow()) {
        return;
    }
    SetFont(font);
    for (HWND child = ::GetWindow(m_hWnd, GW_CHILD); child != nullptr; child = ::GetWindow(child, GW_HWNDNEXT)) {
        ::SendMessageW(child, WM_SETFONT, reinterpret_cast<WPARAM>(font), TRUE);
    }
    if (m_list.IsWindow()) {
        m_list.SetFont(font);
    }
}

void RecordingsView::Refresh() {
    EnsureWatching();
    m_entries = RecordingLibrary::Scan(Settings::Instance().RecordingsFolder());
    if (!m_progressPath.empty()) {
        bool stillActive = false;
        for (auto& entry : m_entries) {
            if (!Text::PathEquals(entry.audioPath, m_progressPath)) {
                continue;
            }
            if (entry.transcriptStatus == TranscriptStatus::Transcribing
                || entry.transcriptStatus == TranscriptStatus::Preparing
                || entry.transcriptStatus == TranscriptStatus::Queued) {
                entry.transcriptPercent = m_progressPercent;
                stillActive = true;
            }
        }
        if (!stillActive) {
            m_progressPath.clear();
            m_progressPercent = -1;
        }
    }
    ApplySort();
    PopulateList();
    UpdateActionEnabled();
}

void RecordingsView::PopulateList() {
    m_list.SetRedraw(FALSE);
    m_list.DeleteAllItems();
    for (size_t i = 0; i < m_entries.size(); ++i) {
        const auto& entry = m_entries[i];
        const int row = m_list.InsertItem(static_cast<int>(i), entry.displayName.c_str());
        m_list.SetItemText(row, ColDate, FormatDate(entry.startLocal).c_str());
        m_list.SetItemText(row, ColDuration, FormatDuration(entry.durationSeconds).c_str());
        m_list.SetItemText(row, ColSize, FormatSize(entry.sizeBytes).c_str());
        m_list.SetItemText(row, ColTranscript, TranscriptLabel(entry).c_str());
        m_list.SetItemData(row, static_cast<DWORD_PTR>(i));
    }
    m_list.SetRedraw(TRUE);
    m_list.Invalidate();
    FitColumns();
}

void RecordingsView::FitColumns() {
    if (!m_list.IsWindow()) {
        return;
    }
    const HWND header = m_list.GetHeader();
    if (header == nullptr || Header_GetItemCount(header) < 5) {
        return;
    }

    static constexpr int kWeights[] = {180, 130, 70, 70, 80};
    static constexpr int kColumnCount = 5;
    static constexpr int kWeightSum = 180 + 130 + 70 + 70 + 80;

    // A second pass picks up the vertical scrollbar appearing or disappearing.
    for (int pass = 0; pass < 2; ++pass) {
        RECT client{};
        m_list.GetClientRect(&client);
        const int width = client.right - client.left;
        if (width <= 0) {
            return;
        }

        int used = 0;
        for (int column = 0; column < kColumnCount; ++column) {
            const int columnWidth = (column == kColumnCount - 1)
                ? (width - used)
                : (width * kWeights[column] / kWeightSum);
            if (m_list.GetColumnWidth(column) != columnWidth) {
                m_list.SetColumnWidth(column, columnWidth);
            }
            used += columnWidth;
        }
    }
}

void RecordingsView::UpdateActionEnabled() {
    const bool hasSel = SelectedIndex() >= 0;
    const RecordingEntry* entry = SelectedEntry();
    ::EnableWindow(GetDlgItem(IDC_REC_PLAY), hasSel ? TRUE : FALSE);
    ::EnableWindow(GetDlgItem(IDC_REC_OPENFOLDER), hasSel ? TRUE : FALSE);
    ::EnableWindow(GetDlgItem(IDC_REC_RENAME), hasSel ? TRUE : FALSE);
    ::EnableWindow(GetDlgItem(IDC_REC_DELETE), hasSel ? TRUE : FALSE);
    ::EnableWindow(GetDlgItem(IDC_REC_TRANSCRIBE), hasSel ? TRUE : FALSE);
    const bool canView = entry != nullptr && entry->hasMarkdown;
    ::EnableWindow(GetDlgItem(IDC_REC_VIEW), canView ? TRUE : FALSE);
}

int RecordingsView::SelectedIndex() const {
    return m_list.GetSelectedIndex();
}

const RecordingEntry* RecordingsView::SelectedEntry() const {
    const int sel = SelectedIndex();
    if (sel < 0) {
        return nullptr;
    }
    const size_t index = static_cast<size_t>(m_list.GetItemData(sel));
    if (index >= m_entries.size()) {
        return nullptr;
    }
    return &m_entries[index];
}

std::wstring RecordingsView::FormatDuration(double seconds) {
    if (seconds < 0.0) {
        seconds = 0.0;
    }
    const int total = static_cast<int>(std::llround(seconds));
    const int h = total / 3600;
    const int m = (total / 60) % 60;
    const int s = total % 60;
    wchar_t buf[32]{};
    if (h > 0) {
        std::swprintf(buf, 32, L"%d:%02d:%02d", h, m, s);
    } else {
        std::swprintf(buf, 32, L"%d:%02d", m, s);
    }
    return buf;
}

std::wstring RecordingsView::FormatSize(uint64_t bytes) {
    wchar_t buf[64]{};
    if (bytes < 1024) {
        std::swprintf(buf, 64, L"%llu B", static_cast<unsigned long long>(bytes));
    } else if (bytes < 1024ull * 1024ull) {
        std::swprintf(buf, 64, L"%.1f KB", static_cast<double>(bytes) / 1024.0);
    } else {
        std::swprintf(buf, 64, L"%.1f MB", static_cast<double>(bytes) / (1024.0 * 1024.0));
    }
    return buf;
}

std::wstring RecordingsView::FormatDate(const std::wstring& startLocal) {
    return Text::IsoToDisplayMinute(startLocal);
}

std::wstring RecordingsView::TranscriptLabel(const RecordingEntry& entry) {
    switch (entry.transcriptStatus) {
    case TranscriptStatus::Queued:
        return L"Queued";
    case TranscriptStatus::Preparing:
        return L"Preparing";
    case TranscriptStatus::Transcribing: {
        if (entry.transcriptPercent >= 0) {
            wchar_t buf[32]{};
            std::swprintf(buf, 32, L"%d %%", entry.transcriptPercent);
            return buf;
        }
        return L"Transcribing";
    }
    case TranscriptStatus::Ready:
        return L"Ready";
    case TranscriptStatus::Failed:
        return L"Failed";
    case TranscriptStatus::Cancelled:
        return L"Cancelled";
    case TranscriptStatus::None:
    default:
        if (entry.hasMarkdown) {
            return L"Ready";
        }
        return L"—";
    }
}

void RecordingsView::ToggleSort(Column column) {
    if (m_sortColumn == column) {
        m_sortAscending = !m_sortAscending;
    } else {
        m_sortColumn = column;
        m_sortAscending = (column == ColName || column == ColTranscript);
    }
    ApplySort();
}

void RecordingsView::ApplySort() {
    std::sort(m_entries.begin(), m_entries.end(), [this](const RecordingEntry& a, const RecordingEntry& b) {
        int cmp = 0;
        switch (m_sortColumn) {
        case ColName: cmp = _wcsicmp(a.displayName.c_str(), b.displayName.c_str()); break;
        case ColDate: cmp = a.startLocal.compare(b.startLocal); break;
        case ColDuration:
            cmp = (a.durationSeconds < b.durationSeconds) ? -1 : (a.durationSeconds > b.durationSeconds ? 1 : 0);
            break;
        case ColSize: cmp = (a.sizeBytes < b.sizeBytes) ? -1 : (a.sizeBytes > b.sizeBytes ? 1 : 0); break;
        case ColTranscript: cmp = TranscriptLabel(a).compare(TranscriptLabel(b)); break;
        }
        return m_sortAscending ? (cmp < 0) : (cmp > 0);
    });
}

bool RecordingsView::PromptRename(std::wstring& stemName) {
    RenameDialog dlg(stemName);
    return dlg.DoModal(m_hWnd) == IDOK && !stemName.empty();
}

void RecordingsView::OnPlay(UINT /*code*/, int /*id*/, HWND /*control*/) {
    const RecordingEntry* entry = SelectedEntry();
    if (entry == nullptr) {
        return;
    }
    ::ShellExecuteW(m_hWnd, L"open", entry->audioPath.c_str(), nullptr, nullptr, SW_SHOWNORMAL);
}

void RecordingsView::OnTranscribe(UINT /*code*/, int /*id*/, HWND /*control*/) {
    const RecordingEntry* entry = SelectedEntry();
    if (entry == nullptr) {
        return;
    }
    auto& service = TranscriptionService::Instance();
    if (service.IsQueuedOrActive(entry->audioPath)) {
        service.Cancel(entry->audioPath);
        return;
    }
    if (!service.Enqueue(GetParent(), entry->audioPath)) {
        return;
    }
    Refresh();
}

void RecordingsView::OnView(UINT /*code*/, int /*id*/, HWND /*control*/) {
    const RecordingEntry* entry = SelectedEntry();
    if (entry == nullptr || !entry->hasMarkdown) {
        return;
    }
    TranscriptWindow::OpenOrActivate(entry->mdPath);
}

void RecordingsView::OnOpenFolder(UINT /*code*/, int /*id*/, HWND /*control*/) {
    const RecordingEntry* entry = SelectedEntry();
    if (entry == nullptr) {
        return;
    }
    const std::wstring params = L"/select,\"" + entry->audioPath + L"\"";
    ::ShellExecuteW(m_hWnd, L"open", L"explorer.exe", params.c_str(), nullptr, SW_SHOWNORMAL);
}

void RecordingsView::OnRename(UINT /*code*/, int /*id*/, HWND /*control*/) {
    const RecordingEntry* entry = SelectedEntry();
    if (entry == nullptr) {
        return;
    }
    std::wstring stem = std::filesystem::path(entry->stem).filename().wstring();
    if (!PromptRename(stem)) {
        return;
    }
    std::wstring error;
    if (!RecordingLibrary::Rename(*entry, stem, error)) {
        ui::ShowMessage(m_hWnd, error.c_str(), MB_OK | MB_ICONWARNING);
        return;
    }
    Refresh();
}

void RecordingsView::OnDelete(UINT /*code*/, int /*id*/, HWND /*control*/) {
    const RecordingEntry* entry = SelectedEntry();
    if (entry == nullptr) {
        return;
    }
    const int answer = ui::ShowMessage(
        m_hWnd,
        L"Move this recording and its sidecars to the Recycle Bin?",
        MB_YESNO | MB_ICONQUESTION | MB_DEFBUTTON2);
    if (answer != IDYES) {
        return;
    }
    std::wstring error;
    if (!RecordingLibrary::DeleteToRecycleBin(*entry, error)) {
        ui::ShowMessage(m_hWnd, error.c_str(), MB_OK | MB_ICONWARNING);
        return;
    }
    Refresh();
}

void RecordingsView::OnRefresh(UINT /*code*/, int /*id*/, HWND /*control*/) {
    Refresh();
}

LRESULT RecordingsView::OnColumnClick(LPNMHDR header) {
    const auto* listHeader = reinterpret_cast<LPNMLISTVIEW>(header);
    if (listHeader != nullptr) {
        ToggleSort(static_cast<Column>(listHeader->iSubItem));
        PopulateList();
    }
    return 0;
}

LRESULT RecordingsView::OnDoubleClick(LPNMHDR /*header*/) {
    const RecordingEntry* entry = SelectedEntry();
    if (entry != nullptr && entry->hasMarkdown) {
        OnView(0, 0, nullptr);
    } else {
        OnPlay(0, 0, nullptr);
    }
    return 0;
}

LRESULT RecordingsView::OnGetInfoTip(LPNMHDR header) {
    auto* tip = reinterpret_cast<NMLVGETINFOTIPW*>(header);
    if (tip == nullptr || tip->iItem < 0) {
        return 0;
    }
    const size_t index = static_cast<size_t>(m_list.GetItemData(tip->iItem));
    if (index >= m_entries.size()) {
        return 0;
    }
    const auto& entry = m_entries[index];
    if (entry.transcriptStatus == TranscriptStatus::Failed && !entry.transcriptError.empty()) {
        wcsncpy_s(tip->pszText, tip->cchTextMax, entry.transcriptError.c_str(), _TRUNCATE);
    }
    return 0;
}

LRESULT RecordingsView::OnRightClick(LPNMHDR /*header*/) {
    CMenu menu;
    menu.LoadMenu(IDR_RECORDINGS_MENU);
    CMenuHandle popup = menu.GetSubMenu(0);
    if (popup.m_hMenu == nullptr) {
        return 0;
    }

    const RecordingEntry* entry = SelectedEntry();
    popup.EnableMenuItem(IDC_REC_PLAY, MF_BYCOMMAND | (entry ? MF_ENABLED : MF_GRAYED));
    popup.EnableMenuItem(IDC_REC_TRANSCRIBE, MF_BYCOMMAND | (entry ? MF_ENABLED : MF_GRAYED));
    popup.EnableMenuItem(
        IDC_REC_VIEW,
        MF_BYCOMMAND | ((entry && entry->hasMarkdown) ? MF_ENABLED : MF_GRAYED));
    popup.EnableMenuItem(IDC_REC_OPENFOLDER, MF_BYCOMMAND | (entry ? MF_ENABLED : MF_GRAYED));
    popup.EnableMenuItem(IDC_REC_RENAME, MF_BYCOMMAND | (entry ? MF_ENABLED : MF_GRAYED));
    popup.EnableMenuItem(IDC_REC_DELETE, MF_BYCOMMAND | (entry ? MF_ENABLED : MF_GRAYED));

    POINT cursor{};
    ::GetCursorPos(&cursor);
    popup.TrackPopupMenu(TPM_RIGHTBUTTON | TPM_LEFTALIGN, cursor.x, cursor.y, m_hWnd);
    return 0;
}

LRESULT RecordingsView::OnItemChanged(LPNMHDR /*header*/) {
    UpdateActionEnabled();
    return 0;
}

LRESULT RecordingsView::OnLibraryChanged(UINT /*msg*/, WPARAM /*wParam*/, LPARAM /*lParam*/, BOOL& handled) {
    handled = TRUE;
    Refresh();
    return 0;
}
