#pragma once

#include <cstdint>
#include <string>

class WavWriter {
public:
    static constexpr uint32_t kSampleRate = 48000;
    static constexpr uint16_t kChannels = 2;
    static constexpr uint16_t kBitsPerSample = 16;

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
    void Abandon();

    bool IsOpen() const { return m_file != nullptr; }
    const std::wstring& PartialPath() const { return m_partialPath; }
    uint64_t FramesWritten() const { return m_framesWritten; }

    // Repair RIFF sizes from file length. Used by M4 recovery and tests.
    static bool RepairPartialHeader(const std::wstring& partialPath);

private:
    bool WriteHeader();
    bool SeekAndWriteSizes();
    static std::wstring MakeBasePath(const std::wstring& folder);
    static bool IsWritableDirectory(const std::wstring& folder);

    void* m_file = nullptr; // HANDLE
    std::wstring m_partialPath;
    std::wstring m_finalPath;
    uint64_t m_framesWritten = 0;
    uint64_t m_bytesSincePatch = 0;
};
