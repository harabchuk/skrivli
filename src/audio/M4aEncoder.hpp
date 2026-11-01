#pragma once

#include <string>

class M4aEncoder {
public:
    // Encodes a PCM16 stereo 48 kHz WAV to AAC 128 kbps M4A via IMFSinkWriter.
    // Returns true and sets outM4aPath on success. Does not delete the WAV.
    static bool EncodeWavToM4a(const std::wstring& wavPath, std::wstring& outM4aPath, std::wstring& error);
};
