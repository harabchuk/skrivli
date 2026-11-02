#pragma once

#include <string>
#include <vector>
#include <windows.h>

struct ModelInfo {
    std::wstring fileName;
    std::wstring displayName;
    const char* sha256 = nullptr; // lowercase hex, 64 chars
};

class ModelManager {
public:
    static constexpr wchar_t kDefaultModel[] = L"ggml-base-q5_1.bin";

    static const std::vector<ModelInfo>& OfferedModels();
    static const ModelInfo* Find(const std::wstring& fileName);
    static std::wstring ModelPath(const std::wstring& fileName);
    static bool IsPresent(const std::wstring& fileName);
    static std::wstring VadModelPath();
    static bool VadModelPresent();

    // Ensures the chosen model exists under the models directory.
    // May show Download / Browse UI. Returns absolute path or empty on cancel/failure.
    static std::wstring EnsureModel(HWND owner, const std::wstring& fileName);

    static bool BrowseAndInstall(HWND owner, std::wstring& outFileName);
    static bool Download(HWND owner, const ModelInfo& info, std::wstring& error);

private:
    static bool VerifySha256(const std::wstring& path, const char* expectedHex);
    static std::wstring Sha256Hex(const std::wstring& path);
};
