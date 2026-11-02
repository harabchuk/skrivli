#pragma once

#include "pch.hpp"

#include <optional>
#include <span>
#include <string>

namespace ui {

struct FileFilter {
    const wchar_t* name;
    const wchar_t* spec;
};

auto PickFolder(HWND owner) -> std::optional<std::wstring>;

auto PickOpenFile(HWND owner, std::span<const FileFilter> filters, const wchar_t* title)
    -> std::optional<std::wstring>;

auto PickSaveFile(HWND owner, std::span<const FileFilter> filters, const wchar_t* defaultExt,
                  const std::wstring& fileName) -> std::optional<std::wstring>;

} // namespace ui
