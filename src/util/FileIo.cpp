#include "util/FileIo.hpp"

#include "util/Paths.hpp"

#include <windows.h>

#include <filesystem>
#include <fstream>
#include <sstream>

namespace FileIo {

namespace {

constexpr unsigned char kBomBytes[] = {0xEF, 0xBB, 0xBF};

bool WriteDirect(const std::wstring& path, std::string_view data) {
    HANDLE file = ::CreateFileW(
        path.c_str(),
        GENERIC_WRITE,
        FILE_SHARE_READ,
        nullptr,
        CREATE_ALWAYS,
        FILE_ATTRIBUTE_NORMAL,
        nullptr);
    if (file == INVALID_HANDLE_VALUE) {
        return false;
    }
    DWORD written = 0;
    const BOOL ok = ::WriteFile(file, data.data(), static_cast<DWORD>(data.size()), &written, nullptr);
    ::CloseHandle(file);
    return ok != FALSE && written == data.size();
}

}  // namespace

std::optional<std::string> ReadFileBytes(const std::wstring& path, StripBom stripBom) {
    std::ifstream file(path, std::ios::binary);
    if (!file) {
        return std::nullopt;
    }
    std::ostringstream ss;
    ss << file.rdbuf();
    std::string bytes = ss.str();
    if (stripBom == StripBom::Yes
        && bytes.size() >= sizeof(kBomBytes)
        && static_cast<unsigned char>(bytes[0]) == kBomBytes[0]
        && static_cast<unsigned char>(bytes[1]) == kBomBytes[1]
        && static_cast<unsigned char>(bytes[2]) == kBomBytes[2]) {
        bytes.erase(0, sizeof(kBomBytes));
    }
    return bytes;
}

bool WriteFileBytes(const std::wstring& path, std::string_view data, WriteMode mode) {
    if (mode == WriteMode::Direct) {
        return WriteDirect(path, data);
    }

    const std::wstring temp = path + L".tmp";
    if (!WriteDirect(temp, data)) {
        ::DeleteFileW(temp.c_str());
        return false;
    }
    if (::MoveFileExW(temp.c_str(), path.c_str(), MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH) == FALSE) {
        ::DeleteFileW(temp.c_str());
        return false;
    }
    return true;
}

bool IsWritableDirectory(const std::wstring& folder) {
    if (!Paths::EnsureDirectory(folder)) {
        return false;
    }
    const std::filesystem::path probe = std::filesystem::path(folder) / L".meetingbuddy_write_test";
    HANDLE file = ::CreateFileW(
        probe.c_str(),
        GENERIC_WRITE,
        0,
        nullptr,
        CREATE_ALWAYS,
        FILE_ATTRIBUTE_TEMPORARY | FILE_FLAG_DELETE_ON_CLOSE,
        nullptr);
    if (file == INVALID_HANDLE_VALUE) {
        return false;
    }
    ::CloseHandle(file);
    return true;
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

}  // namespace FileIo
