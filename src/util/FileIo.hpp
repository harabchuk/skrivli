#pragma once

#include <cstdint>
#include <optional>
#include <string>
#include <string_view>

namespace FileIo {

enum class StripBom {
    No,
    Yes,
};

enum class WriteMode {
    Direct,
    // Writes `<path>.tmp` then replaces the target, so readers never see a partial file.
    Atomic,
};

auto ReadFileBytes(const std::wstring& path, StripBom stripBom = StripBom::No) -> std::optional<std::string>;
bool WriteFileBytes(const std::wstring& path, std::string_view data, WriteMode mode = WriteMode::Direct);

// Creates the folder if needed and verifies a file can be created inside it.
bool IsWritableDirectory(const std::wstring& folder);

// Returns 0 when the file does not exist.
uint64_t FileSize(const std::wstring& path);

}  // namespace FileIo
