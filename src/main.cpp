#include "Application.hpp"

WTL::CAppModule _Module;

int main() {
    const HRESULT comResult = ::CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED);
    if (FAILED(comResult)) {
        return 1;
    }

    AtlInitCommonControls(
        ICC_STANDARD_CLASSES | ICC_WIN95_CLASSES | ICC_TAB_CLASSES | ICC_BAR_CLASSES | ICC_LISTVIEW_CLASSES);

    const HINSTANCE instance = ::GetModuleHandleW(nullptr);
    _Module.Init(nullptr, instance);

    Application app;
    const int result = app.Run(instance, SW_SHOWDEFAULT);

    _Module.Term();
    ::CoUninitialize();
    return result;
}
