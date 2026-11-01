#pragma once

#include "audio/AudioFormat.hpp"
#include "audio/RingBuffer.hpp"

#include <atomic>
#include <cstdint>
#include <vector>

class Mixer {
public:
    static constexpr int64_t kGapSilenceFrames = 240;      // 5 ms
    static constexpr int64_t kStaleFrames = 7200;          // 150 ms
    static constexpr int64_t kDriftThresholdFrames = 960;  // 20 ms
    static constexpr float kLimiterStart = 0.89125093813f; // -1 dBFS

    Mixer() = default;

    void Reset(uint64_t startQpc, uint64_t qpcFrequency, float micGain, float systemGain);

    // Pull from rings, emit one 10 ms stereo block when ready. Returns true if outBlock filled.
    bool Process(RingBuffer& micRing, RingBuffer& sysRing, uint64_t nowQpc, float* outInterleavedStereo);

    // After capture stops: emit remaining covered/stale blocks until rings and timelines are drained.
    bool Flush(RingBuffer& micRing, RingBuffer& sysRing, float* outInterleavedStereo);

    bool HasPending() const;

    float PeakMic() const { return m_peakMic.load(std::memory_order_acquire); }
    float PeakSys() const { return m_peakSys.load(std::memory_order_acquire); }
    float PeakMix() const { return m_peakMix.load(std::memory_order_acquire); }

    // Test helpers / configuration
    void SetGains(float micGain, float systemGain) {
        m_micGain = micGain;
        m_systemGain = systemGain;
    }

private:
    struct SourceBuffer {
        std::vector<float> samples; // interleaved stereo, relative to startFrame
        int64_t startFrame = 0;
        int64_t endFrame = 0;
        double fillAverage = 0.0;
        bool hasData = false;

        void Clear() {
            samples.clear();
            startFrame = 0;
            endFrame = 0;
            fillAverage = 0.0;
            hasData = false;
        }
    };

    void DrainRing(RingBuffer& ring, SourceBuffer& source);
    void IngestPacket(SourceBuffer& source, int64_t frameIndex, const float* data, uint32_t frames);
    void EnsureCapacity(SourceBuffer& source, int64_t frameIndex, uint32_t frames);
    bool BlockReady(int64_t nowFrame) const;
    void ReadSourceBlock(SourceBuffer& source, int64_t blockStart, float* dest);
    void TrimSource(SourceBuffer& source, int64_t keepFrom);
    void ApplyDriftCorrection();
    static float SoftLimit(float sample);

    uint64_t m_startQpc = 0;
    uint64_t m_qpcFrequency = 1;
    float m_micGain = 1.0f;
    float m_systemGain = 0.8f;
    int64_t m_nextBlockFrame = 0;

    SourceBuffer m_mic;
    SourceBuffer m_sys;

    std::vector<float> m_packetScratch;

    std::atomic<float> m_peakMic{0.0f};
    std::atomic<float> m_peakSys{0.0f};
    std::atomic<float> m_peakMix{0.0f};
};
