#include "infrastructure/audio/WavHeader.hpp"

#include <cstring>

namespace {

constexpr uint32_t kFormatOffset = 20;
constexpr uint32_t kChannelsOffset = 22;
constexpr uint32_t kSampleRateOffset = 24;
constexpr uint32_t kBitsOffset = 34;
constexpr uint32_t kDataSizeOffset = 40;
constexpr uint32_t kWaveTagOffset = 8;

template <typename T>
T ReadField(const char* header, uint32_t offset) {
    T value{};
    std::memcpy(&value, header + offset, sizeof(value));
    return value;
}

}  // namespace

auto WavHeader::Read(HANDLE file) -> std::optional<WavHeader> {
    if (file == INVALID_HANDLE_VALUE || file == nullptr) {
        return std::nullopt;
    }

    LARGE_INTEGER start{};
    if (!::SetFilePointerEx(file, start, nullptr, FILE_BEGIN)) {
        return std::nullopt;
    }

    char header[kHeaderBytes]{};
    DWORD read = 0;
    if (!::ReadFile(file, header, kHeaderBytes, &read, nullptr) || read != kHeaderBytes) {
        return std::nullopt;
    }
    if (std::memcmp(header, "RIFF", 4) != 0 || std::memcmp(header + kWaveTagOffset, "WAVE", 4) != 0) {
        return std::nullopt;
    }

    WavHeader result;
    result.format = ReadField<uint16_t>(header, kFormatOffset);
    result.channels = ReadField<uint16_t>(header, kChannelsOffset);
    result.sampleRate = ReadField<uint32_t>(header, kSampleRateOffset);
    result.bits = ReadField<uint16_t>(header, kBitsOffset);
    result.dataSize = ReadField<uint32_t>(header, kDataSizeOffset);
    return result;
}

double WavHeader::DurationSeconds() const {
    if (channels == 0 || sampleRate == 0 || bits == 0) {
        return 0.0;
    }
    const uint64_t bytesPerSecond = static_cast<uint64_t>(sampleRate) * channels * (bits / 8);
    if (bytesPerSecond == 0) {
        return 0.0;
    }
    return static_cast<double>(dataSize) / static_cast<double>(bytesPerSecond);
}
