#pragma once

#include "infrastructure/audio/MiniAudio.hpp"

#include <cstddef>
#include <vector>

// RAII wrapper around ma_data_converter that appends float output to a growing buffer.
// Output is always 32-bit float; `produced` counts frames (not samples) already in `out`.
class MaConverter {
public:
    MaConverter() = default;
    ~MaConverter();

    MaConverter(const MaConverter&) = delete;
    auto operator=(const MaConverter&) -> MaConverter& = delete;

    // Replaces any previous converter. `config.formatOut` must be ma_format_f32.
    bool Init(const ma_data_converter_config& config);
    bool IsReady() const { return m_live; }

    ma_uint64 InputLatency() const;
    // Returns `fallback` when miniaudio cannot estimate the count.
    ma_uint64 ExpectedOutputFrames(ma_uint64 inputFrames, ma_uint64 fallback) const;

    // Converts `frames` input frames and appends them to `out` starting at frame `produced`,
    // growing `out` as needed.
    bool Push(const void* input, ma_uint64 frames, std::vector<float>& out, size_t& produced);

    // Pushes InputLatency() frames of silence so the filter tail reaches `out`.
    bool FlushLatency(std::vector<float>& out, size_t& produced);

private:
    void Uninit();

    ma_data_converter m_converter{};
    bool m_live = false;
    ma_uint32 m_bytesPerInputFrame = 0;
    ma_uint32 m_channelsOut = 0;
};
