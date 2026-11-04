#pragma once

#include "core/Transcript.hpp"

#include <cstdint>
#include <string>
#include <vector>

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
};

struct RecordingEntry {
    std::wstring audioPath;
    std::wstring jsonPath;
    std::wstring transcriptPath; // .txt, or legacy .md when no .txt exists
    std::wstring stem;           // full path without extension
    std::wstring fileName;       // leaf name of audio
    std::wstring displayName;
    std::wstring startLocal; // for sorting/display
    double durationSeconds = 0.0;
    uint64_t sizeBytes = 0;
    TranscriptStatus transcriptStatus = TranscriptStatus::None;
    std::wstring transcriptError;
    int transcriptPercent = -1; // -1 = n/a; set by UI for active job
    bool hasTranscript = false;
    bool transcriptIsLegacyMarkdown = false;
};
