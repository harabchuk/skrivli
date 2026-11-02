#include "core/RingBuffer.hpp"

#include <gtest/gtest.h>
#include <vector>

TEST(RingBuffer, PushPopRoundTrip) {
    // Arrange
    RingBuffer ring(1024);
    const float input[] = {0.1f, -0.2f, 0.3f, -0.4f};

    // Act
    ASSERT_TRUE(ring.Push(input, 2, 12345, false));
    float output[4]{};
    uint32_t frames = 0;
    uint64_t qpc = 0;
    uint32_t flags = 0;
    ASSERT_TRUE(ring.Pop(output, 8, frames, qpc, flags));

    // Assert
    EXPECT_EQ(frames, 2u);
    EXPECT_EQ(qpc, 12345u);
    EXPECT_EQ(flags, 0u);
    EXPECT_FLOAT_EQ(output[0], 0.1f);
    EXPECT_FLOAT_EQ(output[1], -0.2f);
    EXPECT_FLOAT_EQ(output[2], 0.3f);
    EXPECT_FLOAT_EQ(output[3], -0.4f);
    EXPECT_TRUE(ring.Empty());
}

TEST(RingBuffer, Wraparound) {
    RingBuffer ring(64);
    std::vector<float> packet(16, 0.5f); // 8 stereo frames

    for (int i = 0; i < 10; ++i) {
        ASSERT_TRUE(ring.Push(packet.data(), 8, static_cast<uint64_t>(i + 1), false));
        float out[16]{};
        uint32_t frames = 0;
        uint64_t qpc = 0;
        uint32_t flags = 0;
        ASSERT_TRUE(ring.Pop(out, 8, frames, qpc, flags));
        EXPECT_EQ(frames, 8u);
        EXPECT_EQ(qpc, static_cast<uint64_t>(i + 1));
    }
}

TEST(RingBuffer, OverrunDropsWithoutBlocking) {
    // Arrange
    RingBuffer ring(32);
    std::vector<float> packet(16, 1.0f); // 8 frames

    // Act
    int pushed = 0;
    int dropped = 0;
    for (int i = 0; i < 20; ++i) {
        if (ring.Push(packet.data(), 8, static_cast<uint64_t>(i), false)) {
            ++pushed;
        } else {
            ++dropped;
        }
    }

    // Assert
    EXPECT_EQ(pushed, 4); // 32-frame capacity / 8-frame packets
    EXPECT_GT(dropped, 0);
    EXPECT_TRUE(ring.ConsumeOverrun());

    float out[16]{};
    uint32_t frames = 0;
    uint64_t qpc = 0;
    uint32_t flags = 0;
    ASSERT_TRUE(ring.Pop(out, 8, frames, qpc, flags));
}
