#include "audio/DeviceManager.hpp"

#include "Ids.hpp"
#include "util/HResult.hpp"
#include "util/Log.hpp"

#include <functiondiscoverykeys_devpkey.h>
#include <propvarutil.h>

class DeviceManager::NotificationClient final : public IMMNotificationClient {
public:
    explicit NotificationClient(HWND hwnd)
        : m_hwnd(hwnd) {}

    HRESULT STDMETHODCALLTYPE QueryInterface(REFIID riid, void** ppv) override {
        if (ppv == nullptr) {
            return E_POINTER;
        }
        if (riid == __uuidof(IUnknown) || riid == __uuidof(IMMNotificationClient)) {
            *ppv = static_cast<IMMNotificationClient*>(this);
            AddRef();
            return S_OK;
        }
        *ppv = nullptr;
        return E_NOINTERFACE;
    }

    ULONG STDMETHODCALLTYPE AddRef() override {
        return InterlockedIncrement(&m_ref);
    }

    ULONG STDMETHODCALLTYPE Release() override {
        const ULONG ref = InterlockedDecrement(&m_ref);
        if (ref == 0) {
            delete this;
        }
        return ref;
    }

    HRESULT STDMETHODCALLTYPE OnDeviceStateChanged(LPCWSTR /*deviceId*/, DWORD /*newState*/) override {
        PostChanged();
        return S_OK;
    }

    HRESULT STDMETHODCALLTYPE OnDeviceAdded(LPCWSTR /*deviceId*/) override {
        PostChanged();
        return S_OK;
    }

    HRESULT STDMETHODCALLTYPE OnDeviceRemoved(LPCWSTR /*deviceId*/) override {
        PostChanged();
        return S_OK;
    }

    HRESULT STDMETHODCALLTYPE OnDefaultDeviceChanged(
        EDataFlow /*flow*/,
        ERole /*role*/,
        LPCWSTR /*defaultDeviceId*/) override {
        PostChanged();
        return S_OK;
    }

    HRESULT STDMETHODCALLTYPE OnPropertyValueChanged(LPCWSTR /*deviceId*/, const PROPERTYKEY /*key*/) override {
        return S_OK;
    }

private:
    void PostChanged() const {
        if (m_hwnd != nullptr && ::IsWindow(m_hwnd)) {
            ::PostMessageW(m_hwnd, WM_APP_DEVICES_CHANGED, 0, 0);
        }
    }

    HWND m_hwnd = nullptr;
    LONG m_ref = 1;
};

DeviceManager::~DeviceManager() {
    Shutdown();
}

bool DeviceManager::Initialize(HWND notifyHwnd) {
    Shutdown();
    m_notifyHwnd = notifyHwnd;

    HRESULT hr = ::CoCreateInstance(
        __uuidof(MMDeviceEnumerator),
        nullptr,
        CLSCTX_ALL,
        __uuidof(IMMDeviceEnumerator),
        &m_enumerator);
    if (!HResult::SucceededOrLog(hr, L"CoCreateInstance(MMDeviceEnumerator)")) {
        return false;
    }

    m_client.Attach(new NotificationClient(notifyHwnd));
    hr = m_enumerator->RegisterEndpointNotificationCallback(m_client.Get());
    if (!HResult::SucceededOrLog(hr, L"RegisterEndpointNotificationCallback")) {
        m_client.Reset();
        m_enumerator.Reset();
        return false;
    }
    return true;
}

void DeviceManager::Shutdown() {
    if (m_enumerator && m_client) {
        m_enumerator->UnregisterEndpointNotificationCallback(m_client.Get());
    }
    m_client.Reset();
    m_enumerator.Reset();
    m_notifyHwnd = nullptr;
}

std::vector<AudioDeviceInfo> DeviceManager::EnumerateCapture() const {
    return Enumerate(eCapture);
}

std::vector<AudioDeviceInfo> DeviceManager::EnumerateRender() const {
    return Enumerate(eRender);
}

Microsoft::WRL::ComPtr<IMMDevice> DeviceManager::ResolveCaptureDevice(const std::wstring& savedId) const {
    return Resolve(eCapture, eCommunications, savedId);
}

Microsoft::WRL::ComPtr<IMMDevice> DeviceManager::ResolveRenderDevice(const std::wstring& savedId) const {
    return Resolve(eRender, eConsole, savedId);
}

