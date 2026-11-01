#include "audio/Mixer.hpp"
#include "audio/RingBuffer.hpp"

#include <gtest/gtest.h>

#include <algorithm>
#include <cmath>
#include <vector>

namespace {

constexpr uint64_t kFreq = 10'000'000; // 10 MHz fake QPC

uint64_t FramesToQpc(int64_t frames) {
    return static_cast<uint64_t>((frames * static_cast<int64_t>(kFreq)) / 48000);
}

void PushTone(RingBuffer& ring, uint64_t qpc, uint32_t frames, float left, float right) {
    std::vector<float> samples(static_cast<size_t>(frames) * 2);
    for (uint32_t i = 0; i < frames; ++i) {
        samples[i * 2] = left;
        samples[i * 2 + 1] = right;
    }
    ASSERT_TRUE(ring.Push(samples.data(), frames, qpc, false));
}

}  // namespace

TEST(Mixer, AlignsAndMixesCoveredBlock) {
    RingBuffer mic(48000);
    RingBuffer sys(48000);
    Mixer mixer;
    mixer.Reset(0, kFreq, 1.0f, 0.8f);

    PushTone(mic, 0, 480, 0.5f, 0.5f);
    PushTone(sys, 0, 480, 0.25f, 0.25f);

    float out[480 * 2]{};
    ASSERT_TRUE(mixer.Process(mic, sys, FramesToQpc(480), out));

    // mic mono 0.5 * 1.0 + sys 0.25 * 0.8 = 0.5 + 0.2 = 0.7
    EXPECT_NEAR(out[0], 0.7f, 0.01f);
    EXPECT_NEAR(out[1], 0.7f, 0.01f);
}

TEST(Mixer, GapBecomesSilence) {
    RingBuffer mic(48000);
    RingBuffer sys(48000);
    Mixer mixer;
    mixer.Reset(0, kFreq, 1.0f, 1.0f);

    // First packet at frame 0, second after a >5 ms gap.
    PushTone(mic, 0, 100, 0.5f, 0.5f);
    PushTone(mic, FramesToQpc(100 + 480), 100, 0.5f, 0.5f); // gap of 10 ms
    PushTone(sys, 0, 480, 0.0f, 0.0f);

    float out[480 * 2]{};
    ASSERT_TRUE(mixer.Process(mic, sys, FramesToQpc(1000), out));

    // Frames after the first packet and before the second should be silence on mic.
    EXPECT_NEAR(out[0], 0.5f, 0.01f);
    EXPECT_NEAR(out[200 * 2], 0.0f, 0.01f);
}

TEST(Mixer, OverlapIsDropped) {
    RingBuffer mic(48000);
    RingBuffer sys(48000);
    Mixer mixer;
    mixer.Reset(0, kFreq, 1.0f, 0.0f);

    PushTone(mic, 0, 480, 0.4f, 0.4f);
    PushTone(mic, FramesToQpc(100), 480, 0.9f, 0.9f); // overlaps
    PushTone(sys, 0, 480, 0.0f, 0.0f);

    float out[480 * 2]{};
    ASSERT_TRUE(mixer.Process(mic, sys, FramesToQpc(480), out));
    EXPECT_NEAR(out[0], 0.4f, 0.01f);
    EXPECT_NEAR(out[200 * 2], 0.4f, 0.01f);
}

TEST(Mixer, SoftLimiterBelowFullScale) {
    RingBuffer mic(48000);
    RingBuffer sys(48000);
    Mixer mixer;
    mixer.Reset(0, kFreq, 2.0f, 2.0f);

    PushTone(mic, 0, 480, 1.0f, 1.0f);
    PushTone(sys, 0, 480, 1.0f, 1.0f);

    float out[480 * 2]{};
    ASSERT_TRUE(mixer.Process(mic, sys, FramesToQpc(480), out));
    for (float sample : out) {
        EXPECT_LT(std::fabs(sample), 1.0f);
    }
}

TEST(Mixer, DriftCorrectionUnderOffset) {
    RingBuffer mic(48000);
    RingBuffer sys(48000);
    Mixer mixer;
    mixer.Reset(0, kFreq, 1.0f, 1.0f);

    // Simulate ~100 ppm by feeding slightly different amounts over many blocks.
    uint64_t micQpc = 0;
    uint64_t sysQpc = 0;
    for (int block = 0; block < 200; ++block) {
        PushTone(mic, micQpc, 480, 0.1f, 0.1f);
        PushTone(sys, sysQpc, 480, 0.1f, 0.1f);
        // 100 ppm: every 10000 frames, one extra mic frame equivalent.
        micQpc += FramesToQpc(480);
        sysQpc += FramesToQpc(480);
        if ((block % 50) == 49) {
            micQpc += FramesToQpc(1); // mic clock slightly fast in timestamp space
        }

        float out[480 * 2]{};
        const uint64_t now = (std::max)(micQpc, sysQpc) + FramesToQpc(200);
        mixer.Process(mic, sys, now, out);
    }

    SUCCEED();
}
