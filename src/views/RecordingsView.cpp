#include "views/RecordingsView.hpp"

#include "core/TranscriptText.hpp"
#include "infrastructure/common/FileIo.hpp"
#include "infrastructure/common/Log.hpp"
#include "infrastructure/common/Paths.hpp"
#include "infrastructure/common/Text.hpp"
#include "infrastructure/storage/Settings.hpp"
#include "services/TranscriptionService.hpp"
#include "ui/Theme.hpp"
#include "views/ModelPrompt.hpp"

#include <algorithm>
#include <cmath>
#include <cstdio>
#include <filesystem>
#include <shellapi.h>

namespace {

class RenameDialog : public CDialogImpl<RenameDialog> {
  public:
    enum {
        IDD = IDD_RENAME_DIALOG
    };

    explicit RenameDialog(std::wstring& name) : m_name(name) {}

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

constexpr int kMenuActionIds[] = {
    IDC_REC_PLAY, IDC_REC_TRANSCRIBE, IDC_REC_OPENFOLDER, IDC_REC_RENAME, IDC_REC_DELETE,
};

constexpr int kPaneGap = 8;
constexpr int kListWeight = 45;
constexpr int kDetailWeight = 55;
constexpr int kButtonHeight = 24;
constexpr int kButtonWidth = 56;
constexpr int kCreateWidth = 130;

auto DpiScale(HWND hwnd, int value) -> int {
    const UINT dpi = ::GetDpiForWindow(hwnd);
    return ::MulDiv(value, static_cast<int>(dpi), 96);
}

} // namespace

BOOL RecordingsView::OnInitDialog(CWindow /*focus*/, LPARAM /*initParam*/) {
    m_list.Attach(GetDlgItem(IDC_RECORDINGS_LIST));
    m_list.SetExtendedListViewStyle(LVS_EX_FULLROWSELECT | LVS_EX_LABELTIP | LVS_EX_INFOTIP |
                                    LVS_EX_DOUBLEBUFFER);

    m_list.InsertColumn(ColName, L"Name", LVCFMT_LEFT, 180);
    m_list.InsertColumn(ColDate, L"Date", LVCFMT_LEFT, 130);
    m_list.InsertColumn(ColDuration, L"Duration", LVCFMT_RIGHT, 70);
    m_list.InsertColumn(ColSize, L"Size", LVCFMT_RIGHT, 70);
    m_list.InsertColumn(ColTranscript, L"Transcript", LVCFMT_LEFT, 80);

    m_list.SetBkColor(ui::WindowColor());
    m_list.SetTextBkColor(ui::WindowColor());

    m_emptyLabel.Attach(GetDlgItem(IDC_REC_DETAIL_EMPTY));
    m_statusLabel.Attach(GetDlgItem(IDC_REC_DETAIL_STATUS));
    m_playButton.Attach(GetDlgItem(IDC_REC_DETAIL_PLAY));
    m_stopButton.Attach(GetDlgItem(IDC_REC_DETAIL_STOP));
    m_createButton.Attach(GetDlgItem(IDC_REC_DETAIL_CREATE));
    m_transcriptEdit.Attach(GetDlgItem(IDC_REC_DETAIL_TRANSCRIPT));

    if (!m_player.Initialize(m_hWnd, WM_APP_PLAYBACK)) {
        Log::Warn(L"Inline audio player failed to initialize");
    }

    EnsureWatching();
    Refresh();
    return TRUE;
}

void RecordingsView::OnSize(UINT type, CSize size) {
    if (type != SIZE_MINIMIZED && size.cx > 0 && size.cy > 0) {
        LayoutPanes(size.cx, size.cy);
        FitColumns();
    }
}

HBRUSH RecordingsView::OnCtlColorDlg(CDCHandle dc, CWindow /*wnd*/) {
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
    if (!m_selectedAudioPath.empty() && Text::PathEquals(m_selectedAudioPath, audioPath)) {
        UpdateDetailPane();
    }
}

void RecordingsView::OnDestroy() {
    StopPlayback();
    m_player.Shutdown();
    m_watcher.Stop();
    if (m_list.IsWindow()) {
        m_list.Detach();
    }
    if (m_transcriptEdit.IsWindow()) {
        m_transcriptEdit.Detach();
    }
    if (m_emptyLabel.IsWindow()) {
        m_emptyLabel.Detach();
    }
    if (m_statusLabel.IsWindow()) {
        m_statusLabel.Detach();
    }
    if (m_playButton.IsWindow()) {
        m_playButton.Detach();
    }
    if (m_stopButton.IsWindow()) {
        m_stopButton.Detach();
    }
    if (m_createButton.IsWindow()) {
        m_createButton.Detach();
    }
    SetMsgHandled(FALSE);
}

void RecordingsView::EnsureWatching() {
    const auto folder = Settings::Instance().RecordingsFolder();
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
    for (auto child = ::GetWindow(m_hWnd, GW_CHILD); child != nullptr;
         child = ::GetWindow(child, GW_HWNDNEXT)) {
        ::SendMessageW(child, WM_SETFONT, reinterpret_cast<WPARAM>(font), TRUE);
    }
    if (m_list.IsWindow()) {
        m_list.SetFont(font);
    }
}

void RecordingsView::Refresh() {
    EnsureWatching();
    const int topIndex = m_list.IsWindow() ? m_list.GetTopIndex() : 0;
    const auto keepPath = m_selectedAudioPath;
    m_entries = RecordingLibrary::Scan(Settings::Instance().RecordingsFolder());
    if (!m_progressPath.empty()) {
        bool stillActive = false;
        for (auto& entry : m_entries) {
            if (!Text::PathEquals(entry.audioPath, m_progressPath)) {
                continue;
            }
            if (entry.transcriptStatus == TranscriptStatus::Transcribing ||
                entry.transcriptStatus == TranscriptStatus::Preparing ||
                entry.transcriptStatus == TranscriptStatus::Queued) {
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
    if (!keepPath.empty()) {
        // Keep the user's scroll position; EnsureVisible would yank back to the selection.
        SelectByAudioPath(keepPath, false);
    } else {
        UpdateDetailPane();
    }
    RestoreTopIndex(topIndex);
}

bool RecordingsView::SelectByAudioPath(const std::wstring& audioPath, bool ensureVisible) {
    if (audioPath.empty() || !m_list.IsWindow()) {
        return false;
    }
    for (int row = 0; row < m_list.GetItemCount(); ++row) {
        const auto index = static_cast<size_t>(m_list.GetItemData(row));
        if (index >= m_entries.size()) {
            continue;
        }
        if (!Text::PathEquals(m_entries[index].audioPath, audioPath)) {
            continue;
        }
        m_list.SetItemState(-1, 0, LVIS_SELECTED | LVIS_FOCUSED);
        m_list.SetItemState(row, LVIS_SELECTED | LVIS_FOCUSED, LVIS_SELECTED | LVIS_FOCUSED);
        if (ensureVisible) {
            m_list.EnsureVisible(row, FALSE);
        }
        m_selectedAudioPath = m_entries[index].audioPath;
        UpdateDetailPane();
        return true;
    }
    return false;
}

void RecordingsView::PopulateList() {
    m_populating = true;
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
    m_populating = false;
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

    const int topIndex = m_list.GetTopIndex();

    static constexpr int kWeights[] = {180, 130, 70, 70, 80};
    static constexpr int kColumnCount = 5;
    static constexpr int kWeightSum = 180 + 130 + 70 + 70 + 80;

    for (int pass = 0; pass < 2; ++pass) {
        RECT client{};
        m_list.GetClientRect(&client);
        const int width = client.right - client.left;
        if (width <= 0) {
            break;
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

    RestoreTopIndex(topIndex);
}

void RecordingsView::RestoreTopIndex(int topIndex) {
    if (!m_list.IsWindow()) {
        return;
    }
    const int count = m_list.GetItemCount();
    if (count <= 0) {
        return;
    }
    if (topIndex < 0) {
        topIndex = 0;
    } else if (topIndex >= count) {
        topIndex = count - 1;
    }

    RECT itemRect{};
    if (!m_list.GetItemRect(0, &itemRect, LVIR_BOUNDS)) {
        return;
    }
    const int itemHeight = itemRect.bottom - itemRect.top;
    if (itemHeight <= 0) {
        return;
    }

    const int delta = topIndex - m_list.GetTopIndex();
    if (delta != 0) {
        m_list.Scroll(0, delta * itemHeight);
    }
}

void RecordingsView::LayoutPanes(int clientWidth, int clientHeight) {
    const int gap = DpiScale(m_hWnd, kPaneGap);
    const int pad = gap;
    const int usable = clientWidth - pad * 2 - gap;
    if (usable <= 0 || clientHeight <= 0) {
        return;
    }

    const int listWidth = usable * kListWeight / (kListWeight + kDetailWeight);
    const int detailLeft = pad + listWidth + gap;
    const int detailWidth = clientWidth - detailLeft - pad;
    const int top = pad;
    const int height = clientHeight - pad * 2;

    if (m_list.IsWindow()) {
        m_list.SetWindowPos(nullptr, pad, top, listWidth, height, SWP_NOZORDER | SWP_NOACTIVATE);
    }

    const int btnH = DpiScale(m_hWnd, kButtonHeight);
    const int btnW = DpiScale(m_hWnd, kButtonWidth);
    const int createW = DpiScale(m_hWnd, kCreateWidth);
    const int btnGap = DpiScale(m_hWnd, 6);

    if (m_playButton.IsWindow()) {
        m_playButton.SetWindowPos(nullptr, detailLeft, top, btnW, btnH,
                                  SWP_NOZORDER | SWP_NOACTIVATE);
    }
    if (m_stopButton.IsWindow()) {
        m_stopButton.SetWindowPos(nullptr, detailLeft + btnW + btnGap, top, btnW, btnH,
                                  SWP_NOZORDER | SWP_NOACTIVATE);
    }

    const int contentTop = top + btnH + btnGap;
    const int contentHeight = height - btnH - btnGap;
    if (m_transcriptEdit.IsWindow()) {
        m_transcriptEdit.SetWindowPos(nullptr, detailLeft, contentTop, detailWidth, contentHeight,
                                      SWP_NOZORDER | SWP_NOACTIVATE);
    }
    if (m_emptyLabel.IsWindow()) {
        m_emptyLabel.SetWindowPos(nullptr, detailLeft, top + height / 3, detailWidth, btnH * 2,
                                  SWP_NOZORDER | SWP_NOACTIVATE);
    }
    if (m_createButton.IsWindow()) {
        m_createButton.SetWindowPos(nullptr, detailLeft, contentTop, createW, btnH,
                                    SWP_NOZORDER | SWP_NOACTIVATE);
    }
    if (m_statusLabel.IsWindow()) {
        m_statusLabel.SetWindowPos(nullptr, detailLeft, contentTop + btnH + btnGap, detailWidth,
                                   btnH * 3, SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

void RecordingsView::UpdateDetailPane() {
    const auto* entry = SelectedEntry();
    const bool hasSel = entry != nullptr;

    if (m_emptyLabel.IsWindow()) {
        m_emptyLabel.ShowWindow(hasSel ? SW_HIDE : SW_SHOW);
    }

    const int showSel = hasSel ? SW_SHOW : SW_HIDE;
    if (m_playButton.IsWindow()) {
        m_playButton.ShowWindow(showSel);
    }
    if (m_stopButton.IsWindow()) {
        m_stopButton.ShowWindow(showSel);
    }

    if (!hasSel) {
        m_selectedAudioPath.clear();
        if (m_transcriptEdit.IsWindow()) {
            m_transcriptEdit.ShowWindow(SW_HIDE);
            m_transcriptEdit.SetWindowText(L"");
        }
        if (m_createButton.IsWindow()) {
            m_createButton.ShowWindow(SW_HIDE);
        }
        if (m_statusLabel.IsWindow()) {
            m_statusLabel.ShowWindow(SW_HIDE);
            m_statusLabel.SetWindowText(L"");
        }
        UpdatePlaybackButtons();
        return;
    }

    m_selectedAudioPath = entry->audioPath;

    const bool active = entry->transcriptStatus == TranscriptStatus::Queued ||
                        entry->transcriptStatus == TranscriptStatus::Preparing ||
                        entry->transcriptStatus == TranscriptStatus::Transcribing;
    const bool failed = entry->transcriptStatus == TranscriptStatus::Failed;

    if (entry->hasTranscript) {
        if (m_transcriptEdit.IsWindow()) {
            m_transcriptEdit.SetWindowText(LoadTranscriptDisplayText(*entry).c_str());
            m_transcriptEdit.ShowWindow(SW_SHOW);
        }
        if (m_createButton.IsWindow()) {
            m_createButton.ShowWindow(SW_HIDE);
        }
        if (m_statusLabel.IsWindow()) {
            m_statusLabel.ShowWindow(SW_HIDE);
            m_statusLabel.SetWindowText(L"");
        }
    } else if (active) {
        if (m_transcriptEdit.IsWindow()) {
            m_transcriptEdit.ShowWindow(SW_HIDE);
            m_transcriptEdit.SetWindowText(L"");
        }
        if (m_createButton.IsWindow()) {
            m_createButton.ShowWindow(SW_HIDE);
        }
        if (m_statusLabel.IsWindow()) {
            m_statusLabel.SetWindowText(TranscriptLabel(*entry).c_str());
            m_statusLabel.ShowWindow(SW_SHOW);
        }
    } else {
        if (m_transcriptEdit.IsWindow()) {
            m_transcriptEdit.ShowWindow(SW_HIDE);
            m_transcriptEdit.SetWindowText(L"");
        }
        if (m_createButton.IsWindow()) {
            m_createButton.SetWindowText(L"&Create transcript");
            m_createButton.ShowWindow(SW_SHOW);
        }
        if (m_statusLabel.IsWindow()) {
            if (failed && !entry->transcriptError.empty()) {
                m_statusLabel.SetWindowText(entry->transcriptError.c_str());
                m_statusLabel.ShowWindow(SW_SHOW);
            } else {
                m_statusLabel.ShowWindow(SW_HIDE);
                m_statusLabel.SetWindowText(L"");
            }
        }
    }

    UpdatePlaybackButtons();
}

void RecordingsView::UpdatePlaybackButtons() {
    const bool hasSel = SelectedEntry() != nullptr;
    const bool playing = m_player.IsPlaying();
    if (m_playButton.IsWindow()) {
        m_playButton.EnableWindow(hasSel && !playing ? TRUE : FALSE);
    }
    if (m_stopButton.IsWindow()) {
        m_stopButton.EnableWindow(playing ? TRUE : FALSE);
    }
}

void RecordingsView::EnableMenuActions(CMenuHandle popup, bool hasSel) {
    const UINT selFlag = MF_BYCOMMAND | (hasSel ? MF_ENABLED : MF_GRAYED);
    for (const int id : kMenuActionIds) {
        popup.EnableMenuItem(id, selFlag);
    }
}

void RecordingsView::StopPlayback() {
    m_player.Stop();
    UpdatePlaybackButtons();
}

int RecordingsView::SelectedIndex() const {
    return m_list.GetSelectedIndex();
}

auto RecordingsView::SelectedEntry() const -> const RecordingEntry* {
    const int sel = SelectedIndex();
    if (sel < 0) {
        return nullptr;
    }
    const auto index = static_cast<size_t>(m_list.GetItemData(sel));
    if (index >= m_entries.size()) {
        return nullptr;
    }
    return &m_entries[index];
}

auto RecordingsView::FormatDuration(double seconds) -> std::wstring {
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

auto RecordingsView::FormatSize(uint64_t bytes) -> std::wstring {
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

auto RecordingsView::FormatDate(const std::wstring& startLocal) -> std::wstring {
    return Text::IsoToDisplayMinute(startLocal);
}

auto RecordingsView::TranscriptLabel(const RecordingEntry& entry) -> std::wstring {
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
        if (entry.hasTranscript) {
            return L"Ready";
        }
        return L"—";
    }
}

auto RecordingsView::LoadTranscriptDisplayText(const RecordingEntry& entry) -> std::wstring {
    auto bytes = FileIo::ReadFileBytes(entry.transcriptPath, FileIo::StripBom::Yes);
    if (!bytes) {
        return {};
    }
    std::string utf8 = std::move(*bytes);
    if (entry.transcriptIsLegacyMarkdown) {
        utf8 = TranscriptText::StripLegacyMarkdown(utf8);
    }
    return Text::Utf8ToWide(utf8);
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
    std::sort(m_entries.begin(), m_entries.end(),
              [this](const RecordingEntry& a, const RecordingEntry& b) {
                  int cmp = 0;
                  switch (m_sortColumn) {
                  case ColName:
                      cmp = _wcsicmp(a.displayName.c_str(), b.displayName.c_str());
                      break;
                  case ColDate:
                      cmp = a.startLocal.compare(b.startLocal);
                      break;
                  case ColDuration:
                      cmp = (a.durationSeconds < b.durationSeconds)
                                ? -1
                                : (a.durationSeconds > b.durationSeconds ? 1 : 0);
                      break;
                  case ColSize:
                      cmp = (a.sizeBytes < b.sizeBytes) ? -1 : (a.sizeBytes > b.sizeBytes ? 1 : 0);
                      break;
                  case ColTranscript:
                      cmp = TranscriptLabel(a).compare(TranscriptLabel(b));
                      break;
                  }
                  return m_sortAscending ? (cmp < 0) : (cmp > 0);
              });
}

bool RecordingsView::PromptRename(std::wstring& stemName) {
    RenameDialog dlg(stemName);
    return dlg.DoModal(m_hWnd) == IDOK && !stemName.empty();
}

void RecordingsView::OnPlay(UINT /*code*/, int /*id*/, HWND /*control*/) {
    const auto* entry = SelectedEntry();
    if (entry == nullptr) {
        return;
    }
    if (!m_player.Play(entry->audioPath)) {
        ui::ShowMessage(m_hWnd, L"Could not play this recording.", MB_OK | MB_ICONWARNING);
        return;
    }
    UpdatePlaybackButtons();
}

void RecordingsView::OnStop(UINT /*code*/, int /*id*/, HWND /*control*/) {
    StopPlayback();
}

void RecordingsView::OnTranscribe(UINT /*code*/, int /*id*/, HWND /*control*/) {
    const auto* entry = SelectedEntry();
    if (entry == nullptr) {
        return;
    }
    auto& service = TranscriptionService::Instance();
    if (service.IsQueuedOrActive(entry->audioPath)) {
        service.Cancel(entry->audioPath);
        return;
    }
    const auto& settings = Settings::Instance().Data();
    const auto modelFile =
        settings.model.empty() ? std::wstring(kDefaultWhisperModel) : settings.model;
    const auto modelPath = ModelPrompt::EnsureModel(GetParent(), modelFile);
    if (modelPath.empty() || !service.Enqueue(entry->audioPath, modelPath)) {
        return;
    }
    Refresh();
}

void RecordingsView::OnOpenFolder(UINT /*code*/, int /*id*/, HWND /*control*/) {
    const auto* entry = SelectedEntry();
    if (entry == nullptr) {
        return;
    }
    const auto params = L"/select,\"" + entry->audioPath + L"\"";
    ::ShellExecuteW(m_hWnd, L"open", L"explorer.exe", params.c_str(), nullptr, SW_SHOWNORMAL);
}

void RecordingsView::OnRename(UINT /*code*/, int /*id*/, HWND /*control*/) {
    const auto* entry = SelectedEntry();
    if (entry == nullptr) {
        return;
    }
    auto stem = std::filesystem::path(entry->stem).filename().wstring();
    if (!PromptRename(stem)) {
        return;
    }
    StopPlayback();
    std::wstring error;
    if (!RecordingLibrary::Rename(*entry, stem, error)) {
        ui::ShowMessage(m_hWnd, error.c_str(), MB_OK | MB_ICONWARNING);
        return;
    }
    Refresh();
}

void RecordingsView::OnDelete(UINT /*code*/, int /*id*/, HWND /*control*/) {
    const auto* entry = SelectedEntry();
    if (entry == nullptr) {
        return;
    }
    const int answer =
        ui::ShowMessage(m_hWnd, L"Move this recording and its sidecars to the Recycle Bin?",
                        MB_YESNO | MB_ICONQUESTION | MB_DEFBUTTON2);
    if (answer != IDYES) {
        return;
    }
    StopPlayback();
    std::wstring error;
    if (!RecordingLibrary::DeleteToRecycleBin(*entry, error)) {
        ui::ShowMessage(m_hWnd, error.c_str(), MB_OK | MB_ICONWARNING);
        return;
    }
    m_selectedAudioPath.clear();
    Refresh();
}

void RecordingsView::OnRefresh(UINT /*code*/, int /*id*/, HWND /*control*/) {
    Refresh();
}

LRESULT RecordingsView::OnColumnClick(LPNMHDR header) {
    const auto* listHeader = reinterpret_cast<LPNMLISTVIEW>(header);
    if (listHeader != nullptr) {
        const auto keepPath = m_selectedAudioPath;
        ToggleSort(static_cast<Column>(listHeader->iSubItem));
        PopulateList();
        if (!keepPath.empty()) {
            SelectByAudioPath(keepPath);
        }
    }
    return 0;
}

LRESULT RecordingsView::OnDoubleClick(LPNMHDR /*header*/) {
    OnPlay(0, 0, nullptr);
    return 0;
}

LRESULT RecordingsView::OnGetInfoTip(LPNMHDR header) {
    auto* tip = reinterpret_cast<NMLVGETINFOTIPW*>(header);
    if (tip == nullptr || tip->iItem < 0) {
        return 0;
    }
    const auto index = static_cast<size_t>(m_list.GetItemData(tip->iItem));
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
    auto popup = menu.GetSubMenu(0);
    if (popup.m_hMenu == nullptr) {
        return 0;
    }

    EnableMenuActions(popup, SelectedEntry() != nullptr);

    POINT cursor{};
    ::GetCursorPos(&cursor);
    popup.TrackPopupMenu(TPM_RIGHTBUTTON | TPM_LEFTALIGN, cursor.x, cursor.y, m_hWnd);
    return 0;
}

LRESULT RecordingsView::OnItemChanged(LPNMHDR header) {
    if (m_populating) {
        return 0;
    }
    const auto* info = reinterpret_cast<LPNMLISTVIEW>(header);
    if (info == nullptr || (info->uChanged & LVIF_STATE) == 0) {
        return 0;
    }
    if ((info->uNewState & LVIS_SELECTED) == 0 && (info->uOldState & LVIS_SELECTED) == 0) {
        return 0;
    }

    const auto* entry = SelectedEntry();
    const std::wstring nextPath = entry != nullptr ? entry->audioPath : std::wstring{};
    if (m_player.IsPlaying() &&
        (nextPath.empty() || !Text::PathEquals(nextPath, m_player.Path()))) {
        StopPlayback();
    }
    UpdateDetailPane();
    return 0;
}

LRESULT RecordingsView::OnLibraryChanged(UINT /*msg*/, WPARAM /*wParam*/, LPARAM /*lParam*/,
                                         BOOL& handled) {
    handled = TRUE;
    Refresh();
    return 0;
}

LRESULT RecordingsView::OnPlayback(UINT /*msg*/, WPARAM /*wParam*/, LPARAM lParam, BOOL& handled) {
    handled = TRUE;
    // EOF posts lParam != 0 from the audio callback; release the device on the UI thread.
    // Plain state posts (lParam 0) only refresh the buttons — calling Stop() there would race a
    // new Play() that already queued a stop notification from its internal Stop().
    if (lParam != 0) {
        m_player.StopFromEof();
    }
    UpdatePlaybackButtons();
    return 0;
}
