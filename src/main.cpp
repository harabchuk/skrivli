#include "Application.hpp"

WTL::CAppModule _Module;

int main() {
    const HRESULT comResult = ::CoInitialize(nullptr);
    if (FAILED(comResult)) {
        return 1;
    }

    AtlInitCommonControls(ICC_STANDARD_CLASSES | ICC_WIN95_CLASSES);

    const HINSTANCE instance = ::GetModuleHandle(nullptr);
    _Module.Init(nullptr, instance);

    Application app;
    const int result = app.Run(instance, SW_SHOWDEFAULT);

    _Module.Term();
    ::CoUninitialize();
    return result;
}
