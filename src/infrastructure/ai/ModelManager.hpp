#pragma once

#include "core/Transcript.hpp"

#include <atomic>
#include <functional>
#include <string>
#include <vector>

struct ModelInfo {
    std::wstring fileName;
    std::wstring displayName;
    const char* sha256 = nullptr; // lowercase hex, 64 chars
};

class ModelManager {
  public:
    static constexpr const wchar_t* kDefaultModel = kDefaultWhisperModel;

    using ProgressFn = std::function<void(uint64_t done, uint64_t total)>;

    static auto OfferedModels() -> const std::vector<ModelInfo>&;
    static auto Find(const std::wstring& fileName) -> const ModelInfo*;
    static auto ModelPath(const std::wstring& fileName) -> std::wstring;
    static bool IsPresent(const std::wstring& fileName);
    static auto VadModelPath() -> std::wstring;
    static bool VadModelPresent();

    // onProgress runs on the calling thread so a UI can pump messages while bytes arrive.
    static bool DownloadToFile(const ModelInfo& info, std::atomic_bool& cancel,
                               const ProgressFn& onProgress, std::wstring& error);

  private:
    static bool VerifySha256(const std::wstring& path, const char* expectedHex);
    static auto Sha256Hex(const std::wstring& path) -> std::wstring;
};
