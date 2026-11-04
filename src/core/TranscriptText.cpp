#include "core/TranscriptText.hpp"

#include "infrastructure/common/FileIo.hpp"
#include "infrastructure/common/Text.hpp"

#include <sstream>

namespace {

auto Trim(std::string text) -> std::string {
    const auto notSpace = [](unsigned char ch) {
        return ch != ' ' && ch != '\t' && ch != '\r' && ch != '\n';
    };
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

auto DedupAndTrim(const std::vector<TranscriptSegment>& segments)
    -> std::vector<TranscriptSegment> {
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

auto StripInlineMarkdown(std::string line) -> std::string {
    // Remove **bold** markers and a leading AT heading prefix.
    while (!line.empty() && line.front() == '#') {
        line.erase(line.begin());
    }
    line = Trim(line);
    std::string out;
    out.reserve(line.size());
    for (size_t i = 0; i < line.size();) {
        if (i + 1 < line.size() && line[i] == '*' && line[i + 1] == '*') {
            i += 2;
            continue;
        }
        out.push_back(line[i]);
        ++i;
    }
    return Trim(out);
}

} // namespace

auto TranscriptText::Format(const TranscriptMeta& /*meta*/,
                            const std::vector<TranscriptSegment>& segments) -> std::string {
    const auto cleaned = DedupAndTrim(segments);

    std::ostringstream ss;
    if (cleaned.empty()) {
        ss << "No speech detected.\n";
        return ss.str();
    }

    constexpr int64_t kPauseBreak = 150; // 1.5 s in 10 ms units
    constexpr int64_t kMaxSpan = 6000;   // ~60 s

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

        ss << '[' << FormatTimestamp(paraStart) << "] " << Trim(paragraph) << "\n\n";
    }

    return ss.str();
}

bool TranscriptText::WriteUtf8NoBom(const std::wstring& path, const std::string& utf8) {
    return FileIo::WriteFileBytes(path, utf8, FileIo::WriteMode::Direct);
}

auto TranscriptText::StripLegacyMarkdown(const std::string& markdown) -> std::string {
    std::istringstream in(markdown);
    std::ostringstream out;
    std::string line;
    bool wrote = false;
    while (std::getline(in, line)) {
        if (!line.empty() && line.back() == '\r') {
            line.pop_back();
        }
        const auto trimmed = Trim(line);
        if (trimmed.empty()) {
            continue;
        }
        // Drop metadata bullets and section headings.
        if (trimmed.starts_with("- ") || trimmed.starts_with("* ") || trimmed.starts_with("#")) {
            continue;
        }
        const auto plain = StripInlineMarkdown(trimmed);
        if (plain.empty()) {
            continue;
        }
        if (wrote) {
            out << "\n\n";
        }
        out << plain;
        wrote = true;
    }
    if (wrote) {
        out << '\n';
    }
    return out.str();
}
