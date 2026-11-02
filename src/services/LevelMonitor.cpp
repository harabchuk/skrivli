#include "services/LevelMonitor.hpp"

#include "core/AudioFormat.hpp"
#include "infrastructure/common/Log.hpp"
#include "infrastructure/common/Scoped.hpp"

#include <algorithm>
#include <cmath>
#include <vector>

LevelMonitor::~LevelMonitor() {
    Stop();
}

float LevelMonitor::PeakOfPacket(const float* interleavedStereo, uint32_t frames, bool monoAverage) {
    float peak = 0.0f;
    for (uint32_t i = 0; i < frames; ++i) {
        const float left = interleavedStereo[i * 2];
        const float right = interleavedStereo[i * 2 + 1];
        if (monoAverage) {
            peak = (std::max)(peak, std::fabs(0.5f * (left + right)));
        } else {
            peak = (std::max)(peak, (std::max)(std::fabs(left), std::fabs(right)));
        }
    }
    return peak;
}

void LevelMonitor::TeardownCapture() {
    m_micCapture.Stop();
    m_sysCapture.Stop();
    m_keepAlive.Stop();
}

void LevelMonitor::Stop() {
    std::lock_guard<std::mutex> lock(m_mutex);
    if (!m_running.load(std::memory_order_acquire) && !m_drainThread.joinable()) {
        return;
    }

    m_stop.store(true, std::memory_order_release);
    TeardownCapture();
    if (m_drainThread.joinable()) {
        m_drainThread.join();
    }
    m_running.store(false, std::memory_order_release);
    m_peakMic.store(0.0f, std::memory_order_release);
    m_peakSys.store(0.0f, std::memory_order_release);
}

void LevelMonitor::Start(
    const std::wstring& micDeviceId,
    const std::wstring& systemDeviceId,
    bool communicationsProcessing) {
    Stop();

    std::lock_guard<std::mutex> lock(m_mutex);
    if (m_devices == nullptr) {
        return;
    }

    m_stop.store(false, std::memory_order_release);
    m_micRing.Reset();
    m_sysRing.Reset();
    m_peakMic.store(0.0f, std::memory_order_release);
    m_peakSys.store(0.0f, std::memory_order_release);

    auto micDevice = m_devices->ResolveCaptureDevice(micDeviceId);
    auto renderDevice = m_devices->ResolveRenderDevice(systemDeviceId);
    bool any = false;

    if (renderDevice) {
        if (!m_keepAlive.Start(renderDevice.get())) {
            Log::Warn(L"LevelMonitor: system keep-alive failed");
        }
        if (m_sysCapture.Start(renderDevice.get(), CaptureStream::Mode::Loopback, m_sysRing, false)) {
            any = true;
        } else {
            m_keepAlive.Stop();
            Log::Warn(L"LevelMonitor: system capture failed to open");
        }
    }

    if (micDevice) {
        if (m_micCapture.Start(
                micDevice.get(),
                CaptureStream::Mode::Microphone,
                m_micRing,
                communicationsProcessing)) {
            any = true;
        } else {
            Log::Warn(L"LevelMonitor: microphone capture failed to open");
        }
    }

    if (!any) {
        TeardownCapture();
        return;
    }

    try {
        m_drainThread = std::thread([this]() { DrainMain(); });
    } catch (...) {
        TeardownCapture();
        Log::Error(L"LevelMonitor: failed to start drain thread");
        return;
    }

    m_running.store(true, std::memory_order_release);
}

void LevelMonitor::DrainMain() {
    ScopedComInit com(COINIT_MULTITHREADED);

    constexpr uint32_t kMaxPacketFrames = 8192;
    std::vector<float> scratch(static_cast<size_t>(kMaxPacketFrames) * AudioFormat::kChannels);

    while (!m_stop.load(std::memory_order_acquire)) {
        if (m_micCapture.IsRunning() && m_micCapture.DeviceInvalidated()) {
            m_micCapture.Stop();
            m_peakMic.store(0.0f, std::memory_order_release);
        }
        if (m_sysCapture.IsRunning() && m_sysCapture.DeviceInvalidated()) {
            m_sysCapture.Stop();
            m_keepAlive.Stop();
            m_peakSys.store(0.0f, std::memory_order_release);
        }

        float micPeak = 0.0f;
        float sysPeak = 0.0f;
        bool gotMic = false;
        bool gotSys = false;

        uint32_t frames = 0;
        uint64_t qpc = 0;
        uint32_t flags = 0;
        while (m_micRing.Pop(scratch.data(), kMaxPacketFrames, frames, qpc, flags)) {
            gotMic = true;
            micPeak = (std::max)(micPeak, PeakOfPacket(scratch.data(), frames, true));
        }
        while (m_sysRing.Pop(scratch.data(), kMaxPacketFrames, frames, qpc, flags)) {
            gotSys = true;
            sysPeak = (std::max)(sysPeak, PeakOfPacket(scratch.data(), frames, false));
        }

        // Publish measured peaks; silence (no packets this tick) clears the level so meters fall.
        if (m_micCapture.IsRunning()) {
            m_peakMic.store(gotMic ? micPeak : 0.0f, std::memory_order_release);
        }
        if (m_sysCapture.IsRunning()) {
            m_peakSys.store(gotSys ? sysPeak : 0.0f, std::memory_order_release);
        }

        ::Sleep(10);
    }
}
