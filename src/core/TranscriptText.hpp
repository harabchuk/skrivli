#pragma once

#include "core/Transcript.hpp"

#include <string>
#include <vector>

class TranscriptText {
  public:
    // Plain-text transcript with [hh:mm:ss] paragraph prefixes. Meta is unused
    // for the body but kept so call sites stay parallel to the old formatter.
    static auto Format(const TranscriptMeta& meta, const std::vector<TranscriptSegment>& segments)
        -> std::string;
    static bool WriteUtf8NoBom(const std::wstring& path, const std::string& utf8);

    // Turns a legacy Markdown transcript into displayable plain text.
    static auto StripLegacyMarkdown(const std::string& markdown) -> std::string;
};
