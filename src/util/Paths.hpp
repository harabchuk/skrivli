#pragma once

#include <string>

namespace Paths {

std::wstring SettingsDir();
std::wstring SettingsFile();
std::wstring ModelsDir();
std::wstring LogsDir();
std::wstring RescueDir();
std::wstring DefaultRecordingsDir();

bool EnsureDirectory(const std::wstring& path);
bool EnsureAppDataDirectories();

}  // namespace Paths
