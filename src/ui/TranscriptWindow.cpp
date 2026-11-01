#include "ui/TranscriptWindow.hpp"

#include "storage/RecordingSidecar.hpp"
#include "transcription/MarkdownFormatter.hpp"
#include "util/Clipboard.hpp"
#include "util/Log.hpp"

#include <commdlg.h>
#include <richedit.h>
#include <wrl/client.h>

#include <algorithm>
#include <cctype>
#include <cstdio>
#include <filesystem>
#include <fstream>
#include <iterator>
#include <sstream>
#include <vector>

std::map<std::wstring, TranscriptWindow*> TranscriptWindow::s_open;

namespace {

std::wstring NormalizePathKey(const std::wstring& path) {
    std::wstring key = path;
    for (auto& ch : key) {
        if (ch == L'/') {
            ch = L'\\';
        }
        ch = static_cast<wchar_t>(towlower(ch));
    }
    return key;
}

int CountWords(const std::wstring& text) {
    int count = 0;
    bool inWord = false;
    for (wchar_t ch : text) {
        const bool space = iswspace(ch) != 0;
        if (!space && !inWord) {
            ++count;
            inWord = true;
        } else if (space) {
            inWord = false;
        }
    }
    return count;
}

}  // namespace

BOOL TranscriptWindow::PreTranslateMessage(MSG* msg) {
    if (msg != nullptr && msg->message == WM_KEYDOWN) {
        const bool ctrl = (::GetKeyState(VK_CONTROL) & 0x8000) != 0;
        if (ctrl && msg->wParam == 'F') {
            OnFind(0, 0, nullptr);
            return TRUE;
        }
        if (ctrl && msg->wParam == 'A') {
            OnSelectAll(0, 0, nullptr);
            return TRUE;
        }
        if (ctrl && msg->wParam == 'C') {
            OnEditCopy(0, 0, nullptr);
            return TRUE;
        }
    }
    if (m_findDlg != nullptr && ::IsDialogMessageW(m_findDlg, msg)) {
        return TRUE;
    }
    return FALSE;
}

TranscriptWindow* TranscriptWindow::OpenOrActivate(const std::wstring& mdPath) {
    if (mdPath.empty()) {
        return nullptr;
    }
    const std::wstring key = NormalizePathKey(mdPath);
    const auto it = s_open.find(key);
    if (it != s_open.end() && it->second != nullptr && it->second->IsWindow()) {
        it->second->ShowWindow(SW_RESTORE);
        ::SetForegroundWindow(it->second->m_hWnd);
        return it->second;
    }

    auto* window = new TranscriptWindow();
    if (window->CreateEx() == nullptr) {
        delete window;
        return nullptr;
    }
    if (!window->LoadFile(mdPath)) {
        window->DestroyWindow();
        delete window;
        return nullptr;
    }
    window->ShowWindow(SW_SHOW);
    window->UpdateWindow();
    return window;
}

void TranscriptWindow::CloseAll() {
    std::vector<TranscriptWindow*> windows;
    for (auto& entry : s_open) {
        if (entry.second != nullptr) {
            windows.push_back(entry.second);
        }
    }
    for (auto* window : windows) {
        if (window->IsWindow()) {
            window->DestroyWindow();
        }
    }
    s_open.clear();
}

