#pragma once

#include "pch.hpp"

#include "Ids.hpp"
#include "infrastructure/files/AudioPlayer.hpp"
#include "infrastructure/storage/DirectoryWatcher.hpp"
#include "infrastructure/storage/RecordingLibrary.hpp"

#include <vector>

class RecordingsView : public CDialogImpl<RecordingsView> {
  public:
    enum {
        IDD = IDD_RECORDINGS_PAGE
    };

    enum Column {
        ColName = 0,
        ColDate = 1,
        ColDuration = 2,
        ColSize = 3,
        ColTranscript = 4,
    };

    BEGIN_MSG_MAP_EX(RecordingsView)
    MSG_WM_INITDIALOG(OnInitDialog)
    MSG_WM_SIZE(OnSize)
    MSG_WM_DESTROY(OnDestroy)
    MSG_WM_CTLCOLORDLG(OnCtlColorDlg)
    MSG_WM_CTLCOLORSTATIC(OnCtlColorStatic)
    COMMAND_ID_HANDLER_EX(IDC_REC_PLAY, OnPlay)
    COMMAND_ID_HANDLER_EX(IDC_REC_DETAIL_PLAY, OnPlay)
    COMMAND_ID_HANDLER_EX(IDC_REC_DETAIL_STOP, OnStop)
    COMMAND_ID_HANDLER_EX(IDC_REC_TRANSCRIBE, OnTranscribe)
    COMMAND_ID_HANDLER_EX(IDC_REC_DETAIL_CREATE, OnTranscribe)
    COMMAND_ID_HANDLER_EX(IDC_REC_OPENFOLDER, OnOpenFolder)
    COMMAND_ID_HANDLER_EX(IDC_REC_RENAME, OnRename)
    COMMAND_ID_HANDLER_EX(IDC_REC_DELETE, OnDelete)
    COMMAND_ID_HANDLER_EX(IDC_REC_REFRESH, OnRefresh)
    NOTIFY_HANDLER_EX(IDC_RECORDINGS_LIST, LVN_COLUMNCLICK, OnColumnClick)
    NOTIFY_HANDLER_EX(IDC_RECORDINGS_LIST, NM_DBLCLK, OnDoubleClick)
    NOTIFY_HANDLER_EX(IDC_RECORDINGS_LIST, NM_RCLICK, OnRightClick)
    NOTIFY_HANDLER_EX(IDC_RECORDINGS_LIST, LVN_ITEMCHANGED, OnItemChanged)
    NOTIFY_HANDLER_EX(IDC_RECORDINGS_LIST, LVN_GETINFOTIP, OnGetInfoTip)
    MESSAGE_HANDLER(WM_APP_LIBRARY_CHANGED, OnLibraryChanged)
    MESSAGE_HANDLER(WM_APP_PLAYBACK, OnPlayback)
    END_MSG_MAP()

    void Refresh();
    void EnsureWatching();
    void SetTranscriptProgress(const std::wstring& audioPath, int percent);
    void ApplyUiFont(HFONT font);
    bool SelectByAudioPath(const std::wstring& audioPath);

  private:
    BOOL OnInitDialog(CWindow focus, LPARAM initParam);
    void OnSize(UINT type, CSize size);
    void OnDestroy();
    HBRUSH OnCtlColorDlg(CDCHandle dc, CWindow wnd);
    HBRUSH OnCtlColorStatic(CDCHandle dc, CStatic wndStatic);
    void OnPlay(UINT code, int id, HWND control);
    void OnStop(UINT code, int id, HWND control);
    void OnTranscribe(UINT code, int id, HWND control);
    void OnOpenFolder(UINT code, int id, HWND control);
    void OnRename(UINT code, int id, HWND control);
    void OnDelete(UINT code, int id, HWND control);
    void OnRefresh(UINT code, int id, HWND control);
    LRESULT OnColumnClick(LPNMHDR header);
    LRESULT OnDoubleClick(LPNMHDR header);
    LRESULT OnRightClick(LPNMHDR header);
    LRESULT OnItemChanged(LPNMHDR header);
    LRESULT OnGetInfoTip(LPNMHDR header);
    LRESULT OnLibraryChanged(UINT msg, WPARAM wParam, LPARAM lParam, BOOL& handled);
    LRESULT OnPlayback(UINT msg, WPARAM wParam, LPARAM lParam, BOOL& handled);

    void PopulateList();
    void FitColumns();
    void LayoutPanes(int clientWidth, int clientHeight);
    void UpdateDetailPane();
    void UpdatePlaybackButtons();
    void EnableMenuActions(WTL::CMenuHandle popup, bool hasSel);
    void StopPlayback();
    int SelectedIndex() const;
    auto SelectedEntry() const -> const RecordingEntry*;
    void ApplySort();
    void ToggleSort(Column column);
    static auto FormatDuration(double seconds) -> std::wstring;
    static auto FormatSize(uint64_t bytes) -> std::wstring;
    static auto FormatDate(const std::wstring& startLocal) -> std::wstring;
    static auto TranscriptLabel(const RecordingEntry& entry) -> std::wstring;
    auto LoadTranscriptDisplayText(const RecordingEntry& entry) -> std::wstring;
    bool PromptRename(std::wstring& stemName);

    WTL::CListViewCtrl m_list;
    WTL::CEdit m_transcriptEdit;
    WTL::CStatic m_emptyLabel;
    WTL::CStatic m_statusLabel;
    WTL::CButton m_playButton;
    WTL::CButton m_stopButton;
    WTL::CButton m_createButton;
    AudioPlayer m_player;
    std::vector<RecordingEntry> m_entries;
    DirectoryWatcher m_watcher;
    Column m_sortColumn = ColDate;
    bool m_sortAscending = false;
    std::wstring m_progressPath;
    int m_progressPercent = -1;
    std::wstring m_selectedAudioPath;
    bool m_populating = false;
};
