#include "infrastructure/files/WavWriter.hpp"

#include <windows.h>

#include <filesystem>
#include <fstream>
#include <gtest/gtest.h>
#include <vector>

namespace {

std::wstring TempDir() {
    wchar_t path[MAX_PATH]{};
    const DWORD len = ::GetTempPathW(MAX_PATH, path);
    EXPECT_GT(len, 0u);
    const auto dir = std::filesystem::path(path) / L"SkrivliWavTests";
    std::filesystem::create_directories(dir);
    return dir.wstring();
}

} // namespace

TEST(WavWriter, WritesHeaderAndPatchesSizes) {
    // Arrange
    const std::wstring folder = TempDir();
    WavWriter writer;
    ASSERT_TRUE(writer.Open(folder));
    ASSERT_TRUE(writer.IsOpen());
    std::vector<float> frames(480 * 2, 0.25f);

    // Act
    ASSERT_TRUE(writer.WriteFloatFrames(frames.data(), 480));
    ASSERT_TRUE(writer.PatchHeader());
    std::wstring finalPath;
    ASSERT_TRUE(writer.Finalize(finalPath));

    // Assert
    ASSERT_TRUE(std::filesystem::exists(finalPath));
    {
        std::ifstream file(finalPath, std::ios::binary);
        ASSERT_TRUE(file.good());
        char riff[4]{};
        file.read(riff, 4);
        EXPECT_EQ(std::string(riff, 4), "RIFF");

        file.seekg(0, std::ios::end);
        const auto size = file.tellg();
        EXPECT_EQ(static_cast<std::streamoff>(size), static_cast<std::streamoff>(44 + 480 * 2 * 2));
    }

    std::error_code ec;
    std::filesystem::remove(finalPath, ec);
}

TEST(WavWriter, RepairPartialHeader) {
    // Arrange
    const std::wstring folder = TempDir();
    WavWriter writer;
    ASSERT_TRUE(writer.Open(folder));
    const std::wstring partialPath = writer.PartialPath();
    std::vector<float> frames(960 * 2, -0.2f);
    ASSERT_TRUE(writer.WriteFloatFrames(frames.data(), 960));
    writer.Abandon(); // closes without rename; partial remains on disk
    ASSERT_TRUE(std::filesystem::exists(partialPath));

    // Act
    ASSERT_TRUE(WavWriter::RepairPartialHeader(partialPath));

    // Assert
    {
        std::ifstream file(partialPath, std::ios::binary);
        ASSERT_TRUE(file.good());
        file.seekg(4);
        uint32_t riffSize = 0;
        file.read(reinterpret_cast<char*>(&riffSize), 4);
        file.seekg(40);
        uint32_t dataSize = 0;
        file.read(reinterpret_cast<char*>(&dataSize), 4);
        EXPECT_EQ(dataSize, 960u * 2u * 2u);
        EXPECT_EQ(riffSize, 36u + dataSize);
    }

    std::error_code ec;
    std::filesystem::remove(partialPath, ec);
}

TEST(WavWriter, RecoverPartialsInFolder) {
    // Arrange
    const std::wstring folder = TempDir() + L"\\Recover";
    std::filesystem::create_directories(folder);
    WavWriter writer;
    ASSERT_TRUE(writer.Open(folder));
    const std::wstring partialPath = writer.PartialPath();
    std::vector<float> frames(480 * 2, 0.1f);
    ASSERT_TRUE(writer.WriteFloatFrames(frames.data(), 480));
    writer.Abandon();
    ASSERT_TRUE(std::filesystem::exists(partialPath));

    // Act
    const auto recovered = WavWriter::RecoverPartialsInFolder(folder);

    // Assert
    ASSERT_EQ(recovered.size(), 1u);
    EXPECT_TRUE(std::filesystem::exists(recovered[0]));
    EXPECT_FALSE(std::filesystem::exists(partialPath));
    EXPECT_NE(recovered[0].find(L".recovered.wav"), std::wstring::npos);

    std::error_code ec;
    std::filesystem::remove_all(folder, ec);
}
