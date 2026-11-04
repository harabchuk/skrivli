#include "infrastructure/common/FileIo.hpp"

#include "infrastructure/common/Paths.hpp"

#include <windows.h>

#include <filesystem>
#include <fstream>
#include <sstream>
#include <wil/resource.h>

namespace FileIo {

namespace {

constexpr unsigned char kBomBytes[] = {0xEF, 0xBB, 0xBF};

bool WriteDirect(const std::wstring& path, std::string_view data) {
    wil::unique_hfile file(::CreateFileW(path.c_str(), GENERIC_WRITE, FILE_SHARE_READ, nullptr,
                                         CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr));
    if (!file) {
        return false;
    }
    DWORD written = 0;
    const BOOL ok =
        ::WriteFile(file.get(), data.data(), static_cast<DWORD>(data.size()), &written, nullptr);
    return ok != FALSE && written == data.size();
}

} // namespace

auto ReadFileBytes(const std::wstring& path, StripBom stripBom) -> std::optional<std::string> {
    std::ifstream file(path, std::ios::binary);
    if (!file) {
        return std::nullopt;
    }
    std::ostringstream ss;
    ss << file.rdbuf();
    auto bytes = ss.str();
    if (stripBom == StripBom::Yes && bytes.size() >= sizeof(kBomBytes) &&
        static_cast<unsigned char>(bytes[0]) == kBomBytes[0] &&
        static_cast<unsigned char>(bytes[1]) == kBomBytes[1] &&
        static_cast<unsigned char>(bytes[2]) == kBomBytes[2]) {
        bytes.erase(0, sizeof(kBomBytes));
    }
    return bytes;
}

bool WriteFileBytes(const std::wstring& path, std::string_view data, WriteMode mode) {
    if (mode == WriteMode::Direct) {
        return WriteDirect(path, data);
    }

    const auto temp = path + L".tmp";
    if (!WriteDirect(temp, data)) {
        ::DeleteFileW(temp.c_str());
        return false;
    }
    if (::MoveFileExW(temp.c_str(), path.c_str(),
                      MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH) == FALSE) {
        ::DeleteFileW(temp.c_str());
        return false;
    }
    return true;
}

bool IsWritableDirectory(const std::wstring& folder) {
    if (!Paths::EnsureDirectory(folder)) {
        return false;
    }
    const auto probe = std::filesystem::path(folder) / L".skrivli_write_test";
    const wil::unique_hfile file(
        ::CreateFileW(probe.c_str(), GENERIC_WRITE, 0, nullptr, CREATE_ALWAYS,
                      FILE_ATTRIBUTE_TEMPORARY | FILE_FLAG_DELETE_ON_CLOSE, nullptr));
    return static_cast<bool>(file);
}

uint64_t FileSize(const std::wstring& path) {
    WIN32_FILE_ATTRIBUTE_DATA data{};
    if (!::GetFileAttributesExW(path.c_str(), GetFileExInfoStandard, &data)) {
        return 0;
    }
    ULARGE_INTEGER size{};
    size.HighPart = data.nFileSizeHigh;
    size.LowPart = data.nFileSizeLow;
    return size.QuadPart;
}

} // namespace FileIo
