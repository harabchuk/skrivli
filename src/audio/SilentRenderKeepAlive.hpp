#pragma once

#include <atomic>
#include <thread>
#include <wrl/client.h>

struct IMMDevice;

// Shared-mode render client that continuously submits silence on the same
// device used for loopback, so the engine clock keeps advancing.
class SilentRenderKeepAlive {
public:
    SilentRenderKeepAlive() = default;
    ~SilentRenderKeepAlive();

    SilentRenderKeepAlive(const SilentRenderKeepAlive&) = delete;
    SilentRenderKeepAlive& operator=(const SilentRenderKeepAlive&) = delete;

    bool Start(IMMDevice* renderDevice);
    void Stop();
    bool IsRunning() const { return m_running.load(std::memory_order_acquire); }

private:
    void ThreadMain();
    void RenderLoop();

    Microsoft::WRL::ComPtr<IMMDevice> m_device;
    std::thread m_thread;
    std::atomic<bool> m_running{false};
    std::atomic<bool> m_stop{false};
};
