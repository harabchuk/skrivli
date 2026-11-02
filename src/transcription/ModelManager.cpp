#include "pch.hpp"
#include "transcription/ModelManager.hpp"

#include "storage/RecordingSidecar.hpp"
#include "util/FileIo.hpp"
#include "util/Log.hpp"
#include "util/Paths.hpp"
#include "util/Text.hpp"

#include <bcrypt.h>
#include <commctrl.h>
#include <shobjidl.h>
#include <winhttp.h>
#include <wrl/client.h>

#include <atomic>
#include <algorithm>
#include <cstdio>
#include <filesystem>
#include <fstream>
#include <string>
#include <vector>

#pragma comment(lib, "bcrypt.lib")
#pragma comment(lib, "winhttp.lib")

namespace {

constexpr wchar_t kHost[] = L"huggingface.co";
constexpr wchar_t kPathPrefix[] = L"/ggerganov/whisper.cpp/resolve/main/";

std::string ToLowerHex(const BYTE* data, DWORD len) {
    static const char* kDigits = "0123456789abcdef";
    std::string out;
    out.resize(static_cast<size_t>(len) * 2);
    for (DWORD i = 0; i < len; ++i) {
        out[i * 2] = kDigits[(data[i] >> 4) & 0xF];
        out[i * 2 + 1] = kDigits[data[i] & 0xF];
    }
    return out;
}

struct DownloadProgress {
    HWND dialog = nullptr;
    HWND bar = nullptr;
    std::atomic_bool* cancel = nullptr;
    uint64_t total = 0;
    uint64_t done = 0;
};

LRESULT CALLBACK DownloadWndProc(HWND hwnd, UINT msg, WPARAM wParam, LPARAM lParam) {
    if (msg == WM_CREATE) {
        auto* cs = reinterpret_cast<CREATESTRUCTW*>(lParam);
        ::SetWindowLongPtrW(hwnd, GWLP_USERDATA, reinterpret_cast<LONG_PTR>(cs->lpCreateParams));
        return 0;
    }
    if (msg == WM_COMMAND && LOWORD(wParam) == IDCANCEL) {
        auto* state = reinterpret_cast<DownloadProgress*>(::GetWindowLongPtrW(hwnd, GWLP_USERDATA));
        if (state != nullptr && state->cancel != nullptr) {
            state->cancel->store(true);
        }
        return 0;
    }
    if (msg == WM_CLOSE) {
        auto* state = reinterpret_cast<DownloadProgress*>(::GetWindowLongPtrW(hwnd, GWLP_USERDATA));
        if (state != nullptr && state->cancel != nullptr) {
            state->cancel->store(true);
        }
        return 0;
    }
    return ::DefWindowProcW(hwnd, msg, wParam, lParam);
}

HWND CreateDownloadDialog(HWND owner, DownloadProgress* state) {
    const wchar_t* className = L"MeetingBuddy.DownloadDlg";
    WNDCLASSW wc{};
    if (!::GetClassInfoW(_Module.GetModuleInstance(), className, &wc)) {
        wc.lpfnWndProc = DownloadWndProc;
        wc.hInstance = _Module.GetModuleInstance();
        wc.lpszClassName = className;
        wc.hCursor = ::LoadCursor(nullptr, IDC_ARROW);
        wc.hbrBackground = reinterpret_cast<HBRUSH>(COLOR_WINDOW + 1);
        ::RegisterClassW(&wc);
    }

    HWND dlg = ::CreateWindowExW(
        WS_EX_DLGMODALFRAME,
        className,
        L"Downloading model",
        WS_POPUP | WS_CAPTION | WS_SYSMENU,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        420,
        140,
        owner,
        nullptr,
        _Module.GetModuleInstance(),
        state);
    if (dlg == nullptr) {
        return nullptr;
    }

    ::CreateWindowExW(
        0,
        L"STATIC",
        L"Downloading whisper model…",
        WS_CHILD | WS_VISIBLE,
        16,
        16,
        370,
        20,
        dlg,
        nullptr,
        _Module.GetModuleInstance(),
        nullptr);

    HWND bar = ::CreateWindowExW(
        0,
        PROGRESS_CLASSW,
        L"",
        WS_CHILD | WS_VISIBLE,
        16,
        48,
        370,
        18,
        dlg,
        nullptr,
        _Module.GetModuleInstance(),
        nullptr);
    ::SendMessageW(bar, PBM_SETRANGE, 0, MAKELPARAM(0, 1000));
    state->bar = bar;

    ::CreateWindowExW(
        0,
        L"BUTTON",
        L"Cancel",
        WS_CHILD | WS_VISIBLE | BS_PUSHBUTTON,
        300,
        78,
        86,
        24,
        dlg,
        reinterpret_cast<HMENU>(static_cast<INT_PTR>(IDCANCEL)),
        _Module.GetModuleInstance(),
        nullptr);

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

bool HttpDownloadToFile(
    const std::wstring& fileName,
    const std::wstring& destPath,
    DownloadProgress* progress,
    std::wstring& error) {
    const std::wstring partial = destPath + L".partial";
    uint64_t existing = FileIo::FileSize(partial);

    HINTERNET session = ::WinHttpOpen(
        L"MeetingBuddy/1.0",
        WINHTTP_ACCESS_TYPE_DEFAULT_PROXY,
        WINHTTP_NO_PROXY_NAME,
        WINHTTP_NO_PROXY_BYPASS,
        0);
    if (session == nullptr) {
        error = L"WinHttpOpen failed.";
        return false;
    }

    HINTERNET connect = ::WinHttpConnect(session, kHost, INTERNET_DEFAULT_HTTPS_PORT, 0);
    if (connect == nullptr) {
        error = L"WinHttpConnect failed.";
        ::WinHttpCloseHandle(session);
        return false;
    }

    const std::wstring object = std::wstring(kPathPrefix) + fileName;
    HINTERNET request = ::WinHttpOpenRequest(
        connect,
        L"GET",
        object.c_str(),
        nullptr,
        WINHTTP_NO_REFERER,
        WINHTTP_DEFAULT_ACCEPT_TYPES,
        WINHTTP_FLAG_SECURE);
    if (request == nullptr) {
        error = L"WinHttpOpenRequest failed.";
        ::WinHttpCloseHandle(connect);
        ::WinHttpCloseHandle(session);
        return false;
    }

    std::wstring headers;
    if (existing > 0) {
        wchar_t range[128]{};
        std::swprintf(range, 128, L"Range: bytes=%llu-\r\n", static_cast<unsigned long long>(existing));
        headers = range;
    }

    BOOL sent = ::WinHttpSendRequest(
        request,
        headers.empty() ? WINHTTP_NO_ADDITIONAL_HEADERS : headers.c_str(),
        headers.empty() ? 0 : static_cast<DWORD>(-1),
        WINHTTP_NO_REQUEST_DATA,
        0,
        0,
        0);
    if (!sent || !::WinHttpReceiveResponse(request, nullptr)) {
        error = L"HTTP request failed.";
        ::WinHttpCloseHandle(request);
        ::WinHttpCloseHandle(connect);
        ::WinHttpCloseHandle(session);
        return false;
    }

    DWORD status = 0;
    DWORD statusSize = sizeof(status);
    ::WinHttpQueryHeaders(
        request,
        WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
        WINHTTP_HEADER_NAME_BY_INDEX,
        &status,
        &statusSize,
        WINHTTP_NO_HEADER_INDEX);

    if (status != 200 && status != 206) {
        error = L"Model download HTTP status " + std::to_wstring(status);
        ::WinHttpCloseHandle(request);
        ::WinHttpCloseHandle(connect);
        ::WinHttpCloseHandle(session);
        return false;
    }

    wchar_t lengthBuf[64]{};
    DWORD lengthSize = sizeof(lengthBuf);
    uint64_t contentLength = 0;
    if (::WinHttpQueryHeaders(
            request,
            WINHTTP_QUERY_CONTENT_LENGTH,
            WINHTTP_HEADER_NAME_BY_INDEX,
            lengthBuf,
            &lengthSize,
            WINHTTP_NO_HEADER_INDEX)) {
        contentLength = _wcstoui64(lengthBuf, nullptr, 10);
    }

    if (progress != nullptr) {
        progress->done = existing;
        progress->total = existing + contentLength;
        if (progress->total == 0) {
            progress->total = 1;
        }
    }

    const DWORD createDisp = (existing > 0 && status == 206) ? OPEN_ALWAYS : CREATE_ALWAYS;
    HANDLE file = ::CreateFileW(
        partial.c_str(),
        GENERIC_WRITE,
        FILE_SHARE_READ,
        nullptr,
        createDisp,
        FILE_ATTRIBUTE_NORMAL,
        nullptr);
    if (file == INVALID_HANDLE_VALUE) {
        error = L"Could not create download file.";
        ::WinHttpCloseHandle(request);
        ::WinHttpCloseHandle(connect);
        ::WinHttpCloseHandle(session);
        return false;
    }
    if (existing > 0 && status == 206) {
        LARGE_INTEGER move{};
        move.QuadPart = static_cast<LONGLONG>(existing);
        ::SetFilePointerEx(file, move, nullptr, FILE_BEGIN);
    } else {
        existing = 0;
        if (progress != nullptr) {
            progress->done = 0;
            progress->total = contentLength > 0 ? contentLength : 1;
        }
    }

    std::vector<BYTE> buffer(64 * 1024);
    bool ok = true;
    for (;;) {
        if (progress != nullptr && progress->cancel != nullptr && progress->cancel->load()) {
            error = L"Download cancelled.";
            ok = false;
            break;
        }

        DWORD available = 0;
        if (!::WinHttpQueryDataAvailable(request, &available)) {
            error = L"WinHttpQueryDataAvailable failed.";
            ok = false;
            break;
        }
        if (available == 0) {
            break;
        }

        DWORD toRead = (std::min)(available, static_cast<DWORD>(buffer.size()));
        DWORD read = 0;
        if (!::WinHttpReadData(request, buffer.data(), toRead, &read) || read == 0) {
            error = L"WinHttpReadData failed.";
            ok = false;
            break;
        }

        DWORD written = 0;
        if (!::WriteFile(file, buffer.data(), read, &written, nullptr) || written != read) {
            error = L"Failed writing download.";
            ok = false;
            break;
        }

        if (progress != nullptr) {
            progress->done += read;
            if (progress->bar != nullptr && progress->total > 0) {
                const int pos = static_cast<int>((progress->done * 1000ull) / progress->total);
                ::SendMessageW(progress->bar, PBM_SETPOS, pos, 0);
            }
            PumpMessages();
        }
    }

    ::CloseHandle(file);
    ::WinHttpCloseHandle(request);
    ::WinHttpCloseHandle(connect);
    ::WinHttpCloseHandle(session);

    if (!ok) {
        return false;
    }

    std::error_code ec;
    std::filesystem::rename(partial, destPath, ec);
    if (ec) {
        std::filesystem::remove(destPath, ec);
        std::filesystem::rename(partial, destPath, ec);
        if (ec) {
            error = L"Could not finalize downloaded model.";
            return false;
        }
    }
    return true;
}

}  // namespace

const std::vector<ModelInfo>& ModelManager::OfferedModels() {
    // SHA-256 digests from Hugging Face LFS (ggerganov/whisper.cpp).
    static const std::vector<ModelInfo> kModels = {
        {L"ggml-tiny.bin", L"tiny", "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21"},
        {L"ggml-tiny.en.bin", L"tiny.en", "921e4cf8686fdd993dcd081a5da5b6c365bfde1162e72b08d75ac75289920b1f"},
        {L"ggml-tiny-q5_1.bin", L"tiny-q5_1", "818710568da3ca15689e31a743197b520007872ff9576237bda97bd1b469c3d7"},
        {L"ggml-tiny.en-q5_1.bin", L"tiny.en-q5_1", "c77c5766f1cef09b6b7d47f21b546cbddd4157886b3b5d6d4f709e91e66c7c2b"},
        {L"ggml-base.bin", L"base", "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe"},
        {L"ggml-base.en.bin", L"base.en", "a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002"},
        {L"ggml-base-q5_1.bin", L"base-q5_1 (default)", "422f1ae452ade6f30a004d7e5c6a43195e4433bc370bf23fac9cc591f01a8898"},
        {L"ggml-base.en-q5_1.bin", L"base.en-q5_1", "4baf70dd0d7c4247ba2b81fafd9c01005ac77c2f9ef064e00dcf195d0e2fdd2f"},
        {L"ggml-small.bin", L"small", "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b"},
        {L"ggml-small.en.bin", L"small.en", "c6138d6d58ecc8322097e0f987c32f1be8bb0a18532a3f88f734d1bbf9c41e5d"},
        {L"ggml-small-q5_1.bin", L"small-q5_1", "ae85e4a935d7a567bd102fe55afc16bb595bdb618e11b2fc7591bc08120411bb"},
        {L"ggml-small.en-q5_1.bin", L"small.en-q5_1", "bfdff4894dcb76bbf647d56263ea2a96645423f1669176f4844a1bf8e478ad30"},
    };
    return kModels;
}

const ModelInfo* ModelManager::Find(const std::wstring& fileName) {
    for (const auto& model : OfferedModels()) {
        if (_wcsicmp(model.fileName.c_str(), fileName.c_str()) == 0) {
            return &model;
        }
    }
    return nullptr;
}

std::wstring ModelManager::ModelPath(const std::wstring& fileName) {
    return (std::filesystem::path(Paths::ModelsDir()) / fileName).wstring();
}

bool ModelManager::IsPresent(const std::wstring& fileName) {
    return std::filesystem::exists(ModelPath(fileName));
}

std::wstring ModelManager::VadModelPath() {
    return (std::filesystem::path(Paths::ModelsDir()) / L"ggml-silero-v5.1.2.bin").wstring();
}

bool ModelManager::VadModelPresent() {
    return std::filesystem::exists(VadModelPath());
}

std::wstring ModelManager::Sha256Hex(const std::wstring& path) {
    std::ifstream file(path, std::ios::binary);
    if (!file) {
        return {};
    }

    BCRYPT_ALG_HANDLE alg = nullptr;
    BCRYPT_HASH_HANDLE hash = nullptr;
    DWORD objectLen = 0;
    DWORD dataLen = 0;
    DWORD hashLen = 0;

    if (FAILED(::BCryptOpenAlgorithmProvider(&alg, BCRYPT_SHA256_ALGORITHM, nullptr, 0))) {
        return {};
    }
    ::BCryptGetProperty(alg, BCRYPT_OBJECT_LENGTH, reinterpret_cast<PUCHAR>(&objectLen), sizeof(objectLen), &dataLen, 0);
    ::BCryptGetProperty(alg, BCRYPT_HASH_LENGTH, reinterpret_cast<PUCHAR>(&hashLen), sizeof(hashLen), &dataLen, 0);

    std::vector<BYTE> object(objectLen);
    std::vector<BYTE> digest(hashLen);
    if (FAILED(::BCryptCreateHash(alg, &hash, object.data(), objectLen, nullptr, 0, 0))) {
        ::BCryptCloseAlgorithmProvider(alg, 0);
        return {};
    }

    std::vector<char> buffer(1024 * 1024);
    while (file) {
        file.read(buffer.data(), static_cast<std::streamsize>(buffer.size()));
        const std::streamsize got = file.gcount();
        if (got <= 0) {
            break;
        }
        ::BCryptHashData(hash, reinterpret_cast<PUCHAR>(buffer.data()), static_cast<ULONG>(got), 0);
    }

    ::BCryptFinishHash(hash, digest.data(), hashLen, 0);
    ::BCryptDestroyHash(hash);
    ::BCryptCloseAlgorithmProvider(alg, 0);
    return Text::Utf8ToWide(ToLowerHex(digest.data(), hashLen));
}

bool ModelManager::VerifySha256(const std::wstring& path, const char* expectedHex) {
    if (expectedHex == nullptr) {
        return false;
    }
    const std::wstring actual = Sha256Hex(path);
    const std::wstring expected = Text::Utf8ToWide(expectedHex);
    return _wcsicmp(actual.c_str(), expected.c_str()) == 0;
}

bool ModelManager::BrowseAndInstall(HWND owner, std::wstring& outFileName) {
    Microsoft::WRL::ComPtr<IFileOpenDialog> dialog;
    HRESULT hr = ::CoCreateInstance(CLSID_FileOpenDialog, nullptr, CLSCTX_INPROC_SERVER, IID_PPV_ARGS(&dialog));
    if (FAILED(hr) || !dialog) {
        return false;
    }

    COMDLG_FILTERSPEC filters[] = {
        {L"Whisper models (*.bin)", L"*.bin"},
        {L"All files (*.*)", L"*.*"},
    };
    dialog->SetFileTypes(2, filters);
    dialog->SetTitle(L"Select a whisper ggml model");
    hr = dialog->Show(owner);
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

    PWSTR path = nullptr;
    hr = item->GetDisplayName(SIGDN_FILESYSPATH, &path);
    if (FAILED(hr) || path == nullptr) {
        return false;
    }
    const std::wstring source = path;
    ::CoTaskMemFree(path);

    Paths::EnsureDirectory(Paths::ModelsDir());
    const std::wstring leaf = std::filesystem::path(source).filename().wstring();
    const std::wstring dest = ModelPath(leaf);
    std::error_code ec;
    if (!Text::PathEquals(source, dest)) {
        std::filesystem::copy_file(source, dest, std::filesystem::copy_options::overwrite_existing, ec);
        if (ec) {
            ::MessageBoxW(owner, L"Could not copy model into the models folder.", L"MeetingBuddy", MB_OK | MB_ICONWARNING);
            return false;
        }
    }
    outFileName = leaf;
    return true;
}

bool ModelManager::Download(HWND owner, const ModelInfo& info, std::wstring& error) {
    Paths::EnsureDirectory(Paths::ModelsDir());
    const std::wstring dest = ModelPath(info.fileName);

    std::atomic_bool cancel{false};
    DownloadProgress progress{};
    progress.cancel = &cancel;
    progress.dialog = CreateDownloadDialog(owner, &progress);
    if (owner != nullptr) {
        ::EnableWindow(owner, FALSE);
    }

    const bool ok = HttpDownloadToFile(info.fileName, dest, &progress, error);

    if (progress.dialog != nullptr) {
        ::DestroyWindow(progress.dialog);
    }
    if (owner != nullptr) {
        ::EnableWindow(owner, TRUE);
        ::SetForegroundWindow(owner);
    }

    if (!ok) {
        std::error_code ec;
        std::filesystem::remove(dest + L".partial", ec);
        std::filesystem::remove(dest, ec);
        return false;
    }

    if (!VerifySha256(dest, info.sha256)) {
        error = L"SHA-256 mismatch; download deleted.";
        std::error_code ec;
        std::filesystem::remove(dest, ec);
        Log::Error(L"SHA-256 mismatch for " + info.fileName);
        return false;
    }
    return true;
}

std::wstring ModelManager::EnsureModel(HWND owner, const std::wstring& fileName) {
    Paths::EnsureDirectory(Paths::ModelsDir());
    std::wstring chosen = fileName.empty() ? L"ggml-base-q5_1.bin" : fileName;

    if (IsPresent(chosen)) {
        return ModelPath(chosen);
    }

    const int answer = ::MessageBoxW(
        owner,
        L"The selected whisper model is not installed.\n\nDownload it now, or browse for a local .bin file?",
        L"MeetingBuddy",
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
        return ModelPath(browsed);
    }

    const ModelInfo* info = Find(chosen);
    if (info == nullptr) {
        // Unknown custom name — offer browse instead of inventing a hash.
        std::wstring browsed;
        if (!BrowseAndInstall(owner, browsed)) {
            return {};
        }
        return ModelPath(browsed);
    }

    std::wstring error;
    if (!Download(owner, *info, error)) {
        if (!error.empty()) {
            ::MessageBoxW(owner, error.c_str(), L"MeetingBuddy", MB_OK | MB_ICONWARNING);
        }
        return {};
    }
    return ModelPath(info->fileName);
}
