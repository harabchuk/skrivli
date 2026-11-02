#pragma once

#include <windows.h>

#include <wil/com.h>

#include <atomic>
#include <thread>

struct IMMDevice;

// Shared-mode render client that continuously submits silence on the same
// device used for loopback, so the engine clock keeps advancing.
class SilentRenderKeepAlive {
public:
    SilentRenderKeepAlive() = default;
    ~SilentRenderKeepAlive();

    SilentRenderKeepAlive(const SilentRenderKeepAlive&) = delete;
    auto operator=(const SilentRenderKeepAlive&) -> SilentRenderKeepAlive& = delete;

    bool Start(IMMDevice* renderDevice);
    void Stop();
    bool IsRunning() const { return m_running.load(std::memory_order_acquire); }

private:
    void ThreadMain();
    void RenderLoop();

    wil::com_ptr_nothrow<IMMDevice> m_device;
    std::thread m_thread;
    std::atomic<bool> m_running{false};
    std::atomic<bool> m_stop{false};
};
