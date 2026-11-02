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

    static auto OfferedModels() -> const std::vector<ModelInfo>&;
    static auto Find(const std::wstring& fileName) -> const ModelInfo*;
    static auto ModelPath(const std::wstring& fileName) -> std::wstring;
    static bool IsPresent(const std::wstring& fileName);
    static auto VadModelPath() -> std::wstring;
    static bool VadModelPresent();

    // Ensures the chosen model exists under the models directory.
    // May show Download / Browse UI. Returns absolute path or empty on cancel/failure.
    static auto EnsureModel(HWND owner, const std::wstring& fileName) -> std::wstring;

    static bool BrowseAndInstall(HWND owner, std::wstring& outFileName);
    static bool Download(HWND owner, const ModelInfo& info, std::wstring& error);

private:
    static bool VerifySha256(const std::wstring& path, const char* expectedHex);
    static auto Sha256Hex(const std::wstring& path) -> std::wstring;
};
