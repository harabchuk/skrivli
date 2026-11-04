#include "core/TranscriptText.hpp"

#include <filesystem>
#include <fstream>
#include <gtest/gtest.h>
#include <string>

TEST(TranscriptText, TimestampParagraphs) {
    // Arrange
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

    // Act
    const std::string text = TranscriptText::Format(meta, segments);

    // Assert
    EXPECT_NE(text.find("[00:00:00] Hello world Next line"), std::string::npos);
    EXPECT_EQ(text.find("# Meeting"), std::string::npos);
    EXPECT_EQ(text.find("**"), std::string::npos);
    EXPECT_EQ(text.find("\xEF\xBB\xBF"), std::string::npos);
}

TEST(TranscriptText, SplitsOnLongPause) {
    // Arrange
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

    // Act
    const std::string text = TranscriptText::Format(meta, segments);

    // Assert
    EXPECT_NE(text.find("[00:00:00] First"), std::string::npos);
    EXPECT_NE(text.find("[00:00:02] Second"), std::string::npos);
}

TEST(TranscriptText, DropsTripleRepeats) {
    // Arrange
    TranscriptMeta meta{};
    meta.titleDateTime = "2026-11-01 13:18";
    meta.durationSeconds = 5.0;
    meta.recordedRange = "2026-11-01 13:18";
    meta.modelFile = "m.bin";
    meta.languageConfigured = "en";
    std::vector<TranscriptSegment> segments = {
        {0, 10, "thanks"},  {20, 30, "thanks"}, {40, 50, "thanks"},
        {60, 70, "thanks"}, {80, 90, "done"},
    };

    // Act
    const std::string text = TranscriptText::Format(meta, segments);

    // Assert
    EXPECT_NE(text.find("done"), std::string::npos);
    size_t count = 0;
    size_t pos = 0;
    while ((pos = text.find("thanks", pos)) != std::string::npos) {
        ++count;
        pos += 6;
    }
    EXPECT_LE(count, 2u); // at most two consecutive kept
}

TEST(TranscriptText, WriteHasNoBom) {
    // Arrange
    const auto path = std::filesystem::temp_directory_path() / "skrivli_txt_test.txt";

    // Act
    ASSERT_TRUE(TranscriptText::WriteUtf8NoBom(path.wstring(), "[00:00:00] hi\n"));

    // Assert
    {
        std::ifstream in(path, std::ios::binary);
        ASSERT_TRUE(in);
        std::string body((std::istreambuf_iterator<char>(in)), std::istreambuf_iterator<char>());
        EXPECT_EQ(body, "[00:00:00] hi\n");
    }
    std::error_code ec;
    std::filesystem::remove(path, ec);
}

TEST(TranscriptText, StripLegacyMarkdown) {
    // Arrange
    const std::string md = "# Meeting — 2026-11-01 13:18\n\n"
                           "- **Duration:** 00:00:42\n"
                           "- **Recorded:** 2026-11-01 13:18\n\n"
                           "## Transcript\n\n"
                           "**[00:00:03]** Okay, let's get started.\n\n"
                           "**[00:00:09]** First item.\n";

    // Act
    const auto plain = TranscriptText::StripLegacyMarkdown(md);

    // Assert
    EXPECT_EQ(plain.find("# Meeting"), std::string::npos);
    EXPECT_EQ(plain.find("Duration"), std::string::npos);
    EXPECT_EQ(plain.find("**"), std::string::npos);
    EXPECT_NE(plain.find("[00:00:03] Okay, let's get started."), std::string::npos);
    EXPECT_NE(plain.find("[00:00:09] First item."), std::string::npos);
}
