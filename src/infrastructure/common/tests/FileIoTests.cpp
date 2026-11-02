#include "infrastructure/common/FileIo.hpp"

#include <gtest/gtest.h>

#include <filesystem>
#include <fstream>
#include <string>

namespace {

class FileIoTest : public ::testing::Test {
protected:
    void SetUp() override {
        m_dir = std::filesystem::temp_directory_path() / L"MeetingBuddyFileIoTests";
        std::filesystem::remove_all(m_dir);
        std::filesystem::create_directories(m_dir);
    }

    void TearDown() override {
        std::error_code ec;
        std::filesystem::remove_all(m_dir, ec);
    }

    std::wstring Path(const wchar_t* leaf) const {
        return (m_dir / leaf).wstring();
    }

    std::filesystem::path m_dir;
};

const std::string kBom = "\xEF\xBB\xBF";

}  // namespace

TEST_F(FileIoTest, WriteThenReadRoundTripsBytes) {
    // Arrange
    const std::wstring path = Path(L"direct.bin");
    const std::string data("a\0b\r\n\xC3\xA9", 7);

    // Act
    const bool written = FileIo::WriteFileBytes(path, data);
    const auto read = FileIo::ReadFileBytes(path);

    // Assert
    ASSERT_TRUE(written);
    ASSERT_TRUE(read.has_value());
    EXPECT_EQ(*read, data);
}

TEST_F(FileIoTest, DirectWriteReplacesExistingContent) {
    const std::wstring path = Path(L"replace.txt");
    ASSERT_TRUE(FileIo::WriteFileBytes(path, "long original content"));

    ASSERT_TRUE(FileIo::WriteFileBytes(path, "short"));

    EXPECT_EQ(FileIo::ReadFileBytes(path).value(), "short");
}

TEST_F(FileIoTest, AtomicWriteReplacesTargetAndLeavesNoTemp) {
    // Arrange
    const std::wstring path = Path(L"atomic.json");
    ASSERT_TRUE(FileIo::WriteFileBytes(path, "old"));

    // Act
    const bool ok = FileIo::WriteFileBytes(path, "new content", FileIo::WriteMode::Atomic);

    // Assert
    ASSERT_TRUE(ok);
    EXPECT_EQ(FileIo::ReadFileBytes(path).value(), "new content");
    EXPECT_FALSE(std::filesystem::exists(path + L".tmp"));
}

TEST_F(FileIoTest, AtomicWriteCreatesMissingTarget) {
    const std::wstring path = Path(L"fresh.json");

    ASSERT_TRUE(FileIo::WriteFileBytes(path, "x", FileIo::WriteMode::Atomic));

    EXPECT_EQ(FileIo::ReadFileBytes(path).value(), "x");
}

TEST_F(FileIoTest, WriteToMissingDirectoryFails) {
    const std::wstring path = Path(L"missing\\file.txt");

    EXPECT_FALSE(FileIo::WriteFileBytes(path, "x"));
    EXPECT_FALSE(FileIo::WriteFileBytes(path, "x", FileIo::WriteMode::Atomic));
}

TEST_F(FileIoTest, ReadMissingFileReturnsNullopt) {
    EXPECT_FALSE(FileIo::ReadFileBytes(Path(L"nope.txt")).has_value());
}

TEST_F(FileIoTest, ReadEmptyFileReturnsEmptyString) {
    const std::wstring path = Path(L"empty.txt");
    ASSERT_TRUE(FileIo::WriteFileBytes(path, ""));

    const auto read = FileIo::ReadFileBytes(path);

    ASSERT_TRUE(read.has_value());
    EXPECT_TRUE(read->empty());
}

TEST_F(FileIoTest, ReadKeepsBomByDefault) {
    const std::wstring path = Path(L"bom.txt");
    ASSERT_TRUE(FileIo::WriteFileBytes(path, kBom + "hi"));

    EXPECT_EQ(FileIo::ReadFileBytes(path).value(), kBom + "hi");
    EXPECT_EQ(FileIo::ReadFileBytes(path, FileIo::StripBom::No).value(), kBom + "hi");
}

TEST_F(FileIoTest, ReadStripsLeadingBomWhenRequested) {
    const std::wstring path = Path(L"bom.txt");
    ASSERT_TRUE(FileIo::WriteFileBytes(path, kBom + "hi"));

    EXPECT_EQ(FileIo::ReadFileBytes(path, FileIo::StripBom::Yes).value(), "hi");
}

TEST_F(FileIoTest, StripBomLeavesContentWithoutBomAlone) {
    const std::wstring path = Path(L"plain.txt");
    ASSERT_TRUE(FileIo::WriteFileBytes(path, "\xEF\xBBx"));

    EXPECT_EQ(FileIo::ReadFileBytes(path, FileIo::StripBom::Yes).value(), "\xEF\xBBx");
}

TEST_F(FileIoTest, IsWritableDirectoryCreatesMissingFolderAndLeavesNoProbe) {
    const std::wstring folder = Path(L"new\\nested");

    EXPECT_TRUE(FileIo::IsWritableDirectory(folder));

    EXPECT_TRUE(std::filesystem::is_directory(folder));
    EXPECT_TRUE(std::filesystem::is_empty(folder));
}

TEST_F(FileIoTest, IsWritableDirectoryRejectsPathThatIsAFile) {
    const std::wstring file = Path(L"file.txt");
    ASSERT_TRUE(FileIo::WriteFileBytes(file, "x"));

    EXPECT_FALSE(FileIo::IsWritableDirectory(file));
}

TEST_F(FileIoTest, FileSizeReportsBytesAndZeroForMissing) {
    const std::wstring path = Path(L"size.bin");
    ASSERT_TRUE(FileIo::WriteFileBytes(path, std::string(1234, 'z')));

    EXPECT_EQ(FileIo::FileSize(path), 1234u);
    EXPECT_EQ(FileIo::FileSize(Path(L"absent.bin")), 0u);
}
