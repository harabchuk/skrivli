#include "infrastructure/storage/RecordingSidecar.hpp"

#include "core/AudioFormat.hpp"
#include "infrastructure/storage/RecordingLibrary.hpp"
#include "infrastructure/common/FileIo.hpp"
#include "infrastructure/common/Log.hpp"
#include "infrastructure/common/Text.hpp"

#include <glaze/glaze.hpp>

#include <windows.h>

#include <cstdio>

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

auto ForAudio(const std::wstring& audioPath) -> RecordingSidecar {
    RecordingSidecar side{};
    side.startLocal = FormatLocalTimestampNow();
    side.endLocal = side.startLocal;
    side.durationSeconds = RecordingLibrary::ReadWavDurationSeconds(audioPath);
    side.sampleRate = static_cast<int>(AudioFormat::kSampleRate);
    side.channels = static_cast<int>(AudioFormat::kChannels);
    return side;
}


auto FormatLocalTimestamp(const SYSTEMTIME& local) -> std::string {
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

auto FormatLocalTimestampNow() -> std::string {
    SYSTEMTIME local{};
    ::GetLocalTime(&local);
    return FormatLocalTimestamp(local);
}

bool Load(const std::wstring& jsonPath, RecordingSidecar& out) {
    const auto bytes = FileIo::ReadFileBytes(jsonPath);
    if (!bytes.has_value()) {
        return false;
    }
    const auto& json = *bytes;
    if (json.empty()) {
        return false;
    }

    RecordingSidecar data{};
    if (const auto ec = glz::read_json(data, json)) {
        Log::Error(L"Failed to parse sidecar JSON: " + Text::Utf8ToWide(glz::format_error(ec, json)));
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

    if (!FileIo::WriteFileBytes(jsonPath, body)) {
        Log::Error(L"Failed to write sidecar JSON");
        return false;
    }
    return true;
}

}  // namespace Sidecar
