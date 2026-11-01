#include "transcription/MarkdownFormatter.hpp"

#include <gtest/gtest.h>

#include <filesystem>
#include <fstream>
#include <string>

TEST(MarkdownFormatter, TimestampAndTitle) {
    TranscriptMeta meta{};
    meta.titleDateTime = "2026-11-01 13:18";
    meta.durationSeconds = 42.0;
    meta.recordedRange = "2026-11-01 13:18 – 13:19";
    meta.modelFile = "ggml-base-q5_1.bin";
    meta.languageConfigured = "auto";
    meta.languageDetected = "en";

    std::vector<TranscriptSegment> segments = {
        {30, 80, "  Hello world  "},
        {90, 120, "Next line"},
    };

    const std::string md = MarkdownFormatter::Format(meta, segments);
    EXPECT_NE(md.find("# Meeting — 2026-11-01 13:18"), std::string::npos);
    EXPECT_NE(md.find("**[00:00:00]** Hello world Next line"), std::string::npos);
    EXPECT_NE(md.find("language: auto → en"), std::string::npos);
    EXPECT_EQ(md.find("\xEF\xBB\xBF"), std::string::npos);
}

TEST(MarkdownFormatter, SplitsOnLongPause) {
    TranscriptMeta meta{};
    meta.titleDateTime = "2026-11-01 13:18";
    meta.durationSeconds = 10.0;
    meta.recordedRange = "2026-11-01 13:18";
    meta.modelFile = "ggml-base-q5_1.bin";
    meta.languageConfigured = "en";

    std::vector<TranscriptSegment> segments = {
        {0, 50, "First"},
        {250, 300, "Second"}, // 2.0 s pause
    };

    const std::string md = MarkdownFormatter::Format(meta, segments);
    EXPECT_NE(md.find("**[00:00:00]** First"), std::string::npos);
    EXPECT_NE(md.find("**[00:00:02]** Second"), std::string::npos);
}

TEST(MarkdownFormatter, DropsTripleRepeats) {
    TranscriptMeta meta{};
    meta.titleDateTime = "2026-11-01 13:18";
    meta.durationSeconds = 5.0;
    meta.recordedRange = "2026-11-01 13:18";
    meta.modelFile = "m.bin";
    meta.languageConfigured = "en";

    std::vector<TranscriptSegment> segments = {
        {0, 10, "thanks"},
        {20, 30, "thanks"},
        {40, 50, "thanks"},
        {60, 70, "thanks"},
        {80, 90, "done"},
    };

    const std::string md = MarkdownFormatter::Format(meta, segments);
    const size_t first = md.find("thanks");
    const size_t second = md.find("thanks", first + 1);
    // At most two consecutive kept → merged paragraph should not spam.
    EXPECT_NE(first, std::string::npos);
    EXPECT_NE(md.find("done"), std::string::npos);
    // Only two "thanks" tokens remain before dedup merge into one paragraph.
    size_t count = 0;
    size_t pos = 0;
    while ((pos = md.find("thanks", pos)) != std::string::npos) {
        ++count;
        pos += 6;
    }
    EXPECT_LE(count, 2u);
    (void)second;
}

TEST(MarkdownFormatter, WriteHasNoBom) {
    const auto path = std::filesystem::temp_directory_path() / "meetingbuddy_md_test.md";
    ASSERT_TRUE(MarkdownFormatter::WriteUtf8NoBom(path.wstring(), "# hi\n"));
    {
        std::ifstream in(path, std::ios::binary);
        ASSERT_TRUE(in);
        std::string body((std::istreambuf_iterator<char>(in)), std::istreambuf_iterator<char>());
        EXPECT_EQ(body, "# hi\n");
    }
    std::error_code ec;
    std::filesystem::remove(path, ec);
}
