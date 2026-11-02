#pragma once

#include "audio/AudioResampler.hpp"
#include "audio/RingBuffer.hpp"

#include <atlbase.h>

#include <atomic>
#include <cstdint>
#include <thread>
#include <vector>
#include <wrl/client.h>

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
    CaptureStream& operator=(const CaptureStream&) = delete;

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
    bool ActivateClient(Microsoft::WRL::ComPtr<IAudioClient>& client);
    bool InitializeClient(ActiveClient& active);
    void DrainPackets(ActiveClient& active);

    Mode m_mode = Mode::Microphone;
    RingBuffer* m_ring = nullptr;
    bool m_communicationsProcessing = false;
    Microsoft::WRL::ComPtr<IMMDevice> m_device;

    std::thread m_thread;
    ATL::CHandle m_initEvent;
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
