#pragma once

#include "pch.hpp"
#include "Ids.hpp"

#include <map>
#include <string>

class TranscriptWindow
    : public WTL::CFrameWindowImpl<TranscriptWindow>
    , public WTL::CMessageFilter {
public:
    DECLARE_FRAME_WND_CLASS(L"MeetingBuddy.TranscriptWindow", IDR_MAINFRAME)

    BEGIN_MSG_MAP_EX(TranscriptWindow)
        MSG_WM_CREATE(OnCreate)
        MSG_WM_SIZE(OnSize)
        MSG_WM_CLOSE(OnClose)
        MSG_WM_DESTROY(OnDestroy)
        MSG_WM_SETFOCUS(OnSetFocus)
        COMMAND_ID_HANDLER_EX(ID_TRANSCRIPT_COPYALL, OnCopyAll)
        COMMAND_ID_HANDLER_EX(ID_TRANSCRIPT_SAVEAS, OnSaveAs)
        COMMAND_ID_HANDLER_EX(ID_TRANSCRIPT_OPENEDITOR, OnOpenEditor)
        COMMAND_ID_HANDLER_EX(ID_TRANSCRIPT_FIND, OnFind)
        COMMAND_ID_HANDLER_EX(ID_EDIT_COPY, OnEditCopy)
        COMMAND_ID_HANDLER_EX(ID_EDIT_SELECT_ALL, OnSelectAll)
        MESSAGE_HANDLER(WM_COPY, OnCopyMessage)
        MESSAGE_HANDLER(m_findMsg, OnFindMsg)
        CHAIN_MSG_MAP(WTL::CFrameWindowImpl<TranscriptWindow>)
    END_MSG_MAP()

    BOOL PreTranslateMessage(MSG* msg);

    static auto OpenOrActivate(const std::wstring& mdPath) -> TranscriptWindow*;
    static void CloseAll();

    bool LoadFile(const std::wstring& mdPath);
    auto Path() const -> const std::wstring& { return m_path; }
    void OnFinalMessage(HWND /*hwnd*/) override { delete this; }

private:
    LRESULT OnCreate(LPCREATESTRUCT createStruct);
    void OnSize(UINT type, CSize size);
    void OnClose();
    void OnDestroy();
    void OnSetFocus(CWindow oldFocus);
    void OnCopyAll(UINT code, int id, HWND control);
    void OnSaveAs(UINT code, int id, HWND control);
    void OnOpenEditor(UINT code, int id, HWND control);
    void OnFind(UINT code, int id, HWND control);
    void OnEditCopy(UINT code, int id, HWND control);
    void OnSelectAll(UINT code, int id, HWND control);
    LRESULT OnCopyMessage(UINT msg, WPARAM wParam, LPARAM lParam, BOOL& handled);
    LRESULT OnFindMsg(UINT msg, WPARAM wParam, LPARAM lParam, BOOL& handled);

    void LayoutChildren();
    void ApplyLightStyling();
    void UpdateWordCount();
    auto GetPlainText() const -> std::wstring;
    auto GetSelectionOrAll() const -> std::wstring;
    bool CopyPlainMarkdown(bool selectionOnly);

    WTL::CToolBarCtrl m_toolbar;
    WTL::CRichEditCtrl m_edit;
    WTL::CMultiPaneStatusBarCtrl m_status;
    std::wstring m_path;
    std::string m_utf8Source; // UTF-8 from disk for Save as
    FINDREPLACEW m_find{};
    wchar_t m_findBuffer[256]{};
    HWND m_findDlg = nullptr;
    UINT m_findMsg = 0;
    HMODULE m_msftedit = nullptr;

    static std::map<std::wstring, TranscriptWindow*> s_open;
};
