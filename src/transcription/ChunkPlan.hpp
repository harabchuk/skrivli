#pragma once

#include <algorithm>
#include <cstddef>
#include <cstdint>
#include <utility>
#include <vector>

// Plans 48 kHz stereo frame ranges for long-recording transcription (spec §7).
// Recordings longer than one hour are split into 30-minute pieces with a short overlap;
// segment timestamps are offset by the chunk start.
struct ChunkPlan {
    static constexpr int kSampleRate = 48000;
    static constexpr double kLongThresholdSeconds = 60.0 * 60.0;
    static constexpr double kChunkSeconds = 30.0 * 60.0;
    static constexpr double kOverlapSeconds = 1.0;

    static bool ShouldChunk(double durationSeconds) {
        return durationSeconds > kLongThresholdSeconds;
    }

    static size_t ChunkFrames() {
        return static_cast<size_t>(kChunkSeconds * static_cast<double>(kSampleRate));
    }

    static size_t OverlapFrames() {
        return static_cast<size_t>(kOverlapSeconds * static_cast<double>(kSampleRate));
    }

    // Each entry is (frameOffset, frameCount) into interleaved stereo at 48 kHz.
    static std::vector<std::pair<size_t, size_t>> Plan(size_t totalFrames48) {
        std::vector<std::pair<size_t, size_t>> chunks;
        if (totalFrames48 == 0) {
            return chunks;
        }

        const double durationSeconds =
            static_cast<double>(totalFrames48) / static_cast<double>(kSampleRate);
        if (!ShouldChunk(durationSeconds)) {
            chunks.emplace_back(0, totalFrames48);
            return chunks;
        }

        const size_t chunkFrames = ChunkFrames();
        const size_t overlapFrames = OverlapFrames();
        size_t frameOffset = 0;
        while (frameOffset < totalFrames48) {
            const size_t frames = (std::min)(chunkFrames, totalFrames48 - frameOffset);
            chunks.emplace_back(frameOffset, frames);
            if (frameOffset + frames >= totalFrames48) {
                break;
            }
            const size_t advance = frames > overlapFrames ? frames - overlapFrames : frames;
            frameOffset += advance;
        }
        return chunks;
    }

    // whisper.cpp segment times are in 10 ms units.
    static int64_t OffsetTenths(size_t frameOffset) {
        return static_cast<int64_t>((frameOffset * 100.0) / static_cast<double>(kSampleRate));
    }
};