std::wstring DeviceManager::DefaultId(EDataFlow flow, ERole role) const {
    auto device = GetDefault(flow, role);
    if (!device) {
        return {};
    }
    LPWSTR id = nullptr;
    if (FAILED(device->GetId(&id)) || id == nullptr) {
        return {};
    }
    std::wstring result(id);
    ::CoTaskMemFree(id);
    return result;
}

std::wstring DeviceManager::FriendlyName(IMMDevice* device) const {
    if (device == nullptr) {
        return {};
    }

    Microsoft::WRL::ComPtr<IPropertyStore> store;
    HRESULT hr = device->OpenPropertyStore(STGM_READ, &store);
    if (FAILED(hr) || !store) {
        return L"(Unknown device)";
    }

    PROPVARIANT var{};
    PropVariantInit(&var);
    hr = store->GetValue(PKEY_Device_FriendlyName, &var);
    std::wstring name = L"(Unknown device)";
    if (SUCCEEDED(hr) && var.vt == VT_LPWSTR && var.pwszVal != nullptr) {
        name = var.pwszVal;
    }
    PropVariantClear(&var);
    return name;
}

std::vector<AudioDeviceInfo> DeviceManager::Enumerate(EDataFlow flow) const {
    std::vector<AudioDeviceInfo> devices;
    if (!m_enumerator) {
        return devices;
    }

    const std::wstring defaultId = DefaultId(flow, flow == eCapture ? eCommunications : eConsole);

    Microsoft::WRL::ComPtr<IMMDeviceCollection> collection;
    const HRESULT hr = m_enumerator->EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE, &collection);
    if (FAILED(hr) || !collection) {
        HResult::LogIfFailed(hr, L"EnumAudioEndpoints");
        return devices;
    }

    UINT count = 0;
    collection->GetCount(&count);
    devices.reserve(count + 1);

    AudioDeviceInfo defaultEntry{};
    defaultEntry.id.clear();
    defaultEntry.isDefault = true;
    defaultEntry.name = (flow == eCapture) ? L"Default communications device" : L"Default output device";
    devices.push_back(defaultEntry);

    for (UINT i = 0; i < count; ++i) {
        Microsoft::WRL::ComPtr<IMMDevice> device;
        if (FAILED(collection->Item(i, &device)) || !device) {
            continue;
        }

        LPWSTR id = nullptr;
        if (FAILED(device->GetId(&id)) || id == nullptr) {
            continue;
        }

        AudioDeviceInfo info{};
        info.id = id;
        info.name = FriendlyName(device.Get());
        info.isDefault = (!defaultId.empty() && info.id == defaultId);
        ::CoTaskMemFree(id);
        devices.push_back(std::move(info));
    }

    return devices;
}

Microsoft::WRL::ComPtr<IMMDevice> DeviceManager::Resolve(
    EDataFlow flow,
    ERole defaultRole,
    const std::wstring& savedId) const {
    if (!savedId.empty()) {
        auto device = GetById(savedId);
        if (device) {
            DWORD state = 0;
            if (SUCCEEDED(device->GetState(&state)) && state == DEVICE_STATE_ACTIVE) {
                return device;
            }
        }
        Log::Warn(L"Saved audio device missing; falling back to default");
    }
    return GetDefault(flow, defaultRole);
}

Microsoft::WRL::ComPtr<IMMDevice> DeviceManager::GetDefault(EDataFlow flow, ERole role) const {
    Microsoft::WRL::ComPtr<IMMDevice> device;
    if (!m_enumerator) {
        return device;
    }
    const HRESULT hr = m_enumerator->GetDefaultAudioEndpoint(flow, role, &device);
    if (FAILED(hr)) {
        HResult::LogIfFailed(hr, L"GetDefaultAudioEndpoint");
    }
    return device;
}

Microsoft::WRL::ComPtr<IMMDevice> DeviceManager::GetById(const std::wstring& id) const {
    Microsoft::WRL::ComPtr<IMMDevice> device;
    if (!m_enumerator || id.empty()) {
        return device;
    }
    const HRESULT hr = m_enumerator->GetDevice(id.c_str(), &device);
    if (FAILED(hr)) {
        return {};
    }
    return device;
}
