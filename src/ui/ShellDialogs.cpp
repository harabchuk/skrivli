#include "ui/ShellDialogs.hpp"

#include <shobjidl.h>
#include <wrl/client.h>

#include <vector>

namespace ui {

namespace {

std::optional<std::wstring> ShowAndGetPath(IFileDialog* dialog, HWND owner) {
    if (FAILED(dialog->Show(owner))) {
        return std::nullopt;
    }
    Microsoft::WRL::ComPtr<IShellItem> item;
    if (FAILED(dialog->GetResult(&item)) || !item) {
        return std::nullopt;
    }
    PWSTR path = nullptr;
    if (FAILED(item->GetDisplayName(SIGDN_FILESYSPATH, &path)) || path == nullptr) {
        return std::nullopt;
    }
    std::wstring result = path;
    ::CoTaskMemFree(path);
    return result;
}

void ApplyFilters(IFileDialog* dialog, std::span<const FileFilter> filters) {
    if (filters.empty()) {
        return;
    }
    std::vector<COMDLG_FILTERSPEC> specs;
    specs.reserve(filters.size());
    for (const FileFilter& filter : filters) {
        specs.push_back({filter.name, filter.spec});
    }
    dialog->SetFileTypes(static_cast<UINT>(specs.size()), specs.data());
}

}  // namespace

std::optional<std::wstring> PickFolder(HWND owner) {
    Microsoft::WRL::ComPtr<IFileOpenDialog> dialog;
    const HRESULT hr = ::CoCreateInstance(CLSID_FileOpenDialog, nullptr, CLSCTX_ALL, IID_PPV_ARGS(&dialog));
    if (FAILED(hr) || !dialog) {
        return std::nullopt;
    }
    DWORD options = 0;
    dialog->GetOptions(&options);
    dialog->SetOptions(options | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM);
    return ShowAndGetPath(dialog.Get(), owner);
}

std::optional<std::wstring> PickOpenFile(
    HWND owner,
    std::span<const FileFilter> filters,
    const wchar_t* title) {
    Microsoft::WRL::ComPtr<IFileOpenDialog> dialog;
    const HRESULT hr = ::CoCreateInstance(CLSID_FileOpenDialog, nullptr, CLSCTX_INPROC_SERVER, IID_PPV_ARGS(&dialog));
    if (FAILED(hr) || !dialog) {
        return std::nullopt;
    }
    ApplyFilters(dialog.Get(), filters);
    if (title != nullptr) {
        dialog->SetTitle(title);
    }
    return ShowAndGetPath(dialog.Get(), owner);
}

std::optional<std::wstring> PickSaveFile(
    HWND owner,
    std::span<const FileFilter> filters,
    const wchar_t* defaultExt,
    const std::wstring& fileName) {
    Microsoft::WRL::ComPtr<IFileSaveDialog> dialog;
    const HRESULT hr = ::CoCreateInstance(CLSID_FileSaveDialog, nullptr, CLSCTX_INPROC_SERVER, IID_PPV_ARGS(&dialog));
    if (FAILED(hr) || !dialog) {
        return std::nullopt;
    }
    ApplyFilters(dialog.Get(), filters);
    if (defaultExt != nullptr) {
        dialog->SetDefaultExtension(defaultExt);
    }
    if (!fileName.empty()) {
        dialog->SetFileName(fileName.c_str());
    }
    return ShowAndGetPath(dialog.Get(), owner);
}

}  // namespace ui
