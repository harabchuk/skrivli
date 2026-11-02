#include "audio/WavHeader.hpp"

#include <gtest/gtest.h>

#include <cstring>
#include <filesystem>
#include <string>
#include <vector>
#include <windows.h>

namespace {

std::filesystem::path TempFile(const wchar_t* name) {
    wchar_t path[MAX_PATH]{};
    const DWORD len = ::GetTempPathW(MAX_PATH, path);
    EXPECT_GT(len, 0u);
    const auto dir = std::filesystem::path(path) / L"MeetingBuddyWavHeaderTests";
    std::filesystem::create_directories(dir);
    return dir / name;
}

std::vector<char> MakeHeader(
    uint16_t format,
    uint16_t channels,
    uint32_t sampleRate,
    uint16_t bits,
    uint32_t dataSize,
    const char* riff = "RIFF") {
    std::vector<char> header(44, 0);
    std::memcpy(header.data(), riff, 4);
    std::memcpy(header.data() + 8, "WAVE", 4);
    std::memcpy(header.data() + 12, "fmt ", 4);
    std::memcpy(header.data() + 20, &format, sizeof(format));
    std::memcpy(header.data() + 22, &channels, sizeof(channels));
    std::memcpy(header.data() + 24, &sampleRate, sizeof(sampleRate));
    std::memcpy(header.data() + 34, &bits, sizeof(bits));
    std::memcpy(header.data() + 36, "data", 4);
    std::memcpy(header.data() + 40, &dataSize, sizeof(dataSize));
    return header;
}

class TempWav {
public:
    TempWav(const wchar_t* name, const std::vector<char>& bytes) : m_path(TempFile(name)) {
        m_handle = ::CreateFileW(
            m_path.c_str(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ,
            nullptr,
            CREATE_ALWAYS,
            FILE_ATTRIBUTE_NORMAL,
            nullptr);
        EXPECT_NE(m_handle, INVALID_HANDLE_VALUE);
        DWORD written = 0;
        EXPECT_TRUE(::WriteFile(m_handle, bytes.data(), static_cast<DWORD>(bytes.size()), &written, nullptr));
    }

    ~TempWav() {
        if (m_handle != INVALID_HANDLE_VALUE) {
            ::CloseHandle(m_handle);
        }
        std::error_code ec;
        std::filesystem::remove(m_path, ec);
    }

    TempWav(const TempWav&) = delete;
    TempWav& operator=(const TempWav&) = delete;

    HANDLE Handle() const { return m_handle; }

private:
    std::filesystem::path m_path;
    HANDLE m_handle = INVALID_HANDLE_VALUE;
};

}  // namespace

TEST(WavHeader, ReadsCanonicalHeaderFields) {
    // Arrange
    TempWav wav(L"canonical.wav", MakeHeader(1, 2, 48000, 16, 192000));

    // Act
    const auto header = WavHeader::Read(wav.Handle());

    // Assert
    ASSERT_TRUE(header.has_value());
    EXPECT_EQ(header->format, 1);
    EXPECT_EQ(header->channels, 2);
    EXPECT_EQ(header->sampleRate, 48000u);
    EXPECT_EQ(header->bits, 16);
    EXPECT_EQ(header->dataSize, 192000u);
    EXPECT_DOUBLE_EQ(header->DurationSeconds(), 1.0);
}

TEST(WavHeader, LeavesFilePointerAtFirstPcmByte) {
    // Arrange
    std::vector<char> bytes = MakeHeader(1, 2, 48000, 16, 4);
    bytes.push_back(0x11);
    bytes.push_back(0x22);
    bytes.push_back(0x33);
    bytes.push_back(0x44);
    TempWav wav(L"pointer.wav", bytes);

    // Act
    ASSERT_TRUE(WavHeader::Read(wav.Handle()).has_value());
    char firstSample = 0;
    DWORD read = 0;
    ASSERT_TRUE(::ReadFile(wav.Handle(), &firstSample, 1, &read, nullptr));

    // Assert
    EXPECT_EQ(read, 1u);
    EXPECT_EQ(firstSample, 0x11);
}

TEST(WavHeader, RejectsMissingRiffTag) {
    // Arrange
    TempWav wav(L"notriff.wav", MakeHeader(1, 2, 48000, 16, 0, "RIFX"));

    // Act
    const auto header = WavHeader::Read(wav.Handle());

    // Assert
    EXPECT_FALSE(header.has_value());
}

TEST(WavHeader, RejectsFileShorterThanHeader) {
    // Arrange
    TempWav wav(L"short.wav", std::vector<char>(20, 0));

    // Act
    const auto header = WavHeader::Read(wav.Handle());

    // Assert
    EXPECT_FALSE(header.has_value());
}

TEST(WavHeader, RejectsInvalidHandle) {
    EXPECT_FALSE(WavHeader::Read(INVALID_HANDLE_VALUE).has_value());
}

TEST(WavHeader, DurationIsZeroWhenFormatIsIncomplete) {
    // Arrange
    WavHeader header;
    header.dataSize = 1000;

    // Act
    const double duration = header.DurationSeconds();

    // Assert
    EXPECT_DOUBLE_EQ(duration, 0.0);
}

TEST(WavHeader, DurationUsesBytesPerSecond) {
    // Arrange
    WavHeader header;
    header.channels = 1;
    header.sampleRate = 16000;
    header.bits = 16;
    header.dataSize = 16000;

    // Act
    const double duration = header.DurationSeconds();

    // Assert
    EXPECT_DOUBLE_EQ(duration, 0.5);
}
