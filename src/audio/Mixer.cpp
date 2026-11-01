#include "audio/Mixer.hpp"

#include <algorithm>
#include <cmath>

void Mixer::Reset(uint64_t startQpc, uint64_t qpcFrequency, float micGain, float systemGain) {
    m_startQpc = startQpc;
    m_qpcFrequency = qpcFrequency == 0 ? 1 : qpcFrequency;
    m_micGain = micGain;
    m_systemGain = systemGain;
    m_nextBlockFrame = 0;
    m_mic.Clear();
    m_sys.Clear();
    m_packetScratch.resize(4096 * AudioFormat::kChannels);
    m_peakMic.store(0.0f, std::memory_order_relaxed);
    m_peakSys.store(0.0f, std::memory_order_relaxed);
    m_peakMix.store(0.0f, std::memory_order_relaxed);
}

void Mixer::EnsureCapacity(SourceBuffer& source, int64_t frameIndex, uint32_t frames) {
    const int64_t neededEnd = frameIndex + static_cast<int64_t>(frames);
    if (!source.hasData) {
        source.startFrame = frameIndex;
        source.endFrame = frameIndex;
        source.hasData = true;
    }

    if (frameIndex < source.startFrame) {
        const int64_t pad = source.startFrame - frameIndex;
        source.samples.insert(source.samples.begin(), static_cast<size_t>(pad) * AudioFormat::kChannels, 0.0f);
        source.startFrame = frameIndex;
    }

    if (neededEnd > source.endFrame) {
        const size_t needSamples = static_cast<size_t>(neededEnd - source.startFrame) * AudioFormat::kChannels;
        if (source.samples.size() < needSamples) {
            source.samples.resize(needSamples, 0.0f);
        }
        source.endFrame = neededEnd;
    }
}

void Mixer::IngestPacket(SourceBuffer& source, int64_t frameIndex, const float* data, uint32_t frames) {
    if (data == nullptr || frames == 0) {
        return;
    }

    if (source.hasData) {
        if (frameIndex > source.endFrame) {
            const int64_t gap = frameIndex - source.endFrame;
            if (gap > kGapSilenceFrames) {
                EnsureCapacity(source, source.endFrame, static_cast<uint32_t>(gap));
                // zeros already from resize/fill
            } else if (gap > 0) {
                EnsureCapacity(source, source.endFrame, static_cast<uint32_t>(gap));
            }
        } else if (frameIndex + static_cast<int64_t>(frames) <= source.endFrame
                   && frameIndex < source.endFrame) {
            // Fully overlapped / late duplicate — drop.
            if (frameIndex + static_cast<int64_t>(frames) <= source.endFrame) {
                const int64_t overlap = source.endFrame - frameIndex;
                if (overlap >= static_cast<int64_t>(frames)) {
                    return;
                }
            }
        }
    }

    int64_t writeFrame = frameIndex;
    uint32_t writeFrames = frames;
    const float* writeData = data;

    if (source.hasData && writeFrame < source.endFrame) {
        const int64_t skip = source.endFrame - writeFrame;
        if (skip >= static_cast<int64_t>(writeFrames)) {
            return;
        }
        writeFrame = source.endFrame;
        writeData += static_cast<size_t>(skip) * AudioFormat::kChannels;
        writeFrames -= static_cast<uint32_t>(skip);
    }

    EnsureCapacity(source, writeFrame, writeFrames);
    float* dest = source.samples.data()
        + static_cast<size_t>(writeFrame - source.startFrame) * AudioFormat::kChannels;
    std::copy(
        writeData,
        writeData + static_cast<size_t>(writeFrames) * AudioFormat::kChannels,
        dest);
    source.endFrame = (std::max)(source.endFrame, writeFrame + static_cast<int64_t>(writeFrames));
}

void Mixer::DrainRing(RingBuffer& ring, SourceBuffer& source) {
    for (;;) {
        uint32_t frames = 0;
        uint64_t qpc = 0;
        uint32_t flags = 0;
        if (m_packetScratch.size() < 8192 * AudioFormat::kChannels) {
            m_packetScratch.resize(8192 * AudioFormat::kChannels);
        }
        if (!ring.Pop(m_packetScratch.data(), 8192, frames, qpc, flags)) {
            break;
        }
        if (qpc < m_startQpc) {
            continue;
        }
        const int64_t frameIndex = AudioFormat::QpcToFrames(qpc - m_startQpc, m_qpcFrequency);
        IngestPacket(source, frameIndex, m_packetScratch.data(), frames);
        (void)flags;
    }

    ring.ConsumeOverrun();
}

