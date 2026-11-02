#pragma once

#include <windows.h>

#include <cstdint>
#include <memory>
#include <string>
#include <vector>
#include <wil/resource.h>

class WavWriter {
  public:
    static constexpr uint32_t kSampleRate = 48000;
    static constexpr uint16_t kChannels = 2;
    static constexpr uint16_t kBitsPerSample = 16;
    static constexpr uint64_t kMinFreeBytes = 200ull * 1024ull * 1024ull; // 200 MB

    WavWriter() = default;
    ~WavWriter();

    WavWriter(const WavWriter&) = delete;
    auto operator=(const WavWriter&) -> WavWriter& = delete;

    // Opens `<folder>\MeetingBuddy_YYYY-MM-DD_HH-MM-SS.wav.partial`.
    bool Open(const std::wstring& folder);
    // Opens `<finalWavPath>.partial` (e.g. stems: `… .mic.wav.partial`).
    bool OpenFinalPath(const std::wstring& finalWavPath);
    bool WriteFloatFrames(const float* interleavedStereo, uint32_t frameCount);
    bool PatchHeader();
    // Flush, patch, close, rename to `.wav`. On failure leaves the partial.
    bool Finalize(std::wstring& outFinalPath);
    // Close, repair, move into rescueDir as a finalized `.wav`.
    bool RescueTo(const std::wstring& rescueDir, std::wstring& outFinalPath);
    void Abandon();
    // Close and delete the partial (discard without saving).
    void Discard();

    bool IsOpen() const {
        return static_cast<bool>(m_file);
    }
    auto PartialPath() const -> const std::wstring& {
        return m_partialPath;
    }
    auto FinalPath() const -> const std::wstring& {
        return m_finalPath;
    }
    auto Folder() const -> const std::wstring& {
        return m_folder;
    }
    uint64_t FramesWritten() const {
        return m_framesWritten;
    }

    // Patch RIFF/`data` sizes from file length (MiniAudio already wrote the header).
    static bool RepairPartialHeader(const std::wstring& partialPath);
    // Scan folder for `*.wav.partial`, repair, rename to `*.recovered.wav`.
    static auto RecoverPartialsInFolder(const std::wstring& folder) -> std::vector<std::wstring>;
    static bool FolderHasFreeSpace(const std::wstring& folder, uint64_t minBytes = kMinFreeBytes);

  private:
    friend struct WavEncoderCallbacks;

    struct WavEncoder;
    struct WavEncoderDeleter {
        void operator()(WavEncoder* encoder) const;
    };

    bool WriteBytes(const void* data, size_t bytes, size_t& bytesWritten);
    bool SeekFile(int64_t offset, int origin);
    // Uninitializes the encoder (it patches RIFF sizes) and then closes the handle.
    void CloseFile(bool flush = false);
    void ResetState();
    static auto MakeBasePath(const std::wstring& folder) -> std::wstring;
    static auto RecoveredPathFromPartial(const std::wstring& partialPath) -> std::wstring;

    std::unique_ptr<WavEncoder, WavEncoderDeleter> m_encoder;
    wil::unique_hfile m_file;
    std::wstring m_folder;
    std::wstring m_partialPath;
    std::wstring m_finalPath;
    uint64_t m_framesWritten = 0;
    uint64_t m_bytesSincePatch = 0;
    std::vector<int16_t> m_pcmScratch;
};
