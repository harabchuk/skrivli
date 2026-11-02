#include "infrastructure/common/Log.hpp"

#include "infrastructure/common/Paths.hpp"

#include <cstdio>
#include <cwchar>
#include <filesystem>
#include <mutex>
#include <string>

namespace {

constexpr std::uintmax_t kMaxLogBytes = 1 * 1024 * 1024;
constexpr int kKeptRotatedFiles = 3;

auto Mutex() -> std::mutex& {
    static std::mutex mutex;
    return mutex;
}

auto LevelName(Log::Level level) -> const wchar_t* {
    switch (level) {
    case Log::Level::Info:
        return L"INFO";
    case Log::Level::Warn:
        return L"WARN";
    case Log::Level::Error:
        return L"ERROR";
    }
    return L"INFO";
}

auto Timestamp() -> std::wstring {
    SYSTEMTIME st{};
    ::GetLocalTime(&st);
    wchar_t buffer[64]{};
    std::swprintf(
        buffer,
        64,
        L"%04u-%02u-%02u %02u:%02u:%02u.%03u",
        st.wYear,
        st.wMonth,
        st.wDay,
        st.wHour,
        st.wMinute,
        st.wSecond,
        st.wMilliseconds);
    return buffer;
}

void RotateIfNeeded(const std::filesystem::path& logPath) {
    std::error_code ec;
    if (!std::filesystem::exists(logPath, ec)) {
        return;
    }
    const auto size = std::filesystem::file_size(logPath, ec);
    if (ec || size < kMaxLogBytes) {
        return;
    }

    for (int i = kKeptRotatedFiles - 1; i >= 1; --i) {
        const auto from = logPath.wstring() + L"." + std::to_wstring(i);
        const auto to = logPath.wstring() + L"." + std::to_wstring(i + 1);
        std::filesystem::remove(to, ec);
        std::filesystem::rename(from, to, ec);
    }

    const auto first = logPath.wstring() + L".1";
    std::filesystem::remove(first, ec);
    std::filesystem::rename(logPath, first, ec);
}

}  // namespace

namespace Log {

void Write(Level level, const wchar_t* message) {
    if (message == nullptr) {
        message = L"";
    }

    std::lock_guard lock(Mutex());
    Paths::EnsureDirectory(Paths::LogsDir());
    const auto logPath = std::filesystem::path(Paths::LogsDir()) / L"meetingbuddy.log";
    RotateIfNeeded(logPath);

    FILE* file = nullptr;
    if (_wfopen_s(&file, logPath.c_str(), L"a, ccs=UTF-8") != 0 || file == nullptr) {
        return;
    }

    const auto line = Timestamp() + L" [" + LevelName(level) + L"] " + message + L"\n";
    std::fputws(line.c_str(), file);
    std::fclose(file);
}

void WriteHResult(Level level, const wchar_t* message, HRESULT hr) {
    wchar_t buffer[512]{};
    std::swprintf(
        buffer,
        512,
        L"%s (hr=0x%08X)",
        message != nullptr ? message : L"",
        static_cast<unsigned>(hr));
    Write(level, buffer);
}

void Info(const wchar_t* message) {
    Write(Level::Info, message);
}

void Warn(const wchar_t* message) {
    Write(Level::Warn, message);
}

void Error(const wchar_t* message) {
    Write(Level::Error, message);
}

void Info(const std::wstring& message) {
    Write(Level::Info, message.c_str());
}

void Warn(const std::wstring& message) {
    Write(Level::Warn, message.c_str());
}

void Error(const std::wstring& message) {
    Write(Level::Error, message.c_str());
}

}  // namespace Log
