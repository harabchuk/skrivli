#pragma once

#include "audio/RingBuffer.hpp"

#include <atomic>
#include <cstdint>
#include <thread>
#include <wrl/client.h>

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

    bool Start(
        IMMDevice* device,
        Mode mode,
        RingBuffer& ring,
        bool communicationsProcessing);
    void Stop();

    bool IsRunning() const { return m_running.load(std::memory_order_acquire); }
    HRESULT FatalError() const { return m_fatalError.load(std::memory_order_acquire); }

private:
    void ThreadMain();
    void CaptureLoop();

    Mode m_mode = Mode::Microphone;
    RingBuffer* m_ring = nullptr;
    bool m_communicationsProcessing = false;
    Microsoft::WRL::ComPtr<IMMDevice> m_device;

    std::thread m_thread;
    std::atomic<bool> m_running{false};
    std::atomic<bool> m_stop{false};
    std::atomic<HRESULT> m_fatalError{S_OK};
    void* m_mmcssHandle = nullptr;
};
