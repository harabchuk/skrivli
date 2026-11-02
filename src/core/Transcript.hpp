#pragma once

#include <cstdint>
#include <string>

enum class TranscriptStatus {
    None,
    Queued,
    Preparing,
    Transcribing,
    Ready,
    Failed,
    Cancelled,
};

struct TranscriptSegment {
    int64_t t0 = 0; // whisper 10 ms units
    int64_t t1 = 0;
    std::string text;
};

struct TranscriptMeta {
    std::string titleDateTime; // "YYYY-MM-DD HH:MM" local
    double durationSeconds = 0.0;
    std::string recordedRange; // "YYYY-MM-DD HH:MM – HH:MM" or similar
    std::string modelFile;
    std::string languageConfigured; // e.g. "auto"
    std::string languageDetected;   // e.g. "en", empty if unknown
};

inline constexpr wchar_t kDefaultWhisperModel[] = L"ggml-base-q5_1.bin";
