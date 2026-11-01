#pragma once

#include <mfapi.h>
#include <mftransform.h>
#include <wrl/client.h>

#include <cstdint>
#include <vector>

// Media Foundation Audio Resampler DSP wrapper. Converts device mix format
// packets to 48 kHz stereo float. Setup once at stream start; no per-packet
// allocation of the transform itself.
class AudioResampler {
public:
    AudioResampler() = default;
    ~AudioResampler();

    AudioResampler(const AudioResampler&) = delete;
    AudioResampler& operator=(const AudioResampler&) = delete;

    // inputFormat is a WAVEFORMATEX* (possibly WAVEFORMATEXTENSIBLE).
    bool Initialize(const void* inputFormat);
    void Reset();

    // Resamples one packet into outInterleavedStereo (stereo float @ 48 kHz).
    // outFrames is the number of stereo frames written.
    bool Process(
        const void* inputBytes,
        uint32_t inputByteCount,
        uint32_t inputFrames,
        std::vector<float>& outInterleavedStereo,
        uint32_t& outFrames);

    bool IsReady() const { return m_transform != nullptr; }

private:
    bool SetMediaTypes(const void* inputFormat);

    Microsoft::WRL::ComPtr<IMFTransform> m_transform;
    uint32_t m_inputBlockAlign = 0;
    uint32_t m_outputBufferBytes = 0;
    bool m_mfStarted = false;
};
