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

struct RecordingSidecar {
    std::string startLocal; // "YYYY-MM-DDTHH:MM:SS"
    std::string endLocal;
    double durationSeconds = 0.0;
    int sampleRate = 48000;
    int channels = 2;
    std::string micDevice;
    std::string systemDevice;
    std::vector<std::string> deviceNotes;
    TranscriptStatus transcriptStatus = TranscriptStatus::None;
    std::string transcriptError;
    std::string model;
    std::string language;
};

namespace Sidecar {

const char* ToString(TranscriptStatus status);
TranscriptStatus FromString(const std::string& text);

bool Load(const std::wstring& jsonPath, RecordingSidecar& out);
bool Save(const std::wstring& jsonPath, const RecordingSidecar& data);

std::string WideToUtf8(const std::wstring& wide);
std::wstring Utf8ToWide(const std::string& utf8);
std::string FormatLocalTimestamp(const SYSTEMTIME& local);
std::string FormatLocalTimestampNow();

}  // namespace Sidecar
