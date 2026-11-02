#include "infrastructure/audio/CaptureStream.hpp"

#include "core/AudioFormat.hpp"
#include "infrastructure/common/Log.hpp"
#include "infrastructure/common/Scoped.hpp"

#include <audioclient.h>
#include <avrt.h>
#include <mmdeviceapi.h>

#include <cstring>
#include <vector>

namespace {

constexpr DWORD kAutoConvertFlags =
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK
    | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
    | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;

constexpr DWORD kRawEventFlags = AUDCLNT_STREAMFLAGS_EVENTCALLBACK;

void ApplyCommunicationsCategory(IAudioClient* client) {
    wil::com_ptr_nothrow<IAudioClient2> audioClient2;
    if (SUCCEEDED(client->QueryInterface(IID_PPV_ARGS(audioClient2.put()))) && audioClient2) {
        AudioClientProperties props{};
        props.cbSize = sizeof(props);
        props.bIsOffload = FALSE;
        props.eCategory = AudioCategory_Communications;
        props.Options = AUDCLNT_STREAMOPTIONS_NONE;
        audioClient2->SetClientProperties(&props);
    }
}

}  // namespace

struct CaptureStream::ActiveClient {
    wil::com_ptr_nothrow<IAudioClient> audio;
    wil::com_ptr_nothrow<IAudioCaptureClient> capture;
    wil::unique_cotaskmem_ptr<WAVEFORMATEX> mixFormat;
    wil::unique_event_nothrow event;
    std::vector<float> silence;
    UINT32 bufferFrames = 0;
    bool useResampler = false;
    bool pendingDiscontinuity = false;
};

CaptureStream::~CaptureStream() {
    Stop();
}

bool CaptureStream::Start(
    IMMDevice* device,
    Mode mode,
    RingBuffer& ring,
    bool communicationsProcessing) {
    Stop();
    if (device == nullptr) {
        m_fatalError.store(E_POINTER, std::memory_order_release);
        return false;
    }

    m_device = device;
    m_mode = mode;
    m_ring = &ring;
    m_communicationsProcessing = communicationsProcessing;
    m_stop.store(false, std::memory_order_release);
    m_fatalError.store(S_OK, std::memory_order_release);
    m_deviceInvalidated.store(false, std::memory_order_release);
    m_usedResampler.store(false, std::memory_order_release);
    m_sawSilent.store(false, std::memory_order_release);
    m_sawDiscontinuityFlag.store(false, std::memory_order_release);

    if (!m_initEvent.try_create(wil::EventOptions::ManualReset, nullptr)) {
        m_fatalError.store(HRESULT_FROM_WIN32(::GetLastError()), std::memory_order_release);
        return false;
    }

    m_running.store(true, std::memory_order_release);
    try {
        m_thread = std::thread([this]() { ThreadMain(); });
    } catch (...) {
        m_running.store(false, std::memory_order_release);
        m_initEvent.reset();
        m_fatalError.store(E_FAIL, std::memory_order_release);
        return false;
    }

    ::WaitForSingleObject(m_initEvent.get(), 10000);
    if (FAILED(m_fatalError.load(std::memory_order_acquire))
        || m_deviceInvalidated.load(std::memory_order_acquire)) {
        Stop();
        return false;
    }
    return m_running.load(std::memory_order_acquire);
}

void CaptureStream::Stop() {
    m_stop.store(true, std::memory_order_release);
    if (m_thread.joinable()) {
        m_thread.join();
    }
    m_running.store(false, std::memory_order_release);
    m_device.reset();
    m_ring = nullptr;
    m_resampler.Reset();
    m_resampleScratch.clear();
    m_initEvent.reset();
}

void CaptureStream::ThreadMain() {
    {
        ScopedComInit com(COINIT_MULTITHREADED);
        {
            ScopedMmcss mmcss(L"Audio");
            CaptureLoop();
        }

        if (m_initEvent) {
            m_initEvent.SetEvent();
        }
    }
    m_running.store(false, std::memory_order_release);
}

bool CaptureStream::ActivateClient(wil::com_ptr_nothrow<IAudioClient>& client) {
    const auto hr = m_device->Activate(__uuidof(IAudioClient), CLSCTX_ALL, nullptr, client.put_void());
    if (FAILED(hr) || !client) {
        m_fatalError.store(hr, std::memory_order_release);
        return false;
    }

    if (m_mode == Mode::Microphone && m_communicationsProcessing) {
        ApplyCommunicationsCategory(client.get());
    }
    return true;
}