LRESULT TranscriptWindow::OnCreate(LPCREATESTRUCT /*createStruct*/) {
    WTL::CMessageLoop* loop = _Module.GetMessageLoop();
    if (loop != nullptr) {
        loop->AddMessageFilter(this);
    }

    m_findMsg = ::RegisterWindowMessageW(FINDMSGSTRINGW);
    m_msftedit = ::LoadLibraryW(L"Msftedit.dll");
    if (m_msftedit == nullptr) {
        Log::Error(L"Failed to load Msftedit.dll");
        return -1;
    }

    m_hWndClient = nullptr;
    const DWORD toolbarStyle = WS_CHILD | WS_VISIBLE | CCS_TOP | TBSTYLE_LIST | TBSTYLE_WRAPABLE | TBSTYLE_TOOLTIPS;
    HWND toolbarHwnd = ::CreateWindowExW(
        0,
        TOOLBARCLASSNAMEW,
        nullptr,
        toolbarStyle,
        0,
        0,
        0,
        0,
        m_hWnd,
        reinterpret_cast<HMENU>(static_cast<INT_PTR>(IDC_TRANSCRIPT_TOOLBAR)),
        _Module.GetModuleInstance(),
        nullptr);
    m_hWndToolBar = toolbarHwnd;
    m_toolbar.Attach(toolbarHwnd);
    m_toolbar.SendMessage(TB_BUTTONSTRUCTSIZE, sizeof(TBBUTTON), 0);
    m_toolbar.SetExtendedStyle(TBSTYLE_EX_MIXEDBUTTONS);
    TBBUTTON buttons[] = {
        {I_IMAGENONE, ID_TRANSCRIPT_COPYALL, TBSTATE_ENABLED, BTNS_BUTTON | BTNS_AUTOSIZE | BTNS_SHOWTEXT, {0}, 0, (INT_PTR)L"&Copy all"},
        {I_IMAGENONE, ID_TRANSCRIPT_SAVEAS, TBSTATE_ENABLED, BTNS_BUTTON | BTNS_AUTOSIZE | BTNS_SHOWTEXT, {0}, 0, (INT_PTR)L"&Save as…"},
        {I_IMAGENONE, ID_TRANSCRIPT_OPENEDITOR, TBSTATE_ENABLED, BTNS_BUTTON | BTNS_AUTOSIZE | BTNS_SHOWTEXT, {0}, 0, (INT_PTR)L"&Open in editor"},
        {I_IMAGENONE, ID_TRANSCRIPT_FIND, TBSTATE_ENABLED, BTNS_BUTTON | BTNS_AUTOSIZE | BTNS_SHOWTEXT, {0}, 0, (INT_PTR)L"&Find"},
    };
    m_toolbar.AddButtons(static_cast<int>(std::size(buttons)), buttons);
    m_toolbar.AutoSize();

    m_status.Create(m_hWnd);
    m_hWndStatusBar = m_status.m_hWnd;
    int panes[] = {0};
    m_status.SetPanes(panes, 1, true);

    const DWORD style = WS_CHILD | WS_VISIBLE | WS_VSCROLL | ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL | ES_NOHIDESEL;
    HWND edit = ::CreateWindowExW(
        WS_EX_CLIENTEDGE,
        MSFTEDIT_CLASS,
        L"",
        style,
        0,
        0,
        0,
        0,
        m_hWnd,
        reinterpret_cast<HMENU>(static_cast<INT_PTR>(IDC_TRANSCRIPT_EDIT)),
        _Module.GetModuleInstance(),
        nullptr);
    if (edit == nullptr) {
        Log::Error(L"Failed to create RichEdit control");
        return -1;
    }
    m_edit.Attach(edit);
    m_edit.SetEventMask(m_edit.GetEventMask() | ENM_SELCHANGE | ENM_KEYEVENTS);
    m_edit.SendMessage(EM_AUTOURLDETECT, TRUE, 0);
    m_edit.SetBackgroundColor(RGB(252, 252, 250));

    SetWindowText(L"Transcript");
    ResizeClient(720, 560);
    CenterWindow();
    return 0;
}

bool TranscriptWindow::LoadFile(const std::wstring& mdPath) {
    std::ifstream file(mdPath, std::ios::binary);
    if (!file) {
        Log::Error(L"Could not open transcript: " + mdPath);
        return false;
    }
    std::ostringstream ss;
    ss << file.rdbuf();
    m_utf8Source = ss.str();
    // Strip BOM if present.
    if (m_utf8Source.size() >= 3
        && static_cast<unsigned char>(m_utf8Source[0]) == 0xEF
        && static_cast<unsigned char>(m_utf8Source[1]) == 0xBB
        && static_cast<unsigned char>(m_utf8Source[2]) == 0xBF) {
        m_utf8Source.erase(0, 3);
    }

    const std::wstring wide = Sidecar::Utf8ToWide(m_utf8Source);
    m_edit.SetWindowText(wide.c_str());
    ApplyLightStyling();
    UpdateWordCount();

    if (!m_path.empty()) {
        s_open.erase(NormalizePathKey(m_path));
    }
    m_path = mdPath;
    s_open[NormalizePathKey(m_path)] = this;

    const std::wstring title = L"Transcript — " + std::filesystem::path(mdPath).filename().wstring();
    SetWindowText(title.c_str());
    LayoutChildren();
    return true;
}

