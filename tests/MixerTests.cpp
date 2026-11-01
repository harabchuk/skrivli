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
    // Arrange
    RingBuffer mic(48000);
    RingBuffer sys(48000);
    Mixer mixer;
    mixer.Reset(0, kFreq, 1.0f, 0.8f);
    PushTone(mic, 0, 480, 0.5f, 0.5f);
    PushTone(sys, 0, 480, 0.25f, 0.25f);
    float out[480 * 2]{};

    // Act
    ASSERT_TRUE(mixer.Process(mic, sys, FramesToQpc(480), out));

    // Assert
    EXPECT_NEAR(out[0], 0.7f, 0.01f); // 0.5*1.0 + 0.25*0.8
    EXPECT_NEAR(out[1], 0.7f, 0.01f);
}

TEST(Mixer, GapBecomesSilence) {
    // Arrange
    RingBuffer mic(48000);
    RingBuffer sys(48000);
    Mixer mixer;
    mixer.Reset(0, kFreq, 1.0f, 1.0f);
    PushTone(mic, 0, 100, 0.5f, 0.5f);
    PushTone(mic, FramesToQpc(100 + 480), 100, 0.5f, 0.5f); // >5 ms gap → silence
    PushTone(sys, 0, 480, 0.0f, 0.0f);
    float out[480 * 2]{};

    // Act
    ASSERT_TRUE(mixer.Process(mic, sys, FramesToQpc(1000), out));

    // Assert
    EXPECT_NEAR(out[0], 0.5f, 0.01f);
    EXPECT_NEAR(out[200 * 2], 0.0f, 0.01f);
}

TEST(Mixer, OverlapIsDropped) {
    // Arrange
    RingBuffer mic(48000);
    RingBuffer sys(48000);
    Mixer mixer;
    mixer.Reset(0, kFreq, 1.0f, 0.0f);
    PushTone(mic, 0, 480, 0.4f, 0.4f);
    PushTone(mic, FramesToQpc(100), 480, 0.9f, 0.9f); // overlaps
    PushTone(sys, 0, 480, 0.0f, 0.0f);
    float out[480 * 2]{};

    // Act
    ASSERT_TRUE(mixer.Process(mic, sys, FramesToQpc(480), out));

    // Assert
    EXPECT_NEAR(out[0], 0.4f, 0.01f);
    EXPECT_NEAR(out[200 * 2], 0.4f, 0.01f);
}

TEST(Mixer, SoftLimiterBelowFullScale) {
    // Arrange
    RingBuffer mic(48000);
    RingBuffer sys(48000);
    Mixer mixer;
    mixer.Reset(0, kFreq, 2.0f, 2.0f);
    PushTone(mic, 0, 480, 1.0f, 1.0f);
    PushTone(sys, 0, 480, 1.0f, 1.0f);
    float out[480 * 2]{};

    // Act
    ASSERT_TRUE(mixer.Process(mic, sys, FramesToQpc(480), out));

    // Assert
    for (float sample : out) {
        EXPECT_LT(std::fabs(sample), 1.0f);
    }
}

TEST(Mixer, DriftCorrectionUnderOffset) {
    // Arrange
    RingBuffer mic(48000);
    RingBuffer sys(48000);
    Mixer mixer;
    mixer.Reset(0, kFreq, 1.0f, 1.0f);
    uint64_t micQpc = 0;
    uint64_t sysQpc = 0;

    // Act
    for (int block = 0; block < 200; ++block) {
        PushTone(mic, micQpc, 480, 0.1f, 0.1f);
        PushTone(sys, sysQpc, 480, 0.1f, 0.1f);
        micQpc += FramesToQpc(480);
        sysQpc += FramesToQpc(480);
        if ((block % 50) == 49) {
            micQpc += FramesToQpc(1); // ~100 ppm: mic timestamp runs slightly fast
        }

        float out[480 * 2]{};
        const uint64_t now = (std::max)(micQpc, sysQpc) + FramesToQpc(200);
        mixer.Process(mic, sys, now, out);
    }

    // Assert
    SUCCEED();
}

TEST(Mixer, AgcRaisesQuietSpeechTowardTarget) {
    // Arrange
    RingBuffer mic(48000);
    RingBuffer sys(48000);
    Mixer mixer;
    mixer.Reset(0, kFreq, 1.0f, 0.0f, true);
    constexpr float kQuiet = 0.03f; // ~-30 dBFS
    uint64_t qpc = 0;
    float out[480 * 2]{};

    // Act
    for (int block = 0; block < 150; ++block) {
        PushTone(mic, qpc, 480, kQuiet, kQuiet);
        PushTone(sys, qpc, 480, 0.0f, 0.0f);
        qpc += FramesToQpc(480);
        ASSERT_TRUE(mixer.Process(mic, sys, qpc + FramesToQpc(200), out));
    }

    // Assert
    EXPECT_GT(mixer.EffectiveMicGain(), 1.5f);
    EXPECT_LE(mixer.EffectiveMicGain(), Mixer::kAgcMaxGain);
    EXPECT_NEAR(std::fabs(out[0]), kQuiet * mixer.EffectiveMicGain(), 0.02f);
}

TEST(Mixer, AgcLowersLoudSpeechTowardTarget) {
    // Arrange
    RingBuffer mic(48000);
    RingBuffer sys(48000);
    Mixer mixer;
    mixer.Reset(0, kFreq, 1.0f, 0.0f, true);
    constexpr float kLoud = 0.5f; // ~-6 dBFS
    uint64_t qpc = 0;
    float out[480 * 2]{};

    // Act
    for (int block = 0; block < 80; ++block) {
        PushTone(mic, qpc, 480, kLoud, kLoud);
        PushTone(sys, qpc, 480, 0.0f, 0.0f);
        qpc += FramesToQpc(480);
        ASSERT_TRUE(mixer.Process(mic, sys, qpc + FramesToQpc(200), out));
    }

    // Assert
    EXPECT_LT(mixer.EffectiveMicGain(), 0.85f);
    EXPECT_GE(mixer.EffectiveMicGain(), Mixer::kAgcMinGain);
}

TEST(Mixer, AgcIgnoresSilence) {
    // Arrange
    RingBuffer mic(48000);
    RingBuffer sys(48000);
    Mixer mixer;
    mixer.Reset(0, kFreq, 1.0f, 0.0f, true);
    constexpr float kNoise = 0.001f; // below -45 dBFS gate
    uint64_t qpc = 0;
    float out[480 * 2]{};

    // Act
    for (int block = 0; block < 200; ++block) {
        PushTone(mic, qpc, 480, kNoise, kNoise);
        PushTone(sys, qpc, 480, 0.0f, 0.0f);
        qpc += FramesToQpc(480);
        ASSERT_TRUE(mixer.Process(mic, sys, qpc + FramesToQpc(200), out));
    }

    // Assert
    EXPECT_NEAR(mixer.EffectiveMicGain(), 1.0f, 0.05f);
}
