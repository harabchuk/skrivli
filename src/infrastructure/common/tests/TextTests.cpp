#include "infrastructure/common/Text.hpp"

#include <gtest/gtest.h>

TEST(Text, Utf8RoundTripPreservesNonAscii) {
    // Arrange
    const std::wstring wide = L"Meeting \u6D4B\u8BD5 \u2014 caf\u00E9";

    // Act
    const std::string utf8 = Text::WideToUtf8(wide);
    const std::wstring back = Text::Utf8ToWide(utf8);

    // Assert
    EXPECT_GT(utf8.size(), wide.size());
    EXPECT_EQ(back, wide);
}

TEST(Text, ConversionOfEmptyStringsIsEmpty) {
    EXPECT_TRUE(Text::WideToUtf8(L"").empty());
    EXPECT_TRUE(Text::Utf8ToWide("").empty());
}

TEST(Text, WideToUtf8EncodesAsMultiByte) {
    EXPECT_EQ(Text::WideToUtf8(L"\u00E9"), "\xC3\xA9");
}

TEST(Text, EndsWithIgnoreCaseMatchesSuffixCaseInsensitively) {
    EXPECT_TRUE(Text::EndsWithIgnoreCase(L"Meeting.WAV", L".wav"));
    EXPECT_TRUE(Text::EndsWithIgnoreCase(L"a.mic.recovered.wav", L".RECOVERED.wav"));
    EXPECT_FALSE(Text::EndsWithIgnoreCase(L"Meeting.wav", L".m4a"));
}

TEST(Text, EndsWithIgnoreCaseHandlesShortValueAndEmptySuffix) {
    EXPECT_FALSE(Text::EndsWithIgnoreCase(L"a", L".wav"));
    EXPECT_TRUE(Text::EndsWithIgnoreCase(L"a", L""));
    EXPECT_TRUE(Text::EndsWithIgnoreCase(L".wav", L".wav"));
}

TEST(Text, PathEqualsIgnoresCase) {
    EXPECT_TRUE(Text::PathEquals(L"C:\\Meetings\\A.wav", L"c:\\meetings\\a.WAV"));
    EXPECT_FALSE(Text::PathEquals(L"C:\\Meetings\\A.wav", L"C:\\Meetings\\B.wav"));
    EXPECT_TRUE(Text::PathEquals(L"", L""));
}

TEST(Text, FormatHmsPadsEachField) {
    EXPECT_EQ(Text::FormatHms(0), L"00:00:00");
    EXPECT_EQ(Text::FormatHms(59), L"00:00:59");
    EXPECT_EQ(Text::FormatHms(61), L"00:01:01");
    EXPECT_EQ(Text::FormatHms(3661), L"01:01:01");
}

TEST(Text, FormatHmsNarrowMatchesWide) {
    EXPECT_EQ(Text::FormatHms<std::string>(3661), "01:01:01");
    EXPECT_EQ(Text::FormatHms<std::string>(100 * 3600), "100:00:00");
}

TEST(Text, IsoToDisplayMinuteTruncatesAndReplacesSeparator) {
    EXPECT_EQ(Text::IsoToDisplayMinute(std::string("2026-11-01T13:18:00")), "2026-11-01 13:18");
    EXPECT_EQ(Text::IsoToDisplayMinute(std::wstring(L"2026-11-01T13:18:00")), L"2026-11-01 13:18");
}

TEST(Text, IsoToDisplayMinuteReturnsOtherInputUnchanged) {
    EXPECT_EQ(Text::IsoToDisplayMinute(std::string("")), "");
    EXPECT_EQ(Text::IsoToDisplayMinute(std::string("2026-11-01")), "2026-11-01");
    EXPECT_EQ(Text::IsoToDisplayMinute(std::wstring(L"2026-11-01 13:18:00")), L"2026-11-01 13:18:00");
}
