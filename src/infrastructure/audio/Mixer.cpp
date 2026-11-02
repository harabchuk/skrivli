#include "infrastructure/audio/Mixer.hpp"

#include "infrastructure/audio/MiniAudio.hpp"

#include <algorithm>
#include <cmath>

struct Mixer::Downmix {
    ma_channel_converter converter{};
    bool live = false;
};

void Mixer::DownmixDeleter::operator()(Downmix* downmix) const {
    if (downmix == nullptr) {
        return;
    }
    if (downmix->live) {
        ma_channel_converter_uninit(&downmix->converter, nullptr);
        downmix->live = false;
    }
    delete downmix;
}

Mixer::~Mixer() = default;

void Mixer::EnsureDownmix() {
    if (m_downmix != nullptr && m_downmix->live) {
        return;
    }
    auto downmix = std::unique_ptr<Downmix, DownmixDeleter>(new Downmix());
    const auto config = ma_channel_converter_config_init(
        ma_format_f32,
        AudioFormat::kChannels,
        nullptr,
        1,
        nullptr,
        ma_channel_mix_mode_rectangular);
    if (ma_channel_converter_init(&config, nullptr, &downmix->converter) != MA_SUCCESS) {
        return;
    }
    downmix->live = true;
    m_downmix = std::move(downmix);
}

void Mixer::Reset(uint64_t startQpc, uint64_t qpcFrequency, float micGain, float systemGain, bool micAgc) {
    m_startQpc = startQpc;
    m_qpcFrequency = qpcFrequency == 0 ? 1 : qpcFrequency;
    m_micGain = std::clamp(micGain, 0.0f, kAgcMaxGain);
    m_systemGain = systemGain;
    m_micAgc = micAgc;
    m_agcGain = std::clamp(m_micGain <= 0.0f ? 1.0f : m_micGain, kAgcMinGain, kAgcMaxGain);
    m_agcPrevGain = m_agcGain;
    m_agcRms = 0.0f;
    m_nextBlockFrame = 0;
    m_mic.Clear();
    m_sys.Clear();
    m_packetScratch.resize(4096 * AudioFormat::kChannels);
    EnsureDownmix();
    m_peakMic.store(0.0f, std::memory_order_relaxed);
    m_peakSys.store(0.0f, std::memory_order_relaxed);
    m_effectiveMicGain.store(CurrentMicGain(), std::memory_order_relaxed);
}

void Mixer::SetGains(float micGain, float systemGain) {
    m_micGain = std::clamp(micGain, 0.0f, kAgcMaxGain);
    m_systemGain = systemGain;
    if (!m_micAgc) {
        m_effectiveMicGain.store(m_micGain, std::memory_order_release);
    }
}

void Mixer::SetMicAgc(bool enabled) {
    m_micAgc = enabled;
    if (enabled) {
        const float seed = m_micGain > 0.0f ? m_micGain : 1.0f;
        m_agcGain = std::clamp(seed, kAgcMinGain, kAgcMaxGain);
        m_agcPrevGain = m_agcGain;
        m_agcRms = 0.0f;
    }
    m_effectiveMicGain.store(CurrentMicGain(), std::memory_order_release);
}

float Mixer::CurrentMicGain() const {
    return m_micAgc ? m_agcGain : m_micGain;
}

void Mixer::UpdateAgc(float blockRms) {
    constexpr float dt = static_cast<float>(AudioFormat::kBlockFrames) / static_cast<float>(AudioFormat::kSampleRate);
    const float alpha = 1.0f - std::exp(-dt / kAgcWindowSeconds);
    m_agcRms = (1.0f - alpha) * m_agcRms + alpha * blockRms;

    // Hold gain through silence so background noise is not pumped up.
    if (m_agcRms < kAgcGateLinear) {
        m_agcPrevGain = m_agcGain;
        return;
    }

    const auto desired = std::clamp(kAgcTargetLinear / m_agcRms, kAgcMinGain, kAgcMaxGain);
    const float coef = desired < m_agcGain ? kAgcAttackPerBlock : kAgcReleasePerBlock;
    m_agcPrevGain = m_agcGain;
    m_agcGain = m_agcGain + (desired - m_agcGain) * coef;
    m_agcGain = std::clamp(m_agcGain, kAgcMinGain, kAgcMaxGain);
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
    auto* dest = source.samples.data()
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
        const auto frameIndex = AudioFormat::QpcToFrames(qpc - m_startQpc, m_qpcFrequency);
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
    ma_silence_pcm_frames(dest, AudioFormat::kBlockFrames, ma_format_f32, AudioFormat::kChannels);
    if (!source.hasData) {
        return;
    }

    const auto copyStart = (std::max)(blockStart, source.startFrame);
    const auto copyEnd = (std::min)(blockEnd, source.endFrame);
    if (copyStart >= copyEnd) {
        return;
    }

    const int64_t destOffset = copyStart - blockStart;
    const int64_t srcOffset = copyStart - source.startFrame;
    const auto frameCount = static_cast<size_t>(copyEnd - copyStart);
    ma_copy_pcm_frames(
        dest + static_cast<size_t>(destOffset) * AudioFormat::kChannels,
        source.samples.data() + static_cast<size_t>(srcOffset) * AudioFormat::kChannels,
        frameCount,
        ma_format_f32,
        AudioFormat::kChannels);
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

    const auto micFill = static_cast<double>(m_mic.endFrame - m_nextBlockFrame);
    const auto sysFill = static_cast<double>(m_sys.endFrame - m_nextBlockFrame);
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

    if (delta > static_cast<double>(kDriftThresholdFrames)) {
        // Mic ahead of system — drop one mic frame (or duplicate sys if mic is empty).
        dropOneFrame(m_mic, m_nextBlockFrame);
        m_mic.fillAverage -= 1.0;
    } else {
        // System ahead of mic — drop one system frame.
        dropOneFrame(m_sys, m_nextBlockFrame);
        m_sys.fillAverage -= 1.0;
    }
}

