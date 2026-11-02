#pragma once

#include <windows.h>

#include <mmdeviceapi.h>
#include <objbase.h>
#include <propidl.h>
#include <string>
#include <vector>
#include <wil/com.h>

struct AudioDeviceInfo {
    std::wstring id;
    std::wstring name;
    bool isDefault = false;
};

class DeviceManager {
  public:
    DeviceManager() = default;
    ~DeviceManager();

    DeviceManager(const DeviceManager&) = delete;
    auto operator=(const DeviceManager&) -> DeviceManager& = delete;

    bool Initialize(HWND notifyHwnd);
    void Shutdown();

    auto EnumerateCapture() const -> std::vector<AudioDeviceInfo>;
    auto EnumerateRender() const -> std::vector<AudioDeviceInfo>;

    // Empty id → default for the flow. Missing saved id falls back to default.
    auto ResolveCaptureDevice(const std::wstring& savedId) const -> wil::com_ptr_nothrow<IMMDevice>;
    auto ResolveRenderDevice(const std::wstring& savedId) const -> wil::com_ptr_nothrow<IMMDevice>;

    auto FriendlyName(IMMDevice* device) const -> std::wstring;

  private:
    class NotificationClient;

    auto Enumerate(EDataFlow flow) const -> std::vector<AudioDeviceInfo>;
    auto DefaultId(EDataFlow flow, ERole role) const -> std::wstring;
    auto Resolve(EDataFlow flow, ERole defaultRole, const std::wstring& savedId) const
        -> wil::com_ptr_nothrow<IMMDevice>;
    auto GetDefault(EDataFlow flow, ERole role) const -> wil::com_ptr_nothrow<IMMDevice>;
    auto GetById(const std::wstring& id) const -> wil::com_ptr_nothrow<IMMDevice>;

    wil::com_ptr_nothrow<IMMDeviceEnumerator> m_enumerator;
    wil::com_ptr_nothrow<IMMNotificationClient> m_client;
    HWND m_notifyHwnd = nullptr;
};
