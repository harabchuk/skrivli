#include "core/MarkdownFormatter.hpp"

#include "infrastructure/common/FileIo.hpp"
#include "infrastructure/common/Text.hpp"

#include <algorithm>
#include <cmath>
#include <cstdio>
#include <sstream>

namespace {

auto Trim(std::string text) -> std::string {
    const auto notSpace = [](unsigned char ch) { return ch != ' ' && ch != '\t' && ch != '\r' && ch != '\n'; };
    while (!text.empty() && !notSpace(static_cast<unsigned char>(text.front()))) {
        text.erase(text.begin());
    }
    while (!text.empty() && !notSpace(static_cast<unsigned char>(text.back()))) {
        text.pop_back();
    }
    return text;
}

auto FormatTimestamp(int64_t tenthsOfMs) -> std::string {
    if (tenthsOfMs < 0) {
        tenthsOfMs = 0;
    }
    return Text::FormatHms<std::string>(static_cast<uint64_t>(tenthsOfMs / 100));
}

auto FormatDuration(double seconds) -> std::string {
    if (seconds < 0.0) {
        seconds = 0.0;
    }
    return Text::FormatHms<std::string>(static_cast<uint64_t>(std::llround(seconds)));
}

auto DedupAndTrim(const std::vector<TranscriptSegment>& segments) -> std::vector<TranscriptSegment> {
    std::vector<TranscriptSegment> out;
    out.reserve(segments.size());
    int repeat = 0;
    std::string last;
    for (const auto& seg : segments) {
        TranscriptSegment cleaned = seg;
        cleaned.text = Trim(cleaned.text);
        if (cleaned.text.empty()) {
            continue;
        }
        if (!out.empty() && cleaned.text == last) {
            ++repeat;
            if (repeat >= 2) {
                // Drop 3rd+ identical consecutive line.
                continue;
            }
        } else {
            repeat = 0;
            last = cleaned.text;
        }
        out.push_back(std::move(cleaned));
    }
    return out;
}

}  // namespace

auto MarkdownFormatter::Format(const TranscriptMeta& meta, const std::vector<TranscriptSegment>& segments) -> std::string {
    const auto cleaned = DedupAndTrim(segments);

    std::ostringstream ss;
    ss << "# Meeting — " << meta.titleDateTime << "\n\n";
    ss << "- **Duration:** " << FormatDuration(meta.durationSeconds) << "\n";
    ss << "- **Recorded:** " << meta.recordedRange << "\n";
    ss << "- **Model:** " << meta.modelFile << " (language: " << meta.languageConfigured;
    if (!meta.languageDetected.empty() && meta.languageDetected != meta.languageConfigured) {
        ss << " → " << meta.languageDetected;
    }
    ss << ")\n\n";
    ss << "## Transcript\n\n";

    if (cleaned.empty()) {
        ss << "_No speech detected._\n";
        return ss.str();
    }

    constexpr int64_t kPauseBreak = 150;   // 1.5 s in 10 ms units
    constexpr int64_t kMaxSpan = 6000;    // ~60 s

    size_t i = 0;
    while (i < cleaned.size()) {
        const int64_t paraStart = cleaned[i].t0;
        std::string paragraph = cleaned[i].text;
        int64_t paraEnd = cleaned[i].t1;
        ++i;

        while (i < cleaned.size()) {
            const int64_t gap = cleaned[i].t0 - paraEnd;
            const int64_t span = cleaned[i].t1 - paraStart;
            if (gap > kPauseBreak || span > kMaxSpan) {
                break;
            }
            if (!paragraph.empty() && paragraph.back() != ' ') {
                paragraph.push_back(' ');
            }
            paragraph += cleaned[i].text;
            paraEnd = cleaned[i].t1;
            ++i;
        }

        ss << "**[" << FormatTimestamp(paraStart) << "]** " << Trim(paragraph) << "\n\n";
    }

    return ss.str();
}

bool MarkdownFormatter::WriteUtf8NoBom(const std::wstring& path, const std::string& utf8) {
    return FileIo::WriteFileBytes(path, utf8, FileIo::WriteMode::Direct);
}
