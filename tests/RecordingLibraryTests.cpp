#include "storage/RecordingLibrary.hpp"
#include "storage/RecordingSidecar.hpp"
#include "audio/WavWriter.hpp"

#include <gtest/gtest.h>

#include <filesystem>
#include <fstream>
#include <vector>
#include <windows.h>

namespace {

std::wstring TempDir() {
    wchar_t path[MAX_PATH]{};
    const DWORD len = ::GetTempPathW(MAX_PATH, path);
    EXPECT_GT(len, 0u);
    const auto dir = std::filesystem::path(path) / L"MeetingBuddyLibraryTests";
    std::filesystem::create_directories(dir);
    return dir.wstring();
}

std::wstring WriteTinyWav(const std::wstring& folder, const wchar_t* leaf) {
    const std::wstring path = (std::filesystem::path(folder) / leaf).wstring();
    WavWriter writer;
    // Open creates its own timestamped name; write a fixed file instead.
    std::ofstream file(path, std::ios::binary);
    // Minimal PCM WAV header + 480 stereo frames of silence (int16).
#pragma pack(push, 1)
    struct Hdr {
        char riff[4] = {'R', 'I', 'F', 'F'};
        uint32_t riffSize = 36 + 480 * 2 * 2;
        char wave[4] = {'W', 'A', 'V', 'E'};
        char fmt[4] = {'f', 'm', 't', ' '};
        uint32_t fmtSize = 16;
        uint16_t audioFormat = 1;
        uint16_t channels = 2;
        uint32_t sampleRate = 48000;
        uint32_t byteRate = 48000 * 2 * 2;
        uint16_t blockAlign = 4;
        uint16_t bitsPerSample = 16;
        char data[4] = {'d', 'a', 't', 'a'};
        uint32_t dataSize = 480 * 2 * 2;
    };
#pragma pack(pop)
    Hdr hdr{};
    file.write(reinterpret_cast<const char*>(&hdr), sizeof(hdr));
    std::vector<char> zeros(hdr.dataSize, 0);
    file.write(zeros.data(), static_cast<std::streamsize>(zeros.size()));
    file.close();
    return path;
}

}  // namespace

TEST(RecordingLibrary, PairsSidecarsAndSortsNewestFirst) {
    // Arrange
    const std::wstring folder = TempDir() + L"\\Scan";
    std::filesystem::remove_all(folder);
    std::filesystem::create_directories(folder);
    const std::wstring older = WriteTinyWav(folder, L"MeetingBuddy_2026-01-01_10-00-00.wav");
    const std::wstring newer = WriteTinyWav(folder, L"MeetingBuddy_2026-02-01_11-00-00.wav");
    RecordingSidecar side{};
    side.startLocal = "2026-02-01T11:00:00";
    side.endLocal = "2026-02-01T11:00:10";
    side.durationSeconds = 10.0;
    side.micDevice = "Mic";
    side.transcriptStatus = TranscriptStatus::None;
    ASSERT_TRUE(RecordingLibrary::WriteSidecarForAudio(newer, side));
    {
        std::ofstream md(RecordingLibrary::MdPathForAudio(older), std::ios::binary);
        md << "# Meeting\n"; // sidecar → Ready
    }

    // Act
    const auto entries = RecordingLibrary::Scan(folder);

    // Assert
    ASSERT_GE(entries.size(), 2u);
    EXPECT_EQ(entries[0].audioPath, newer);
    EXPECT_EQ(entries[0].startLocal, L"2026-02-01T11:00:00");
    EXPECT_DOUBLE_EQ(entries[0].durationSeconds, 10.0);
    bool foundOlder = false;
    for (const auto& e : entries) {
        if (e.audioPath == older) {
            foundOlder = true;
            EXPECT_TRUE(e.hasMarkdown);
            EXPECT_EQ(e.transcriptStatus, TranscriptStatus::Ready);
        }
    }
    EXPECT_TRUE(foundOlder);

    std::error_code ec;
    std::filesystem::remove_all(folder, ec);
}

TEST(RecordingLibrary, MissingSidecarUsesFileNameTimestamp) {
    // Arrange
    const std::wstring folder = TempDir() + L"\\NoSide";
    std::filesystem::remove_all(folder);
    std::filesystem::create_directories(folder);
    const std::wstring wav = WriteTinyWav(folder, L"MeetingBuddy_2026-03-15_08-30-45.wav");

    // Act
    const auto entries = RecordingLibrary::Scan(folder);

    // Assert
    ASSERT_EQ(entries.size(), 1u);
    EXPECT_EQ(entries[0].audioPath, wav);
    EXPECT_EQ(entries[0].startLocal, L"2026-03-15T08:30:45");
    EXPECT_GT(entries[0].durationSeconds, 0.0);

    std::error_code ec;
    std::filesystem::remove_all(folder, ec);
}

TEST(RecordingLibrary, RecoverPartialThenScan) {
    // Arrange
    const std::wstring folder = TempDir() + L"\\Partial";
    std::filesystem::remove_all(folder);
    std::filesystem::create_directories(folder);
    WavWriter writer;
    ASSERT_TRUE(writer.Open(folder));
    const std::wstring partial = writer.PartialPath();
    std::vector<float> frames(480 * 2, 0.05f);
    ASSERT_TRUE(writer.WriteFloatFrames(frames.data(), 480));
    writer.Abandon();
    ASSERT_TRUE(std::filesystem::exists(partial));

    // Act
    const auto entries = RecordingLibrary::Scan(folder);

    // Assert
    EXPECT_FALSE(std::filesystem::exists(partial));
    ASSERT_EQ(entries.size(), 1u);
    EXPECT_NE(entries[0].audioPath.find(L".recovered.wav"), std::wstring::npos);

    std::error_code ec;
    std::filesystem::remove_all(folder, ec);
}

TEST(RecordingLibrary, RenameKeepsSidecarsTogether) {
    // Arrange
    const std::wstring folder = TempDir() + L"\\Rename";
    std::filesystem::remove_all(folder);
    std::filesystem::create_directories(folder);
    const std::wstring wav = WriteTinyWav(folder, L"MeetingBuddy_2026-04-01_12-00-00.wav");
    RecordingSidecar side{};
    side.startLocal = "2026-04-01T12:00:00";
    side.durationSeconds = 1.0;
    ASSERT_TRUE(RecordingLibrary::WriteSidecarForAudio(wav, side));
    {
        std::ofstream md(RecordingLibrary::MdPathForAudio(wav), std::ios::binary);
        md << "hi";
    }
    auto entries = RecordingLibrary::Scan(folder);
    ASSERT_EQ(entries.size(), 1u);
    std::wstring error;

    // Act
    ASSERT_TRUE(RecordingLibrary::Rename(entries[0], L"RenamedMeeting", error)) << error;

    // Assert
    EXPECT_TRUE(std::filesystem::exists((std::filesystem::path(folder) / L"RenamedMeeting.wav").wstring()));
    EXPECT_TRUE(std::filesystem::exists((std::filesystem::path(folder) / L"RenamedMeeting.json").wstring()));
    EXPECT_TRUE(std::filesystem::exists((std::filesystem::path(folder) / L"RenamedMeeting.md").wstring()));

    std::error_code ec;
    std::filesystem::remove_all(folder, ec);
}
