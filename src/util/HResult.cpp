#include "util/HResult.hpp"

#include "util/Log.hpp"

namespace HResult {

HRESULT LogIfFailed(HRESULT hr, const wchar_t* context) {
    if (FAILED(hr)) {
        Log::WriteHResult(Log::Level::Error, context, hr);
    }
    return hr;
}

bool SucceededOrLog(HRESULT hr, const wchar_t* context) {
    return SUCCEEDED(LogIfFailed(hr, context));
}

}  // namespace HResult
