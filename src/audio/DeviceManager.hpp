#pragma once

#include <mmdeviceapi.h>
#include <windows.h>

#include <string>
#include <vector>
#include <wrl/client.h>

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
    DeviceManager& operator=(const DeviceManager&) = delete;

    bool Initialize(HWND notifyHwnd);
    void Shutdown();

    std::vector<AudioDeviceInfo> EnumerateCapture() const;
    std::vector<AudioDeviceInfo> EnumerateRender() const;

    // Empty id → default for the flow. Missing saved id falls back to default.
    Microsoft::WRL::ComPtr<IMMDevice> ResolveCaptureDevice(const std::wstring& savedId) const;
    Microsoft::WRL::ComPtr<IMMDevice> ResolveRenderDevice(const std::wstring& savedId) const;

    std::wstring FriendlyName(IMMDevice* device) const;

private:
    class NotificationClient;

    std::vector<AudioDeviceInfo> Enumerate(EDataFlow flow) const;
    std::wstring DefaultId(EDataFlow flow, ERole role) const;
    Microsoft::WRL::ComPtr<IMMDevice> Resolve(
        EDataFlow flow,
        ERole defaultRole,
        const std::wstring& savedId) const;
    Microsoft::WRL::ComPtr<IMMDevice> GetDefault(EDataFlow flow, ERole role) const;
    Microsoft::WRL::ComPtr<IMMDevice> GetById(const std::wstring& id) const;

    Microsoft::WRL::ComPtr<IMMDeviceEnumerator> m_enumerator;
    Microsoft::WRL::ComPtr<IMMNotificationClient> m_client;
    HWND m_notifyHwnd = nullptr;
};