void TranscriptWindow::ApplyLightStyling() {
    const int length = m_edit.GetWindowTextLength();
    if (length <= 0) {
        return;
    }

    CHARFORMAT2W base{};
    base.cbSize = sizeof(base);
    base.dwMask = CFM_FACE | CFM_SIZE | CFM_COLOR | CFM_BOLD;
    base.dwEffects = 0;
    base.yHeight = 200; // 10 pt
    base.crTextColor = RGB(30, 30, 30);
    wcscpy_s(base.szFaceName, L"Consolas");
    m_edit.SetSel(0, -1);
    m_edit.SetSelectionCharFormat(base);

    CString text;
    m_edit.GetWindowText(text);
    const std::wstring body = text.GetString();

    // Headings starting with '#'
    size_t lineStart = 0;
    while (lineStart < body.size()) {
        size_t lineEnd = body.find(L'\n', lineStart);
        if (lineEnd == std::wstring::npos) {
            lineEnd = body.size();
        }
        if (body[lineStart] == L'#') {
            CHARFORMAT2W heading{};
            heading.cbSize = sizeof(heading);
            heading.dwMask = CFM_BOLD | CFM_SIZE | CFM_COLOR;
            heading.dwEffects = CFE_BOLD;
            heading.yHeight = body.compare(lineStart, 2, L"##") == 0 ? 240 : 280;
            heading.crTextColor = RGB(20, 20, 20);
            m_edit.SetSel(static_cast<int>(lineStart), static_cast<int>(lineEnd));
            m_edit.SetSelectionCharFormat(heading);
        }
        lineStart = lineEnd + 1;
    }

    // **[hh:mm:ss]** timestamps
    size_t pos = 0;
    while ((pos = body.find(L"**[", pos)) != std::wstring::npos) {
        const size_t end = body.find(L"]**", pos);
        if (end == std::wstring::npos) {
            break;
        }
        CHARFORMAT2W stamp{};
        stamp.cbSize = sizeof(stamp);
        stamp.dwMask = CFM_COLOR | CFM_BOLD;
        stamp.dwEffects = 0;
        stamp.crTextColor = RGB(120, 120, 120);
        m_edit.SetSel(static_cast<int>(pos), static_cast<int>(end + 3));
        m_edit.SetSelectionCharFormat(stamp);
        pos = end + 3;
    }

    m_edit.SetSel(0, 0);
}

void TranscriptWindow::UpdateWordCount() {
    const int words = CountWords(GetPlainText());
    wchar_t buf[64]{};
    std::swprintf(buf, 64, L"%d words", words);
    m_status.SetPaneText(0, buf);
}

std::wstring TranscriptWindow::GetPlainText() const {
    CString text;
    const_cast<TranscriptWindow*>(this)->m_edit.GetWindowText(text);
    return text.GetString();
}

std::wstring TranscriptWindow::GetSelectionOrAll() const {
    CHARRANGE range{};
    const_cast<TranscriptWindow*>(this)->m_edit.GetSel(range);
    if (range.cpMin == range.cpMax) {
        return GetPlainText();
    }
    const std::wstring all = GetPlainText();
    if (range.cpMin < 0 || range.cpMax > static_cast<LONG>(all.size()) || range.cpMin > range.cpMax) {
        return all;
    }
    return all.substr(static_cast<size_t>(range.cpMin), static_cast<size_t>(range.cpMax - range.cpMin));
}

bool TranscriptWindow::CopyPlainMarkdown(bool selectionOnly) {
    const std::wstring text = selectionOnly ? GetSelectionOrAll() : GetPlainText();
    return Clipboard::SetUnicodeText(m_hWnd, text);
}

void TranscriptWindow::LayoutChildren() {
    UpdateLayout(FALSE);
    CRect client;
    GetClientRect(&client);

    int top = 0;
    if (m_hWndToolBar != nullptr && ::IsWindowVisible(m_hWndToolBar)) {
        CRect toolbarRect;
        ::GetWindowRect(m_hWndToolBar, &toolbarRect);
        top = toolbarRect.Height();
    }
    int bottom = client.bottom;
    if (m_hWndStatusBar != nullptr && ::IsWindowVisible(m_hWndStatusBar)) {
        CRect statusRect;
        ::GetWindowRect(m_hWndStatusBar, &statusRect);
        bottom -= statusRect.Height();
        m_status.UpdatePanesLayout();
    }
    if (m_edit.IsWindow()) {
        m_edit.SetWindowPos(nullptr, 0, top, client.Width(), (std::max)(0, bottom - top), SWP_NOZORDER);
    }
}

void TranscriptWindow::OnSize(UINT type, CSize /*size*/) {
    if (type != SIZE_MINIMIZED) {
        LayoutChildren();
    }
}

void TranscriptWindow::OnClose() {
    DestroyWindow();
}

void TranscriptWindow::OnDestroy() {
    WTL::CMessageLoop* loop = _Module.GetMessageLoop();
    if (loop != nullptr) {
        loop->RemoveMessageFilter(this);
    }
    if (!m_path.empty()) {
        s_open.erase(NormalizePathKey(m_path));
    }
    if (m_edit.IsWindow()) {
        m_edit.Detach();
    }
    if (m_toolbar.IsWindow()) {
        m_toolbar.Detach();
    }
    SetMsgHandled(FALSE);
}

void TranscriptWindow::OnSetFocus(CWindow /*oldFocus*/) {
    if (m_edit.IsWindow()) {
        m_edit.SetFocus();
    }
}

void TranscriptWindow::OnCopyAll(UINT /*code*/, int /*id*/, HWND /*control*/) {
    CopyPlainMarkdown(false);
}

