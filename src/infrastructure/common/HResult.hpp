#pragma once

#include <windows.h>

namespace HResult {

HRESULT LogIfFailed(HRESULT hr, const wchar_t* context);
bool SucceededOrLog(HRESULT hr, const wchar_t* context);

} // namespace HResult
