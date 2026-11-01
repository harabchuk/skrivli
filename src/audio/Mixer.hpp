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

    // App-side mic AGC: target speech ~-18 dBFS, gate silence, clamp 0.5×–4×.
    static constexpr float kAgcTargetLinear = 0.125892541f; // -18 dBFS
    static constexpr float kAgcGateLinear = 0.00562341325f; // -45 dBFS
    static constexpr float kAgcMinGain = 0.5f;
    static constexpr float kAgcMaxGain = 4.0f;
    static constexpr float kAgcWindowSeconds = 0.75f;
    static constexpr float kAgcAttackPerBlock = 0.35f;  // fast reduce when loud
    static constexpr float kAgcReleasePerBlock = 0.015f; // slow raise when quiet

    Mixer() = default;

    void Reset(uint64_t startQpc, uint64_t qpcFrequency, float micGain, float systemGain, bool micAgc = false);

    // Pull from rings, emit one 10 ms stereo block when ready. Returns true if outBlock filled.
    // Optional stem buffers receive aligned pre-gain mic (mono→stereo) and system audio.
    bool Process(
        RingBuffer& micRing,
        RingBuffer& sysRing,
        uint64_t nowQpc,
        float* outInterleavedStereo,
        float* outMicStem = nullptr,
        float* outSysStem = nullptr);

    // After capture stops: emit remaining covered/stale blocks until rings and timelines are drained.
    bool Flush(
        RingBuffer& micRing,
        RingBuffer& sysRing,
        float* outInterleavedStereo,
        float* outMicStem = nullptr,
        float* outSysStem = nullptr);

    bool HasPending() const;

    float PeakMic() const { return m_peakMic.load(std::memory_order_acquire); }
    float PeakSys() const { return m_peakSys.load(std::memory_order_acquire); }
    float PeakMix() const { return m_peakMix.load(std::memory_order_acquire); }
    float EffectiveMicGain() const { return m_effectiveMicGain.load(std::memory_order_acquire); }

    void SetGains(float micGain, float systemGain);
    void SetMicAgc(bool enabled);

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
    void UpdateAgc(float blockRms);
    float CurrentMicGain() const;
    static float SoftLimit(float sample);

    uint64_t m_startQpc = 0;
    uint64_t m_qpcFrequency = 1;
    float m_micGain = 1.0f;
    float m_systemGain = 0.8f;
    bool m_micAgc = false;
    float m_agcGain = 1.0f;
    float m_agcRms = 0.0f;
    float m_agcPrevGain = 1.0f;
    int64_t m_nextBlockFrame = 0;

    SourceBuffer m_mic;
    SourceBuffer m_sys;

    std::vector<float> m_packetScratch;

    std::atomic<float> m_peakMic{0.0f};
    std::atomic<float> m_peakSys{0.0f};
    std::atomic<float> m_peakMix{0.0f};
    std::atomic<float> m_effectiveMicGain{1.0f};
};
