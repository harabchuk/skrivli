#pragma once

#include "core/Recording.hpp"

#include <string>
#include <vector>

class RecordingLibrary {
  public:
    // Runs partial recovery, then scans folder for *.wav / *.m4a.
    static auto Scan(const std::wstring& folder) -> std::vector<RecordingEntry>;

    static bool WriteSidecarForAudio(const std::wstring& audioPath, const RecordingSidecar& data);

    static bool Rename(const RecordingEntry& entry, const std::wstring& newStemName,
                       std::wstring& error);
    static bool DeleteToRecycleBin(const RecordingEntry& entry, std::wstring& error);

    static double ReadWavDurationSeconds(const std::wstring& wavPath);
    static auto JsonPathForAudio(const std::wstring& audioPath) -> std::wstring;
    static auto TranscriptPathForAudio(const std::wstring& audioPath) -> std::wstring;
    static auto LegacyMdPathForAudio(const std::wstring& audioPath) -> std::wstring;
};
