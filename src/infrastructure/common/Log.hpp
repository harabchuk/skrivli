#pragma once

#include <string>
#include <windows.h>

namespace Log {

enum class Level {
    Info,
    Warn,
    Error,
};

void Info(const wchar_t* message);
void Warn(const wchar_t* message);
void Error(const wchar_t* message);

void Info(const std::wstring& message);
void Warn(const std::wstring& message);
void Error(const std::wstring& message);

void Write(Level level, const wchar_t* message);
void WriteHResult(Level level, const wchar_t* message, HRESULT hr);

}  // namespace Log