bool Mixer::BlockReady(int64_t nowFrame) const {
    const int64_t blockEnd = m_nextBlockFrame + AudioFormat::kBlockFrames;
    const bool micCovered = m_mic.hasData && m_mic.endFrame >= blockEnd;
    const bool sysCovered = m_sys.hasData && m_sys.endFrame >= blockEnd;
    if (micCovered && sysCovered) {
        return true;
    }
    return nowFrame - m_nextBlockFrame >= kStaleFrames;
}

void Mixer::ReadSourceBlock(SourceBuffer& source, int64_t blockStart, float* dest) {
    const int64_t blockEnd = blockStart + AudioFormat::kBlockFrames;
    std::fill(dest, dest + AudioFormat::kBlockFrames * AudioFormat::kChannels, 0.0f);
    if (!source.hasData) {
        return;
    }

    const int64_t copyStart = (std::max)(blockStart, source.startFrame);
    const int64_t copyEnd = (std::min)(blockEnd, source.endFrame);
    if (copyStart >= copyEnd) {
        return;
    }

    const int64_t destOffset = copyStart - blockStart;
    const int64_t srcOffset = copyStart - source.startFrame;
    const size_t sampleCount = static_cast<size_t>(copyEnd - copyStart) * AudioFormat::kChannels;
    std::copy(
        source.samples.data() + static_cast<size_t>(srcOffset) * AudioFormat::kChannels,
        source.samples.data() + static_cast<size_t>(srcOffset) * AudioFormat::kChannels + sampleCount,
        dest + static_cast<size_t>(destOffset) * AudioFormat::kChannels);
}

void Mixer::TrimSource(SourceBuffer& source, int64_t keepFrom) {
    if (!source.hasData || keepFrom <= source.startFrame) {
        return;
    }
    if (keepFrom >= source.endFrame) {
        source.Clear();
        return;
    }

    const size_t dropSamples = static_cast<size_t>(keepFrom - source.startFrame) * AudioFormat::kChannels;
    source.samples.erase(source.samples.begin(), source.samples.begin() + static_cast<std::ptrdiff_t>(dropSamples));
    source.startFrame = keepFrom;
}

void Mixer::ApplyDriftCorrection() {
    if (!m_mic.hasData || !m_sys.hasData) {
        return;
    }

    const double micFill = static_cast<double>(m_mic.endFrame - m_nextBlockFrame);
    const double sysFill = static_cast<double>(m_sys.endFrame - m_nextBlockFrame);
    constexpr double alpha = 0.05;
    m_mic.fillAverage = (1.0 - alpha) * m_mic.fillAverage + alpha * micFill;
    m_sys.fillAverage = (1.0 - alpha) * m_sys.fillAverage + alpha * sysFill;

    const double delta = m_mic.fillAverage - m_sys.fillAverage;
    if (std::abs(delta) <= static_cast<double>(kDriftThresholdFrames)) {
        return;
    }

    auto dropOneFrame = [](SourceBuffer& source, int64_t atFrame) {
        if (!source.hasData || atFrame < source.startFrame || atFrame + 1 >= source.endFrame) {
            return;
        }
        const size_t index = static_cast<size_t>(atFrame - source.startFrame) * AudioFormat::kChannels;
        source.samples.erase(
            source.samples.begin() + static_cast<std::ptrdiff_t>(index),
            source.samples.begin() + static_cast<std::ptrdiff_t>(index + AudioFormat::kChannels));
        source.endFrame -= 1;
    };

    auto duplicateOneFrame = [](SourceBuffer& source, int64_t atFrame) {
        if (!source.hasData || atFrame < source.startFrame || atFrame >= source.endFrame) {
            return;
        }
        const size_t index = static_cast<size_t>(atFrame - source.startFrame) * AudioFormat::kChannels;
        const float l = source.samples[index];
        const float r = source.samples[index + 1];
        source.samples.insert(
            source.samples.begin() + static_cast<std::ptrdiff_t>(index + AudioFormat::kChannels),
            {l, r});
        source.endFrame += 1;
    };

    if (delta > static_cast<double>(kDriftThresholdFrames)) {
        // Mic ahead of system — drop one mic frame (or duplicate sys if mic is empty).
        dropOneFrame(m_mic, m_nextBlockFrame);
        m_mic.fillAverage -= 1.0;
    } else {
        // System ahead of mic — drop one system frame.
        dropOneFrame(m_sys, m_nextBlockFrame);
        m_sys.fillAverage -= 1.0;
    }

    // Keep the helper available for the opposite correction direction in tests/tuning.
    (void)duplicateOneFrame;
}

