#include "ui/ShellDialogs.hpp"

#include <shobjidl.h>
#include <vector>
#include <wil/com.h>
#include <wil/resource.h>

namespace ui {

namespace {

auto ShowAndGetPath(IFileDialog* dialog, HWND owner) -> std::optional<std::wstring> {
    if (FAILED(dialog->Show(owner))) {
        return std::nullopt;
    }
    wil::com_ptr_nothrow<IShellItem> item;
    if (FAILED(dialog->GetResult(item.put())) || !item) {
        return std::nullopt;
    }
    wil::unique_cotaskmem_string path;
    if (FAILED(item->GetDisplayName(SIGDN_FILESYSPATH, path.put())) || !path) {
        return std::nullopt;
    }
    return std::wstring(path.get());
}

void ApplyFilters(IFileDialog* dialog, std::span<const FileFilter> filters) {
    if (filters.empty()) {
        return;
    }
    std::vector<COMDLG_FILTERSPEC> specs;
    specs.reserve(filters.size());
    for (const auto& filter : filters) {
        specs.push_back({filter.name, filter.spec});
    }
    dialog->SetFileTypes(static_cast<UINT>(specs.size()), specs.data());
}

} // namespace

auto PickFolder(HWND owner) -> std::optional<std::wstring> {
    wil::com_ptr_nothrow<IFileOpenDialog> dialog;
    const auto hr =
        ::CoCreateInstance(CLSID_FileOpenDialog, nullptr, CLSCTX_ALL, IID_PPV_ARGS(dialog.put()));
    if (FAILED(hr) || !dialog) {
        return std::nullopt;
    }
    DWORD options = 0;
    dialog->GetOptions(&options);
    dialog->SetOptions(options | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM);
    return ShowAndGetPath(dialog.get(), owner);
}

auto PickOpenFile(HWND owner, std::span<const FileFilter> filters, const wchar_t* title)
    -> std::optional<std::wstring> {
    wil::com_ptr_nothrow<IFileOpenDialog> dialog;
    const auto hr = ::CoCreateInstance(CLSID_FileOpenDialog, nullptr, CLSCTX_INPROC_SERVER,
                                       IID_PPV_ARGS(dialog.put()));
    if (FAILED(hr) || !dialog) {
        return std::nullopt;
    }
    ApplyFilters(dialog.get(), filters);
    if (title != nullptr) {
        dialog->SetTitle(title);
    }
    return ShowAndGetPath(dialog.get(), owner);
}

auto PickSaveFile(HWND owner, std::span<const FileFilter> filters, const wchar_t* defaultExt,
                  const std::wstring& fileName) -> std::optional<std::wstring> {
    wil::com_ptr_nothrow<IFileSaveDialog> dialog;
    const auto hr = ::CoCreateInstance(CLSID_FileSaveDialog, nullptr, CLSCTX_INPROC_SERVER,
                                       IID_PPV_ARGS(dialog.put()));
    if (FAILED(hr) || !dialog) {
        return std::nullopt;
    }
    ApplyFilters(dialog.get(), filters);
    if (defaultExt != nullptr) {
        dialog->SetDefaultExtension(defaultExt);
    }
    if (!fileName.empty()) {
        dialog->SetFileName(fileName.c_str());
    }
    return ShowAndGetPath(dialog.get(), owner);
}

} // namespace ui
