#pragma once

#include "pch.hpp"
#include "Ids.hpp"
#include "storage/DirectoryWatcher.hpp"
#include "storage/RecordingLibrary.hpp"

#include <vector>

class RecordingsView
    : public CDialogImpl<RecordingsView>
    , public WTL::CDialogResize<RecordingsView> {
public:
    enum { IDD = IDD_RECORDINGS_PAGE };

    enum Column {
        ColName = 0,
        ColDate = 1,
        ColDuration = 2,
        ColSize = 3,
        ColTranscript = 4,
    };

    BEGIN_MSG_MAP_EX(RecordingsView)
        MSG_WM_INITDIALOG(OnInitDialog)
        MSG_WM_DESTROY(OnDestroy)
        COMMAND_ID_HANDLER_EX(IDC_REC_PLAY, OnPlay)
        COMMAND_ID_HANDLER_EX(IDC_REC_TRANSCRIBE, OnTranscribe)
        COMMAND_ID_HANDLER_EX(IDC_REC_VIEW, OnView)
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
        CHAIN_MSG_MAP(WTL::CDialogResize<RecordingsView>)
    END_MSG_MAP()

    BEGIN_DLGRESIZE_MAP(RecordingsView)
        DLGRESIZE_CONTROL(IDC_RECORDINGS_LIST, DLSZ_SIZE_X | DLSZ_SIZE_Y)
        DLGRESIZE_CONTROL(IDC_REC_REFRESH, DLSZ_MOVE_Y)
    END_DLGRESIZE_MAP()

    void Refresh();
    void EnsureWatching();
    void SetTranscriptProgress(const std::wstring& audioPath, int percent);

private:
    BOOL OnInitDialog(CWindow focus, LPARAM initParam);
    void OnDestroy();
    void OnPlay(UINT code, int id, HWND control);
    void OnTranscribe(UINT code, int id, HWND control);
    void OnView(UINT code, int id, HWND control);
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

    void PopulateList();
    void UpdateActionEnabled();
    int SelectedIndex() const;
    const RecordingEntry* SelectedEntry() const;
    void ApplySort();
    void ToggleSort(Column column);
    static std::wstring FormatDuration(double seconds);
    static std::wstring FormatSize(uint64_t bytes);
    static std::wstring FormatDate(const std::wstring& startLocal);
    static std::wstring TranscriptLabel(const RecordingEntry& entry);
    bool PromptRename(std::wstring& stemName);

    WTL::CListViewCtrl m_list;
    std::vector<RecordingEntry> m_entries;
    DirectoryWatcher m_watcher;
    Column m_sortColumn = ColDate;
    bool m_sortAscending = false;
    std::wstring m_progressPath;
    int m_progressPercent = -1;
};