float Mixer::SoftLimit(float sample) {
    const float absSample = std::fabs(sample);
    if (absSample <= kLimiterStart) {
        return sample;
    }
    const float sign = sample < 0.0f ? -1.0f : 1.0f;
    const float over = absSample - kLimiterStart;
    const float limited = kLimiterStart + std::tanh(over / (1.0f - kLimiterStart)) * (1.0f - kLimiterStart);
    return sign * (std::min)(limited, 0.999f);
}

bool Mixer::HasPending() const {
    const bool micPending = m_mic.hasData && m_mic.endFrame > m_nextBlockFrame;
    const bool sysPending = m_sys.hasData && m_sys.endFrame > m_nextBlockFrame;
    return micPending || sysPending;
}

bool Mixer::Process(
    RingBuffer& micRing,
    RingBuffer& sysRing,
    uint64_t nowQpc,
    float* outInterleavedStereo,
    float* outMicStem,
    float* outSysStem) {
    if (outInterleavedStereo == nullptr) {
        return false;
    }

    DrainRing(micRing, m_mic);
    DrainRing(sysRing, m_sys);
    ApplyDriftCorrection();

    const int64_t nowFrame = (nowQpc >= m_startQpc)
        ? AudioFormat::QpcToFrames(nowQpc - m_startQpc, m_qpcFrequency)
        : 0;
    if (!BlockReady(nowFrame)) {
        return false;
    }

    float micBlock[AudioFormat::kBlockFrames * AudioFormat::kChannels];
    float sysBlock[AudioFormat::kBlockFrames * AudioFormat::kChannels];
    ReadSourceBlock(m_mic, m_nextBlockFrame, micBlock);
    ReadSourceBlock(m_sys, m_nextBlockFrame, sysBlock);

    float micPeak = 0.0f;
    float sysPeak = 0.0f;
    float mixPeak = 0.0f;

    for (uint32_t i = 0; i < AudioFormat::kBlockFrames; ++i) {
        const float micL = micBlock[i * 2];
        const float micR = micBlock[i * 2 + 1];
        const float micMono = 0.5f * (micL + micR);
        const float sysL = sysBlock[i * 2];
        const float sysR = sysBlock[i * 2 + 1];

        micPeak = (std::max)(micPeak, std::fabs(micMono));
        sysPeak = (std::max)(sysPeak, (std::max)(std::fabs(sysL), std::fabs(sysR)));

        if (outMicStem != nullptr) {
            outMicStem[i * 2] = SoftLimit(micMono);
            outMicStem[i * 2 + 1] = SoftLimit(micMono);
        }
        if (outSysStem != nullptr) {
            outSysStem[i * 2] = SoftLimit(sysL);
            outSysStem[i * 2 + 1] = SoftLimit(sysR);
        }

        float outL = m_micGain * micMono + m_systemGain * sysL;
        float outR = m_micGain * micMono + m_systemGain * sysR;
        outL = SoftLimit(outL);
        outR = SoftLimit(outR);
        outInterleavedStereo[i * 2] = outL;
        outInterleavedStereo[i * 2 + 1] = outR;
        mixPeak = (std::max)(mixPeak, (std::max)(std::fabs(outL), std::fabs(outR)));
    }

    m_peakMic.store(micPeak, std::memory_order_release);
    m_peakSys.store(sysPeak, std::memory_order_release);
    m_peakMix.store(mixPeak, std::memory_order_release);

    m_nextBlockFrame += AudioFormat::kBlockFrames;
    const int64_t keepFrom = m_nextBlockFrame - AudioFormat::kSampleRate; // retain ~1 s
    TrimSource(m_mic, keepFrom);
    TrimSource(m_sys, keepFrom);
    return true;
}

bool Mixer::Flush(
    RingBuffer& micRing,
    RingBuffer& sysRing,
    float* outInterleavedStereo,
    float* outMicStem,
    float* outSysStem) {
    DrainRing(micRing, m_mic);
    DrainRing(sysRing, m_sys);

    if (!HasPending() && micRing.Empty() && sysRing.Empty()) {
        return false;
    }

    // Force emission of the next block (treat as stale).
    const uint64_t forceNow = m_startQpc
        + AudioFormat::FramesToQpc(m_nextBlockFrame + kStaleFrames + AudioFormat::kBlockFrames, m_qpcFrequency);
    return Process(micRing, sysRing, forceNow, outInterleavedStereo, outMicStem, outSysStem);
}
