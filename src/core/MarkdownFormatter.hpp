#pragma once

#include "core/Transcript.hpp"

#include <string>
#include <vector>

class MarkdownFormatter {
  public:
    static auto Format(const TranscriptMeta& meta, const std::vector<TranscriptSegment>& segments)
        -> std::string;
    static bool WriteUtf8NoBom(const std::wstring& path, const std::string& utf8);
};
