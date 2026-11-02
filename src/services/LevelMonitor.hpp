#pragma once

#include "infrastructure/audio/CaptureStream.hpp"
#include "infrastructure/audio/DeviceManager.hpp"
#include "core/RingBuffer.hpp"
#include "infrastructure/audio/SilentRenderKeepAlive.hpp"

#include <atomic>
#include <mutex>
#include <string>
#include <thread>

// Idle-time mic + system peak metering (no disk I/O). Stop before recording starts.
class LevelMonitor {
public:
    LevelMonitor() = default;
    ~LevelMonitor();

    LevelMonitor(const LevelMonitor&) = delete;
    auto operator=(const LevelMonitor&) -> LevelMonitor& = delete;

    void SetDeviceManager(DeviceManager* devices) { m_devices = devices; }

    // Opens capture for the given devices and starts the drain thread.
    // Empty ids resolve to the default endpoints. Failures are silent (peaks stay 0).
    void Start(
        const std::wstring& micDeviceId,
        const std::wstring& systemDeviceId,
        bool communicationsProcessing);

    void Stop();

    bool IsRunning() const { return m_running.load(std::memory_order_acquire); }

    float PeakMic() const { return m_peakMic.load(std::memory_order_acquire); }
    float PeakSys() const { return m_peakSys.load(std::memory_order_acquire); }

private:
    void DrainMain();
    void TeardownCapture();
    static float PeakOfPacket(const float* interleavedStereo, uint32_t frames, bool monoAverage);

    DeviceManager* m_devices = nullptr;

    std::mutex m_mutex;
    std::atomic<bool> m_running{false};
    std::atomic<bool> m_stop{false};

    RingBuffer m_micRing{4800};
    RingBuffer m_sysRing{4800};
    CaptureStream m_micCapture;
    CaptureStream m_sysCapture;
    SilentRenderKeepAlive m_keepAlive;

    std::thread m_drainThread;

    std::atomic<float> m_peakMic{0.0f};
    std::atomic<float> m_peakSys{0.0f};
};
