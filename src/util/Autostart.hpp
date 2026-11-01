#pragma once

#include <string>

namespace Autostart {

constexpr wchar_t kRunValueName[] = L"MeetingBuddy";

// Creates or removes the HKCU Run entry for this executable.
// When enabled and startMinimized is true, the command includes --tray.
bool Apply(bool enable, bool startMinimized);

bool IsEnabled();

std::wstring CommandLineForRunKey(bool startMinimized);

}  // namespace Autostart