void TranscriptWindow::OnEditCopy(UINT /*code*/, int /*id*/, HWND /*control*/) {
    CopyPlainMarkdown(true);
}

void TranscriptWindow::OnSelectAll(UINT /*code*/, int /*id*/, HWND /*control*/) {
    m_edit.SetSel(0, -1);
}

LRESULT TranscriptWindow::OnCopyMessage(UINT /*msg*/, WPARAM /*wParam*/, LPARAM /*lParam*/, BOOL& handled) {
    handled = TRUE;
    CopyPlainMarkdown(true);
    return 0;
}

void TranscriptWindow::OnSaveAs(UINT /*code*/, int /*id*/, HWND /*control*/) {
    COMDLG_FILTERSPEC filters[] = {
        {L"Markdown (*.md)", L"*.md"},
        {L"All files (*.*)", L"*.*"},
    };
    Microsoft::WRL::ComPtr<IFileSaveDialog> dialog;
    if (FAILED(::CoCreateInstance(CLSID_FileSaveDialog, nullptr, CLSCTX_INPROC_SERVER, IID_PPV_ARGS(&dialog))) || !dialog) {
        return;
    }
    dialog->SetFileTypes(2, filters);
    dialog->SetDefaultExtension(L"md");
    dialog->SetFileName(std::filesystem::path(m_path).filename().c_str());
    if (FAILED(dialog->Show(m_hWnd))) {
        return;
    }
    Microsoft::WRL::ComPtr<IShellItem> item;
    if (FAILED(dialog->GetResult(&item)) || !item) {
        return;
    }
    PWSTR path = nullptr;
    if (FAILED(item->GetDisplayName(SIGDN_FILESYSPATH, &path)) || path == nullptr) {
        return;
    }
    const std::wstring dest = path;
    ::CoTaskMemFree(path);
    if (!MarkdownFormatter::WriteUtf8NoBom(dest, m_utf8Source)) {
        ::MessageBoxW(m_hWnd, L"Could not save the transcript.", L"MeetingBuddy", MB_OK | MB_ICONWARNING);
    }
}

void TranscriptWindow::OnOpenEditor(UINT /*code*/, int /*id*/, HWND /*control*/) {
    if (!m_path.empty()) {
        ::ShellExecuteW(m_hWnd, L"open", m_path.c_str(), nullptr, nullptr, SW_SHOWNORMAL);
    }
}

void TranscriptWindow::OnFind(UINT /*code*/, int /*id*/, HWND /*control*/) {
    if (m_findDlg != nullptr && ::IsWindow(m_findDlg)) {
        ::SetFocus(m_findDlg);
        return;
    }
    ZeroMemory(&m_find, sizeof(m_find));
    m_find.lStructSize = sizeof(m_find);
    m_find.hwndOwner = m_hWnd;
    m_find.lpstrFindWhat = m_findBuffer;
    m_find.wFindWhatLen = static_cast<WORD>(std::size(m_findBuffer));
    m_find.Flags = FR_DOWN;
    m_findDlg = ::FindTextW(&m_find);
}

LRESULT TranscriptWindow::OnFindMsg(UINT /*msg*/, WPARAM /*wParam*/, LPARAM lParam, BOOL& handled) {
    handled = TRUE;
    auto* fr = reinterpret_cast<FINDREPLACEW*>(lParam);
    if (fr == nullptr) {
        return 0;
    }
    if (fr->Flags & FR_DIALOGTERM) {
        m_findDlg = nullptr;
        return 0;
    }
    if (!(fr->Flags & FR_FINDNEXT)) {
        return 0;
    }

    FINDTEXTEXW ft{};
    CHARRANGE sel{};
    m_edit.GetSel(sel);
    ft.chrg.cpMin = (fr->Flags & FR_DOWN) ? sel.cpMax : sel.cpMin;
    ft.chrg.cpMax = -1;
    ft.lpstrText = fr->lpstrFindWhat;
    DWORD flags = 0;
    if (fr->Flags & FR_MATCHCASE) {
        flags |= FR_MATCHCASE;
    }
    if (fr->Flags & FR_WHOLEWORD) {
        flags |= FR_WHOLEWORD;
    }
    if (fr->Flags & FR_DOWN) {
        flags |= FR_DOWN;
    }

    const LRESULT found = m_edit.SendMessage(EM_FINDTEXTEXW, flags, reinterpret_cast<LPARAM>(&ft));
    if (found >= 0) {
        m_edit.SetSel(ft.chrgText);
        m_edit.SendMessage(EM_SCROLLCARET, 0, 0);
    } else {
        ::MessageBoxW(m_hWnd, L"Text not found.", L"MeetingBuddy", MB_OK | MB_ICONINFORMATION);
    }
    return 0;
}
