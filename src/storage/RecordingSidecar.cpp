#include "storage/RecordingSidecar.hpp"

#include "util/Log.hpp"

#include <glaze/glaze.hpp>

#include <windows.h>

#include <cstdio>
#include <fstream>
#include <sstream>

template <>
struct glz::meta<TranscriptStatus> {
    static constexpr std::array keys{
        "none",
        "queued",
        "preparing",
        "transcribing",
        "ready",
        "failed",
        "cancelled"};
    static constexpr std::array value{
        TranscriptStatus::None,
        TranscriptStatus::Queued,
        TranscriptStatus::Preparing,
        TranscriptStatus::Transcribing,
        TranscriptStatus::Ready,
        TranscriptStatus::Failed,
        TranscriptStatus::Cancelled};
};

namespace Sidecar {

std::string WideToUtf8(const std::wstring& wide) {
    if (wide.empty()) {
        return {};
    }
    const int size = ::WideCharToMultiByte(CP_UTF8, 0, wide.c_str(), static_cast<int>(wide.size()), nullptr, 0, nullptr, nullptr);
    if (size <= 0) {
        return {};
    }
    std::string out(static_cast<size_t>(size), '\0');
    ::WideCharToMultiByte(CP_UTF8, 0, wide.c_str(), static_cast<int>(wide.size()), out.data(), size, nullptr, nullptr);
    return out;
}

std::wstring Utf8ToWide(const std::string& utf8) {
    if (utf8.empty()) {
        return {};
    }
    const int size = ::MultiByteToWideChar(CP_UTF8, 0, utf8.c_str(), static_cast<int>(utf8.size()), nullptr, 0);
    if (size <= 0) {
        return {};
    }
    std::wstring out(static_cast<size_t>(size), L'\0');
    ::MultiByteToWideChar(CP_UTF8, 0, utf8.c_str(), static_cast<int>(utf8.size()), out.data(), size);
    return out;
}

std::string FormatLocalTimestamp(const SYSTEMTIME& local) {
    char buf[32]{};
    std::snprintf(
        buf,
        sizeof(buf),
        "%04u-%02u-%02uT%02u:%02u:%02u",
        local.wYear,
        local.wMonth,
        local.wDay,
        local.wHour,
        local.wMinute,
        local.wSecond);
    return buf;
}

std::string FormatLocalTimestampNow() {
    SYSTEMTIME local{};
    ::GetLocalTime(&local);
    return FormatLocalTimestamp(local);
}

bool Load(const std::wstring& jsonPath, RecordingSidecar& out) {
    std::ifstream file(jsonPath, std::ios::binary);
    if (!file) {
        return false;
    }
    std::ostringstream ss;
    ss << file.rdbuf();
    const std::string json = ss.str();
    if (json.empty()) {
        return false;
    }

    RecordingSidecar data{};
    if (const auto ec = glz::read_json(data, json)) {
        Log::Error(L"Failed to parse sidecar JSON: " + Utf8ToWide(glz::format_error(ec, json)));
        return false;
    }
    out = std::move(data);
    return true;
}

bool Save(const std::wstring& jsonPath, const RecordingSidecar& data) {
    std::string body;
    if (const auto ec = glz::write<glz::opts{.prettify = true}>(data, body)) {
        Log::Error(L"Failed to serialize sidecar JSON");
        return false;
    }
    if (!body.empty() && body.back() != '\n') {
        body.push_back('\n');
    }

    HANDLE file = ::CreateFileW(
        jsonPath.c_str(),
        GENERIC_WRITE,
        FILE_SHARE_READ,
        nullptr,
        CREATE_ALWAYS,
        FILE_ATTRIBUTE_NORMAL,
        nullptr);
    if (file == INVALID_HANDLE_VALUE) {
        Log::Error(L"Failed to write sidecar JSON");
        return false;
    }
    DWORD written = 0;
    const BOOL ok = ::WriteFile(file, body.data(), static_cast<DWORD>(body.size()), &written, nullptr);
    ::CloseHandle(file);
    return ok != FALSE && written == body.size();
}

}  // namespace Sidecar
