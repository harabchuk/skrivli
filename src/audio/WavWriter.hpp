#pragma once

#include <cstdint>
#include <string>
#include <vector>

class WavWriter {
public:
    static constexpr uint32_t kSampleRate = 48000;
    static constexpr uint16_t kChannels = 2;
    static constexpr uint16_t kBitsPerSample = 16;
    static constexpr uint64_t kMinFreeBytes = 200ull * 1024ull * 1024ull; // 200 MB

    WavWriter() = default;
    ~WavWriter();

    WavWriter(const WavWriter&) = delete;
    WavWriter& operator=(const WavWriter&) = delete;

    // Opens `<folder>\MeetingBuddy_YYYY-MM-DD_HH-MM-SS.wav.partial`.
    bool Open(const std::wstring& folder);
    bool WriteFloatFrames(const float* interleavedStereo, uint32_t frameCount);
    bool PatchHeader();
    // Flush, patch, close, rename to `.wav`. On failure leaves the partial.
    bool Finalize(std::wstring& outFinalPath);
    // Close, repair, move into rescueDir as a finalized `.wav`.
    bool RescueTo(const std::wstring& rescueDir, std::wstring& outFinalPath);
    void Abandon();
    // Close and delete the partial (discard without saving).
    void Discard();

    bool IsOpen() const { return m_file != nullptr; }
    const std::wstring& PartialPath() const { return m_partialPath; }
    const std::wstring& Folder() const { return m_folder; }
    uint64_t FramesWritten() const { return m_framesWritten; }

    // Repair RIFF sizes from file length. Used by recovery and tests.
    static bool RepairPartialHeader(const std::wstring& partialPath);
    // Scan folder for `*.wav.partial`, repair, rename to `*.recovered.wav`.
    static std::vector<std::wstring> RecoverPartialsInFolder(const std::wstring& folder);
    static bool FolderHasFreeSpace(const std::wstring& folder, uint64_t minBytes = kMinFreeBytes);
    static bool IsWritableDirectory(const std::wstring& folder);

private:
    bool WriteHeader();
    bool SeekAndWriteSizes();
    static std::wstring MakeBasePath(const std::wstring& folder);
    static std::wstring RecoveredPathFromPartial(const std::wstring& partialPath);

    void* m_file = nullptr; // HANDLE
    std::wstring m_folder;
    std::wstring m_partialPath;
    std::wstring m_finalPath;
    uint64_t m_framesWritten = 0;
    uint64_t m_bytesSincePatch = 0;
};
