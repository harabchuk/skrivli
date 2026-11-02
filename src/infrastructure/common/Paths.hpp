#pragma once

#include <string>

namespace Paths {

auto SettingsDir() -> std::wstring;
auto SettingsFile() -> std::wstring;
auto ModelsDir() -> std::wstring;
auto LogsDir() -> std::wstring;
auto RescueDir() -> std::wstring;
auto DefaultRecordingsDir() -> std::wstring;

bool EnsureDirectory(const std::wstring& path);
bool EnsureAppDataDirectories();

} // namespace Paths
