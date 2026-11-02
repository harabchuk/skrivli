#pragma once

#include "infrastructure/audio/AudioResampler.hpp"
#include "core/RingBuffer.hpp"

#include <windows.h>
#include <objbase.h>

#include <wil/com.h>
#include <wil/resource.h>

#include <atomic>
#include <cstdint>
#include <thread>
#include <vector>

struct IAudioClient;
struct IMMDevice;

class CaptureStream {
public:
    enum class Mode {
        Microphone,
        Loopback,
    };

    CaptureStream() = default;
    ~CaptureStream();

    CaptureStream(const CaptureStream&) = delete;
    auto operator=(const CaptureStream&) -> CaptureStream& = delete;

    // Starts capture on a worker thread. Blocks until Initialize succeeds or fails.
    bool Start(
        IMMDevice* device,
        Mode mode,
        RingBuffer& ring,
        bool communicationsProcessing);
    void Stop();

    bool IsRunning() const { return m_running.load(std::memory_order_acquire); }
    HRESULT FatalError() const { return m_fatalError.load(std::memory_order_acquire); }
    bool DeviceInvalidated() const { return m_deviceInvalidated.load(std::memory_order_acquire); }
    bool UsedResampler() const { return m_usedResampler.load(std::memory_order_acquire); }
    bool SawSilentPacket() const { return m_sawSilent.load(std::memory_order_acquire); }
    bool SawDiscontinuity() const { return m_sawDiscontinuityFlag.load(std::memory_order_acquire); }

private:
    struct ActiveClient;

    void ThreadMain();
    void CaptureLoop();
    bool ActivateClient(wil::com_ptr_nothrow<IAudioClient>& client);
    bool InitializeClient(ActiveClient& active);
    void DrainPackets(ActiveClient& active);

    Mode m_mode = Mode::Microphone;
    RingBuffer* m_ring = nullptr;
    bool m_communicationsProcessing = false;
    wil::com_ptr_nothrow<IMMDevice> m_device;

    std::thread m_thread;
    wil::unique_event_nothrow m_initEvent;
    std::atomic<bool> m_running{false};
    std::atomic<bool> m_stop{false};
    std::atomic<HRESULT> m_fatalError{S_OK};
    std::atomic<bool> m_deviceInvalidated{false};
    std::atomic<bool> m_usedResampler{false};
    std::atomic<bool> m_sawSilent{false};
    std::atomic<bool> m_sawDiscontinuityFlag{false};

    AudioResampler m_resampler;
    std::vector<float> m_resampleScratch;
};
