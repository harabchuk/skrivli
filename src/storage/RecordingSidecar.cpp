#include "storage/RecordingSidecar.hpp"

#include "util/Log.hpp"

#include <windows.h>

#include <cstdio>
#include <fstream>
#include <sstream>

namespace {

std::string EscapeJson(const std::string& text) {
    std::string out;
    out.reserve(text.size() + 8);
    for (char ch : text) {
        switch (ch) {
        case '\\': out += "\\\\"; break;
        case '"': out += "\\\""; break;
        case '\n': out += "\\n"; break;
        case '\r': out += "\\r"; break;
        case '\t': out += "\\t"; break;
        default:
            if (static_cast<unsigned char>(ch) < 0x20) {
                char buf[8]{};
                std::snprintf(buf, sizeof(buf), "\\u%04x", static_cast<unsigned char>(ch));
                out += buf;
            } else {
                out.push_back(ch);
            }
            break;
        }
    }
    return out;
}

bool ExtractString(const std::string& json, const char* key, std::string& out) {
    const std::string pattern = std::string("\"") + key + "\"";
    const size_t keyPos = json.find(pattern);
    if (keyPos == std::string::npos) {
        return false;
    }
    const size_t colon = json.find(':', keyPos + pattern.size());
    if (colon == std::string::npos) {
        return false;
    }
    const size_t firstQuote = json.find('"', colon + 1);
    if (firstQuote == std::string::npos) {
        return false;
    }
    std::string value;
    for (size_t i = firstQuote + 1; i < json.size(); ++i) {
        if (json[i] == '\\' && i + 1 < json.size()) {
            const char next = json[i + 1];
            if (next == '"' || next == '\\' || next == '/' ) {
                value.push_back(next);
                ++i;
            } else if (next == 'n') {
                value.push_back('\n');
                ++i;
            } else if (next == 'r') {
                value.push_back('\r');
                ++i;
            } else if (next == 't') {
                value.push_back('\t');
                ++i;
            } else {
                value.push_back(next);
                ++i;
            }
            continue;
        }
        if (json[i] == '"') {
            out = value;
            return true;
        }
        value.push_back(json[i]);
    }
    return false;
}

bool ExtractNumber(const std::string& json, const char* key, double& out) {
    const std::string pattern = std::string("\"") + key + "\"";
    const size_t keyPos = json.find(pattern);
    if (keyPos == std::string::npos) {
        return false;
    }
    const size_t colon = json.find(':', keyPos + pattern.size());
    if (colon == std::string::npos) {
        return false;
    }
    size_t i = colon + 1;
    while (i < json.size() && (json[i] == ' ' || json[i] == '\t')) {
        ++i;
    }
    char* end = nullptr;
    const double value = std::strtod(json.c_str() + i, &end);
    if (end == json.c_str() + i) {
        return false;
    }
    out = value;
    return true;
}

bool ExtractStringArray(const std::string& json, const char* key, std::vector<std::string>& out) {
    out.clear();
    const std::string pattern = std::string("\"") + key + "\"";
    const size_t keyPos = json.find(pattern);
    if (keyPos == std::string::npos) {
        return false;
    }
    const size_t colon = json.find(':', keyPos + pattern.size());
    if (colon == std::string::npos) {
        return false;
    }
    const size_t open = json.find('[', colon + 1);
    if (open == std::string::npos) {
        return false;
    }
    const size_t close = json.find(']', open + 1);
    if (close == std::string::npos) {
        return false;
    }
    const std::string body = json.substr(open + 1, close - open - 1);
    size_t pos = 0;
    while (pos < body.size()) {
        const size_t q1 = body.find('"', pos);
        if (q1 == std::string::npos) {
            break;
        }
        std::string value;
        size_t i = q1 + 1;
        for (; i < body.size(); ++i) {
            if (body[i] == '\\' && i + 1 < body.size()) {
                value.push_back(body[i + 1]);
                ++i;
                continue;
            }
            if (body[i] == '"') {
                break;
            }
            value.push_back(body[i]);
        }
        out.push_back(value);
        pos = i + 1;
    }
    return true;
}

}  // namespace

namespace Sidecar {

const char* ToString(TranscriptStatus status) {
    switch (status) {
    case TranscriptStatus::Queued: return "queued";
    case TranscriptStatus::Preparing: return "preparing";
    case TranscriptStatus::Transcribing: return "transcribing";
    case TranscriptStatus::Ready: return "ready";
    case TranscriptStatus::Failed: return "failed";
    case TranscriptStatus::Cancelled: return "cancelled";
    case TranscriptStatus::None:
    default: return "none";
    }
}

TranscriptStatus FromString(const std::string& text) {
    if (text == "queued") return TranscriptStatus::Queued;
    if (text == "preparing") return TranscriptStatus::Preparing;
    if (text == "transcribing") return TranscriptStatus::Transcribing;
    if (text == "ready") return TranscriptStatus::Ready;
    if (text == "failed") return TranscriptStatus::Failed;
    if (text == "cancelled") return TranscriptStatus::Cancelled;
    return TranscriptStatus::None;
}

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
    ExtractString(json, "startLocal", data.startLocal);
    ExtractString(json, "endLocal", data.endLocal);
    ExtractNumber(json, "durationSeconds", data.durationSeconds);
    double sampleRate = data.sampleRate;
    double channels = data.channels;
    ExtractNumber(json, "sampleRate", sampleRate);
    ExtractNumber(json, "channels", channels);
    data.sampleRate = static_cast<int>(sampleRate);
    data.channels = static_cast<int>(channels);
    ExtractString(json, "micDevice", data.micDevice);
    ExtractString(json, "systemDevice", data.systemDevice);
    ExtractStringArray(json, "deviceNotes", data.deviceNotes);
    std::string status;
    if (ExtractString(json, "transcriptStatus", status)) {
        data.transcriptStatus = FromString(status);
    }
    ExtractString(json, "transcriptError", data.transcriptError);
    ExtractString(json, "model", data.model);
    ExtractString(json, "language", data.language);
    out = std::move(data);
    return true;
}

bool Save(const std::wstring& jsonPath, const RecordingSidecar& data) {
    std::ostringstream ss;
    ss << "{\n";
    ss << "  \"startLocal\": \"" << EscapeJson(data.startLocal) << "\",\n";
    ss << "  \"endLocal\": \"" << EscapeJson(data.endLocal) << "\",\n";
    ss << "  \"durationSeconds\": " << data.durationSeconds << ",\n";
    ss << "  \"sampleRate\": " << data.sampleRate << ",\n";
    ss << "  \"channels\": " << data.channels << ",\n";
    ss << "  \"micDevice\": \"" << EscapeJson(data.micDevice) << "\",\n";
    ss << "  \"systemDevice\": \"" << EscapeJson(data.systemDevice) << "\",\n";
    ss << "  \"deviceNotes\": [";
    for (size_t i = 0; i < data.deviceNotes.size(); ++i) {
        if (i > 0) {
            ss << ", ";
        }
        ss << "\"" << EscapeJson(data.deviceNotes[i]) << "\"";
    }
    ss << "],\n";
    ss << "  \"transcriptStatus\": \"" << ToString(data.transcriptStatus) << "\",\n";
    ss << "  \"transcriptError\": \"" << EscapeJson(data.transcriptError) << "\",\n";
    ss << "  \"model\": \"" << EscapeJson(data.model) << "\",\n";
    ss << "  \"language\": \"" << EscapeJson(data.language) << "\"\n";
    ss << "}\n";

    const std::string body = ss.str();
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
