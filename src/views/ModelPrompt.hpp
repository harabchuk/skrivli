#pragma once

#include "pch.hpp"

#include "infrastructure/ai/ModelManager.hpp"

#include <string>

class ModelPrompt {
public:
    static auto EnsureModel(HWND owner, const std::wstring& fileName) -> std::wstring;
    static bool BrowseAndInstall(HWND owner, std::wstring& outFileName);
    static bool Download(HWND owner, const ModelInfo& info, std::wstring& error);
};
