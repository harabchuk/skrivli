#pragma once

#include "core/Transcript.hpp"

#include <atomic>
#include <cstdint>
#include <functional>
#include <mutex>
#include <string>
#include <vector>

class WhisperEngine {
public:
    using ProgressFn = std::function<void(int percent)>;

    WhisperEngine() = default;
    ~WhisperEngine();

    WhisperEngine(const WhisperEngine&) = delete;
    auto operator=(const WhisperEngine&) -> WhisperEngine& = delete;

    bool EnsureLoaded(const std::wstring& modelPath, std::wstring& error);
    void Unload();
    void UnloadIfIdle(uint64_t idleMs);

    // pcm16kMono is mono float @ 16 kHz. cancelFlag is polled by abort_callback.
    bool Transcribe(
        const std::vector<float>& pcm16kMono,
        const std::string& language,
        int threads,
        bool useVad,
        const std::wstring& vadModelPath,
        std::atomic_bool& cancelFlag,
        ProgressFn onProgress,
        std::vector<TranscriptSegment>& outSegments,
        std::string& outDetectedLanguage,
        std::wstring& error);

    bool IsLoaded() const;
private:
    struct whisper_context* m_ctx = nullptr;
    std::wstring m_modelPath;
    mutable std::mutex m_mutex;
    uint64_t m_lastUseTick = 0;
};
