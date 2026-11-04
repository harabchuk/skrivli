#include "views/ModelPrompt.hpp"

#include "infrastructure/common/Paths.hpp"
#include "infrastructure/common/Text.hpp"
#include "ui/ShellDialogs.hpp"
#include "ui/Theme.hpp"

#include <atomic>
#include <commctrl.h>
#include <filesystem>

namespace {

struct DownloadUi {
    HWND dialog = nullptr;
    HWND bar = nullptr;
};

void RequestCancel(HWND hwnd) {
    auto* cancel = reinterpret_cast<std::atomic_bool*>(::GetWindowLongPtrW(hwnd, GWLP_USERDATA));
    if (cancel != nullptr) {
        cancel->store(true);
    }
}

LRESULT CALLBACK DownloadWndProc(HWND hwnd, UINT msg, WPARAM wParam, LPARAM lParam) {
    if (msg == WM_CREATE) {
        auto* cs = reinterpret_cast<CREATESTRUCTW*>(lParam);
        ::SetWindowLongPtrW(hwnd, GWLP_USERDATA, reinterpret_cast<LONG_PTR>(cs->lpCreateParams));
        return 0;
    }
    if ((msg == WM_COMMAND && LOWORD(wParam) == IDCANCEL) || msg == WM_CLOSE) {
        RequestCancel(hwnd);
        return 0;
    }
    return ::DefWindowProcW(hwnd, msg, wParam, lParam);
}

auto CreateDownloadDialog(HWND owner, std::atomic_bool* cancel, HWND* bar) -> HWND {
    const wchar_t* className = L"Skrivli.DownloadDlg";
    WNDCLASSW wc{};
    if (!::GetClassInfoW(_Module.GetModuleInstance(), className, &wc)) {
        wc.lpfnWndProc = DownloadWndProc;
        wc.hInstance = _Module.GetModuleInstance();
        wc.lpszClassName = className;
        wc.hCursor = ::LoadCursor(nullptr, IDC_ARROW);
        wc.hbrBackground = reinterpret_cast<HBRUSH>(COLOR_WINDOW + 1);
        ::RegisterClassW(&wc);
    }

    auto dlg = ::CreateWindowExW(WS_EX_DLGMODALFRAME, className, L"Downloading model",
                                 WS_POPUP | WS_CAPTION | WS_SYSMENU, CW_USEDEFAULT, CW_USEDEFAULT,
                                 420, 140, owner, nullptr, _Module.GetModuleInstance(), cancel);
    if (dlg == nullptr) {
        return nullptr;
    }

    ::CreateWindowExW(0, L"STATIC", L"Downloading whisper model…", WS_CHILD | WS_VISIBLE, 16, 16,
                      370, 20, dlg, nullptr, _Module.GetModuleInstance(), nullptr);

    auto progress = ::CreateWindowExW(0, PROGRESS_CLASSW, L"", WS_CHILD | WS_VISIBLE, 16, 48, 370,
                                      18, dlg, nullptr, _Module.GetModuleInstance(), nullptr);
    ::SendMessageW(progress, PBM_SETRANGE, 0, MAKELPARAM(0, 1000));
    if (bar != nullptr) {
        *bar = progress;
    }

    ::CreateWindowExW(0, L"BUTTON", L"Cancel", WS_CHILD | WS_VISIBLE | BS_PUSHBUTTON, 300, 78, 86,
                      24, dlg, reinterpret_cast<HMENU>(static_cast<INT_PTR>(IDCANCEL)),
                      _Module.GetModuleInstance(), nullptr);

    RECT ownerRect{};
    if (owner != nullptr) {
        ::GetWindowRect(owner, &ownerRect);
        const int x = ownerRect.left + ((ownerRect.right - ownerRect.left) - 420) / 2;
        const int y = ownerRect.top + ((ownerRect.bottom - ownerRect.top) - 140) / 2;
        ::SetWindowPos(dlg, nullptr, x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER);
    }

    ::ShowWindow(dlg, SW_SHOW);
    ::UpdateWindow(dlg);
    return dlg;
}

void PumpMessages() {
    MSG msg{};
    while (::PeekMessageW(&msg, nullptr, 0, 0, PM_REMOVE)) {
        ::TranslateMessage(&msg);
        ::DispatchMessageW(&msg);
    }
}

} // namespace

