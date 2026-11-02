#pragma once

#include "storage/RecordingSidecar.hpp"

#include <cstdint>
#include <string>
#include <vector>

struct RecordingEntry {
    std::wstring audioPath;
    std::wstring jsonPath;
    std::wstring mdPath;
    std::wstring stem;      // full path without extension
    std::wstring fileName;  // leaf name of audio
    std::wstring displayName;
    std::wstring startLocal; // for sorting/display
    double durationSeconds = 0.0;
    uint64_t sizeBytes = 0;
    TranscriptStatus transcriptStatus = TranscriptStatus::None;
    std::wstring transcriptError;
    int transcriptPercent = -1; // -1 = n/a; set by UI for active job
    bool hasMarkdown = false;
};

class RecordingLibrary {
public:
    // Runs partial recovery, then scans folder for *.wav / *.m4a.
    static std::vector<RecordingEntry> Scan(const std::wstring& folder);

    static bool WriteSidecarForAudio(
        const std::wstring& audioPath,
        const RecordingSidecar& data);

    static bool Rename(const RecordingEntry& entry, const std::wstring& newStemName, std::wstring& error);
    static bool DeleteToRecycleBin(const RecordingEntry& entry, std::wstring& error);

    static double ReadWavDurationSeconds(const std::wstring& wavPath);
    static std::wstring JsonPathForAudio(const std::wstring& audioPath);
    static std::wstring MdPathForAudio(const std::wstring& audioPath);
};
