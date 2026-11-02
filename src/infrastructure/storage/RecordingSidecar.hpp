#pragma once

#include <windows.h>

#include <cstdint>
#include <string>
#include <vector>

enum class TranscriptStatus {
    None,
    Queued,
    Preparing,
    Transcribing,
    Ready,
    Failed,
    Cancelled,
};

// Mapped directly by Glaze (field names == JSON keys). See RecordingSidecar.cpp
// for TranscriptStatus string names ("none", "queued", ...).
struct RecordingSidecar {
    std::string startLocal{}; // "YYYY-MM-DDTHH:MM:SS"
    std::string endLocal{};
    double durationSeconds = 0.0;
    int sampleRate = 48000;
    int channels = 2;
    std::string micDevice{};
    std::string systemDevice{};
    std::vector<std::string> deviceNotes{};
    TranscriptStatus transcriptStatus = TranscriptStatus::None;
    std::string transcriptError{};
    std::string model{};
    std::string language{};

    // start/end = now, duration read from the WAV header, internal sample rate and channels.
    static RecordingSidecar ForAudio(const std::wstring& audioPath);
};

namespace Sidecar {

bool Load(const std::wstring& jsonPath, RecordingSidecar& out);
bool Save(const std::wstring& jsonPath, const RecordingSidecar& data);

auto FormatLocalTimestamp(const SYSTEMTIME& local) -> std::string;
auto FormatLocalTimestampNow() -> std::string;

}  // namespace Sidecar
