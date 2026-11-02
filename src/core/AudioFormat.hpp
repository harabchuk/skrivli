#pragma once

#include <audioclient.h>
#include <cstdint>
#include <ks.h>
#include <ksmedia.h>
#include <mmreg.h>

namespace AudioFormat {

inline constexpr UINT32 kSampleRate = 48000;
inline constexpr UINT16 kChannels = 2;
inline constexpr UINT16 kBitsPerSample = 32;
inline constexpr UINT32 kBlockFrames = 480;                // 10 ms
inline constexpr REFERENCE_TIME kBufferDuration = 2000000; // 200 ms in 100-ns units
inline constexpr DWORD kEventTimeoutMs = 100;

inline WAVEFORMATEXTENSIBLE MakeFloatStereo48k() {
    WAVEFORMATEXTENSIBLE format{};
    format.Format.wFormatTag = WAVE_FORMAT_EXTENSIBLE;
    format.Format.nChannels = kChannels;
    format.Format.nSamplesPerSec = kSampleRate;
    format.Format.wBitsPerSample = kBitsPerSample;
    format.Format.nBlockAlign = static_cast<WORD>((kChannels * kBitsPerSample) / 8);
    format.Format.nAvgBytesPerSec = format.Format.nSamplesPerSec * format.Format.nBlockAlign;
    format.Format.cbSize = sizeof(WAVEFORMATEXTENSIBLE) - sizeof(WAVEFORMATEX);
    format.Samples.wValidBitsPerSample = kBitsPerSample;
    format.dwChannelMask = KSAUDIO_SPEAKER_STEREO;
    format.SubFormat = KSDATAFORMAT_SUBTYPE_IEEE_FLOAT;
    return format;
}

inline int64_t QpcToFrames(uint64_t qpcDelta, uint64_t qpcFrequency) {
    if (qpcFrequency == 0) {
        return 0;
    }
    return static_cast<int64_t>((qpcDelta * static_cast<uint64_t>(kSampleRate)) / qpcFrequency);
}

inline uint64_t FramesToQpc(int64_t frames, uint64_t qpcFrequency) {
    return (static_cast<uint64_t>(frames) * qpcFrequency) / static_cast<uint64_t>(kSampleRate);
}

} // namespace AudioFormat