bool CaptureStream::InitializeClient(ActiveClient& active) {
    if (!ActivateClient(active.audio)) {
        return false;
    }

    auto floatFormat = AudioFormat::MakeFloatStereo48k();
    DWORD flags = kAutoConvertFlags;
    if (m_mode == Mode::Loopback) {
        flags |= AUDCLNT_STREAMFLAGS_LOOPBACK;
    }

    auto hr = active.audio->Initialize(
        AUDCLNT_SHAREMODE_SHARED,
        flags,
        AudioFormat::kBufferDuration,
        0,
        reinterpret_cast<WAVEFORMATEX*>(&floatFormat),
        nullptr);

    if (FAILED(hr)) {
        Log::WriteHResult(Log::Level::Warn, L"AUTOCONVERTPCM Initialize failed; trying mix-format + miniaudio resampler", hr);

        WAVEFORMATEX* mixFormat = nullptr;
        hr = active.audio->GetMixFormat(&mixFormat);
        active.mixFormat.reset(mixFormat);
        if (FAILED(hr) || !active.mixFormat) {
            m_fatalError.store(hr, std::memory_order_release);
            return false;
        }

        // Re-Activate a fresh client after a failed Initialize.
        active.audio.reset();
        if (!ActivateClient(active.audio)) {
            return false;
        }

        DWORD rawFlags = kRawEventFlags;
        if (m_mode == Mode::Loopback) {
            rawFlags |= AUDCLNT_STREAMFLAGS_LOOPBACK;
        }

        hr = active.audio->Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            rawFlags,
            AudioFormat::kBufferDuration,
            0,
            active.mixFormat.get(),
            nullptr);
        if (FAILED(hr)) {
            m_fatalError.store(hr, std::memory_order_release);
            return false;
        }

        if (!m_resampler.Initialize(active.mixFormat.get())) {
            m_fatalError.store(E_FAIL, std::memory_order_release);
            return false;
        }
        active.useResampler = true;
        m_usedResampler.store(true, std::memory_order_release);
    }

    if (!active.event.try_create(wil::EventOptions::None, nullptr)) {
        m_fatalError.store(HRESULT_FROM_WIN32(::GetLastError()), std::memory_order_release);
        return false;
    }

    hr = active.audio->SetEventHandle(active.event.get());
    if (FAILED(hr)) {
        m_fatalError.store(hr, std::memory_order_release);
        return false;
    }

    hr = active.audio->GetService(__uuidof(IAudioCaptureClient), active.capture.put_void());
    if (FAILED(hr) || !active.capture) {
        m_fatalError.store(hr, std::memory_order_release);
        return false;
    }

    active.audio->GetBufferSize(&active.bufferFrames);
    if (active.bufferFrames == 0) {
        active.bufferFrames = AudioFormat::kSampleRate / 5;
    }
    active.silence.assign(static_cast<size_t>(active.bufferFrames) * AudioFormat::kChannels, 0.0f);
    return true;
}

void CaptureStream::DrainPackets(ActiveClient& active) {
    const UINT32 mixBlockAlign = (active.mixFormat != nullptr) ? active.mixFormat->nBlockAlign : 0;

    for (;;) {
        UINT32 packetFrames = 0;
        auto hr = active.capture->GetNextPacketSize(&packetFrames);
        if (hr == AUDCLNT_E_DEVICE_INVALIDATED) {
            m_deviceInvalidated.store(true, std::memory_order_release);
            m_stop.store(true, std::memory_order_release);
            break;
        }
        if (FAILED(hr) || packetFrames == 0) {
            break;
        }

        BYTE* data = nullptr;
        UINT32 frames = 0;
        DWORD bufferFlags = 0;
        UINT64 devicePosition = 0;
        UINT64 qpcPosition = 0;
        hr = active.capture->GetBuffer(&data, &frames, &bufferFlags, &devicePosition, &qpcPosition);
        if (hr == AUDCLNT_E_DEVICE_INVALIDATED) {
            m_deviceInvalidated.store(true, std::memory_order_release);
            m_stop.store(true, std::memory_order_release);
            break;
        }
        if (FAILED(hr)) {
            break;
        }

        const bool discontinuity =
            ((bufferFlags & AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY) != 0) || active.pendingDiscontinuity;
        active.pendingDiscontinuity = false;
        if ((bufferFlags & AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY) != 0) {
            m_sawDiscontinuityFlag.store(true, std::memory_order_release);
        }

        if (frames > 0) {
            if ((bufferFlags & AUDCLNT_BUFFERFLAGS_SILENT) != 0) {
                m_sawSilent.store(true, std::memory_order_release);
                if (!m_ring->Push(active.silence.data(), frames, qpcPosition, discontinuity)) {
                    active.pendingDiscontinuity = true;
                }
            } else if (active.useResampler) {
                if (data == nullptr || mixBlockAlign == 0) {
                    active.capture->ReleaseBuffer(frames);
                    continue;
                }
                uint32_t outFrames = 0;
                if (!m_resampler.Process(
                        data,
                        frames * mixBlockAlign,
                        frames,
                        m_resampleScratch,
                        outFrames)) {
                    active.pendingDiscontinuity = true;
                } else if (outFrames > 0
                           && !m_ring->Push(
                                  m_resampleScratch.data(),
                                  outFrames,
                                  qpcPosition,
                                  discontinuity)) {
                    active.pendingDiscontinuity = true;
                }
            } else {
                const auto* samples = reinterpret_cast<const float*>(data);
                if (samples == nullptr) {
                    if (frames > active.bufferFrames) {
                        frames = active.bufferFrames;
                    }
                    samples = active.silence.data();
                }
                if (!m_ring->Push(samples, frames, qpcPosition, discontinuity)) {
                    active.pendingDiscontinuity = true;
                }
            }
        }

        active.capture->ReleaseBuffer(frames);
    }
}

void CaptureStream::CaptureLoop() {
    if (!m_device || m_ring == nullptr) {
        m_fatalError.store(E_UNEXPECTED, std::memory_order_release);
        return;
    }

    ActiveClient active;
    if (!InitializeClient(active)) {
        return;
    }

    const auto hr = active.audio->Start();
    if (FAILED(hr)) {
        m_fatalError.store(hr, std::memory_order_release);
        return;
    }

    // Init succeeded — unblock Start().
    if (m_initEvent) {
        m_initEvent.SetEvent();
    }

    while (!m_stop.load(std::memory_order_acquire)) {
        const auto wait = ::WaitForSingleObject(active.event.get(), AudioFormat::kEventTimeoutMs);
        if (wait != WAIT_OBJECT_0 && wait != WAIT_TIMEOUT) {
            break;
        }
        DrainPackets(active);
    }

    active.audio->Stop();
}
