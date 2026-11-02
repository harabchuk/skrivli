#pragma once

#include "core/Recording.hpp"

#include <windows.h>

#include <string>

namespace Sidecar {

bool Load(const std::wstring& jsonPath, RecordingSidecar& out);
bool Save(const std::wstring& jsonPath, const RecordingSidecar& data);

auto FormatLocalTimestamp(const SYSTEMTIME& local) -> std::string;
auto FormatLocalTimestampNow() -> std::string;

// start/end = now, duration read from the WAV header, internal sample rate and channels.
auto ForAudio(const std::wstring& audioPath) -> RecordingSidecar;

} // namespace Sidecar
