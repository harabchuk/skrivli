#pragma once

#include <cstdint>
#include <memory>
#include <vector>

// miniaudio data converter. Turns a device mix-format packet into 48 kHz
// stereo float. Set up once at stream start.
class AudioResampler {
  public:
    AudioResampler() = default;
    ~AudioResampler();

    AudioResampler(const AudioResampler&) = delete;
    auto operator=(const AudioResampler&) -> AudioResampler& = delete;

    // inputFormat is a WAVEFORMATEX* (possibly WAVEFORMATEXTENSIBLE).
    bool Initialize(const void* inputFormat);
    void Reset();

    // Resamples one packet into outInterleavedStereo (stereo float @ 48 kHz).
    // outFrames is the number of stereo frames written.
    bool Process(const void* inputBytes, uint32_t inputByteCount, uint32_t inputFrames,
                 std::vector<float>& outInterleavedStereo, uint32_t& outFrames);

    bool IsReady() const {
        return m_state != nullptr;
    }

  private:
    struct State;
    struct StateDeleter {
        void operator()(State* state) const;
    };

    std::unique_ptr<State, StateDeleter> m_state;
};
