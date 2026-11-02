#include "core/ChunkPlan.hpp"

#include <algorithm>
#include <gtest/gtest.h>

TEST(ChunkPlan, ShortRecordingIsSingleChunk) {
    // Arrange
    const size_t frames = static_cast<size_t>(ChunkPlan::kSampleRate * 59 * 60); // 59 min

    // Act
    const auto chunks = ChunkPlan::Plan(frames);

    // Assert
    ASSERT_EQ(chunks.size(), 1u);
    EXPECT_EQ(chunks[0].first, 0u);
    EXPECT_EQ(chunks[0].second, frames);
    EXPECT_FALSE(ChunkPlan::ShouldChunk(59.0 * 60.0));
}

TEST(ChunkPlan, SyntheticLongBufferSplitsWithOverlap) {
    // Arrange — just over one hour so chunking engages (no two-hour take required).
    const size_t totalFrames =
        static_cast<size_t>(ChunkPlan::kSampleRate * (60 * 60 + 30)); // 1 h 30 s
    const size_t chunkFrames = ChunkPlan::ChunkFrames();
    const size_t overlapFrames = ChunkPlan::OverlapFrames();

    // Act
    const auto chunks = ChunkPlan::Plan(totalFrames);

    // Assert
    ASSERT_GE(chunks.size(), 2u);
    EXPECT_TRUE(ChunkPlan::ShouldChunk(static_cast<double>(totalFrames) / ChunkPlan::kSampleRate));

    EXPECT_EQ(chunks.front().first, 0u);
    EXPECT_EQ(chunks.front().second, chunkFrames);

    // Second chunk starts one overlap before the end of the first.
    EXPECT_EQ(chunks[1].first, chunkFrames - overlapFrames);

    size_t coveredEnd = 0;
    for (const auto& [offset, frames] : chunks) {
        EXPECT_GT(frames, 0u);
        EXPECT_LE(offset + frames, totalFrames);
        coveredEnd = (std::max)(coveredEnd, offset + frames);
    }
    EXPECT_EQ(coveredEnd, totalFrames);

    // Timestamp offset for the second chunk is in 10 ms units.
    EXPECT_EQ(ChunkPlan::OffsetTenths(chunks[1].first),
              static_cast<int64_t>((chunks[1].first * 100.0) / ChunkPlan::kSampleRate));
}

TEST(ChunkPlan, ExactlyOneHourNotChunked) {
    // Arrange
    const size_t frames = static_cast<size_t>(ChunkPlan::kSampleRate * 60 * 60);

    // Act / Assert
    EXPECT_FALSE(ChunkPlan::ShouldChunk(60.0 * 60.0));
    EXPECT_EQ(ChunkPlan::Plan(frames).size(), 1u);
}

TEST(ChunkPlan, EmptyAudioYieldsNoChunks) {
    EXPECT_TRUE(ChunkPlan::Plan(0).empty());
}