bool ModelPrompt::BrowseAndInstall(HWND owner, std::wstring& outFileName) {
    static constexpr ui::FileFilter kFilters[] = {
        {L"Whisper models (*.bin)", L"*.bin"},
        {L"All files (*.*)", L"*.*"},
    };
    const auto picked = ui::PickOpenFile(owner, kFilters, L"Select a whisper ggml model");
    if (!picked) {
        return false;
    }
    const std::wstring& source = *picked;

    Paths::EnsureDirectory(Paths::ModelsDir());
    const auto leaf = std::filesystem::path(source).filename().wstring();
    const auto dest = ModelManager::ModelPath(leaf);
    std::error_code ec;
    if (!Text::PathEquals(source, dest)) {
        std::filesystem::copy_file(source, dest, std::filesystem::copy_options::overwrite_existing,
                                   ec);
        if (ec) {
            ui::ShowMessage(owner, L"Could not copy model into the models folder.",
                            MB_OK | MB_ICONWARNING);
            return false;
        }
    }
    outFileName = leaf;
    return true;
}

bool ModelPrompt::Download(HWND owner, const ModelInfo& info, std::wstring& error) {
    std::atomic_bool cancel{false};
    DownloadUi ui{};
    ui.dialog = CreateDownloadDialog(owner, &cancel, &ui.bar);
    if (owner != nullptr) {
        ::EnableWindow(owner, FALSE);
    }

    const bool ok = ModelManager::DownloadToFile(
        info, cancel,
        [&](uint64_t done, uint64_t total) {
            if (ui.bar != nullptr && total > 0) {
                const auto pos = static_cast<int>((done * 1000ull) / total);
                ::SendMessageW(ui.bar, PBM_SETPOS, static_cast<WPARAM>(pos), 0);
            }
            PumpMessages();
        },
        error);

    if (ui.dialog != nullptr) {
        ::DestroyWindow(ui.dialog);
    }
    if (owner != nullptr) {
        ::EnableWindow(owner, TRUE);
        ::SetForegroundWindow(owner);
    }
    return ok;
}

auto ModelPrompt::EnsureModel(HWND owner, const std::wstring& fileName) -> std::wstring {
    Paths::EnsureDirectory(Paths::ModelsDir());
    std::wstring chosen = fileName.empty() ? ModelManager::kDefaultModel : fileName;

    if (ModelManager::IsPresent(chosen)) {
        return ModelManager::ModelPath(chosen);
    }

    const int answer = ui::ShowMessage(owner,
                                       L"The selected whisper model is not installed.\n\nDownload "
                                       L"it now, or browse for a local .bin file?",
                                       MB_YESNOCANCEL | MB_ICONQUESTION | MB_DEFBUTTON1);
    // Yes = Download, No = Browse, Cancel = abort
    if (answer == IDCANCEL) {
        return {};
    }

    if (answer == IDNO) {
        std::wstring browsed;
        if (!BrowseAndInstall(owner, browsed)) {
            return {};
        }
        return ModelManager::ModelPath(browsed);
    }

    const ModelInfo* info = ModelManager::Find(chosen);
    if (info == nullptr) {
        // Unknown custom name — offer browse instead of inventing a hash.
        std::wstring browsed;
        if (!BrowseAndInstall(owner, browsed)) {
            return {};
        }
        return ModelManager::ModelPath(browsed);
    }

    std::wstring error;
    if (!Download(owner, *info, error)) {
        if (!error.empty()) {
            ui::ShowMessage(owner, error.c_str(), MB_OK | MB_ICONWARNING);
        }
        return {};
    }
    return ModelManager::ModelPath(info->fileName);
}