float Mixer::SoftLimit(float sample) {
    const auto absSample = std::fabs(sample);
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

    float micMono[AudioFormat::kBlockFrames];
    const bool downmixed = m_downmix != nullptr && m_downmix->live
        && ma_channel_converter_process_pcm_frames(
               &m_downmix->converter,
               micMono,
               micBlock,
               AudioFormat::kBlockFrames)
            == MA_SUCCESS;
    if (!downmixed) {
        for (uint32_t i = 0; i < AudioFormat::kBlockFrames; ++i) {
            micMono[i] = 0.5f * (micBlock[i * 2] + micBlock[i * 2 + 1]);
        }
    }

    float micPeak = 0.0f;
    float sysPeak = 0.0f;
    double micPower = 0.0;

    for (uint32_t i = 0; i < AudioFormat::kBlockFrames; ++i) {
        micPeak = (std::max)(micPeak, std::fabs(micMono[i]));
        micPower += static_cast<double>(micMono[i]) * static_cast<double>(micMono[i]);
        const float sysL = sysBlock[i * 2];
        const float sysR = sysBlock[i * 2 + 1];
        sysPeak = (std::max)(sysPeak, (std::max)(std::fabs(sysL), std::fabs(sysR)));
    }

    if (m_micAgc) {
        UpdateAgc(static_cast<float>(std::sqrt(micPower / static_cast<double>(AudioFormat::kBlockFrames))));
    }

    const float gainStart = m_micAgc ? m_agcPrevGain : m_micGain;
    const auto gainEnd = CurrentMicGain();
    m_effectiveMicGain.store(gainEnd, std::memory_order_release);

    float micScaled[AudioFormat::kBlockFrames * AudioFormat::kChannels];
    for (uint32_t i = 0; i < AudioFormat::kBlockFrames; ++i) {
        const float t = static_cast<float>(i) / static_cast<float>(AudioFormat::kBlockFrames);
        const float micGain = gainStart + (gainEnd - gainStart) * t;
        const float scaled = micGain * micMono[i];
        micScaled[i * 2] = scaled;
        micScaled[i * 2 + 1] = scaled;

        if (outMicStem != nullptr) {
            outMicStem[i * 2] = SoftLimit(micMono[i]);
            outMicStem[i * 2 + 1] = SoftLimit(micMono[i]);
        }
        if (outSysStem != nullptr) {
            outSysStem[i * 2] = SoftLimit(sysBlock[i * 2]);
            outSysStem[i * 2 + 1] = SoftLimit(sysBlock[i * 2 + 1]);
        }
    }

    ma_silence_pcm_frames(outInterleavedStereo, AudioFormat::kBlockFrames, ma_format_f32, AudioFormat::kChannels);
    ma_mix_pcm_frames_f32(outInterleavedStereo, micScaled, AudioFormat::kBlockFrames, AudioFormat::kChannels, 1.0f);
    ma_mix_pcm_frames_f32(
        outInterleavedStereo,
        sysBlock,
        AudioFormat::kBlockFrames,
        AudioFormat::kChannels,
        m_systemGain);
    for (uint32_t i = 0; i < AudioFormat::kBlockFrames; ++i) {
        const auto outL = SoftLimit(outInterleavedStereo[i * 2]);
        const auto outR = SoftLimit(outInterleavedStereo[i * 2 + 1]);
        outInterleavedStereo[i * 2] = outL;
        outInterleavedStereo[i * 2 + 1] = outR;
    }

    m_peakMic.store(micPeak, std::memory_order_release);
    m_peakSys.store(sysPeak, std::memory_order_release);
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
