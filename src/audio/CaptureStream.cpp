#include "audio/CaptureStream.hpp"

#include "audio/AudioFormat.hpp"
#include "util/Log.hpp"
#include "util/Scoped.hpp"

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

}  // namespace

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

    m_initEvent = ::CreateEventW(nullptr, TRUE, FALSE, nullptr);
    if (m_initEvent == nullptr) {
        m_fatalError.store(HRESULT_FROM_WIN32(::GetLastError()), std::memory_order_release);
        return false;
    }

    m_running.store(true, std::memory_order_release);
    try {
        m_thread = std::thread([this]() { ThreadMain(); });
    } catch (...) {
        m_running.store(false, std::memory_order_release);
        ::CloseHandle(m_initEvent);
        m_initEvent = nullptr;
        m_fatalError.store(E_FAIL, std::memory_order_release);
        return false;
    }

    ::WaitForSingleObject(m_initEvent, 10000);
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
    m_device.Reset();
    m_ring = nullptr;
    m_resampler.Reset();
    m_resampleScratch.clear();
    if (m_initEvent != nullptr) {
        ::CloseHandle(m_initEvent);
        m_initEvent = nullptr;
    }
}

void CaptureStream::ThreadMain() {
    {
        ScopedComInit com(COINIT_MULTITHREADED);
        {
            ScopedMmcss mmcss(L"Audio");
            CaptureLoop();
        }

        if (m_initEvent != nullptr) {
            ::SetEvent(m_initEvent);
        }
    }
    m_running.store(false, std::memory_order_release);
}

void CaptureStream::CaptureLoop() {
    if (!m_device || m_ring == nullptr) {
        m_fatalError.store(E_UNEXPECTED, std::memory_order_release);
        return;
    }

    Microsoft::WRL::ComPtr<IAudioClient> audioClient;
    HRESULT hr = m_device->Activate(__uuidof(IAudioClient), CLSCTX_ALL, nullptr, &audioClient);
    if (FAILED(hr) || !audioClient) {
        m_fatalError.store(hr, std::memory_order_release);
        return;
    }

    if (m_mode == Mode::Microphone && m_communicationsProcessing) {
        Microsoft::WRL::ComPtr<IAudioClient2> audioClient2;
        if (SUCCEEDED(audioClient.As(&audioClient2)) && audioClient2) {
            AudioClientProperties props{};
            props.cbSize = sizeof(props);
            props.bIsOffload = FALSE;
            props.eCategory = AudioCategory_Communications;
            props.Options = AUDCLNT_STREAMOPTIONS_NONE;
            audioClient2->SetClientProperties(&props);
        }
    }

    auto floatFormat = AudioFormat::MakeFloatStereo48k();
    DWORD flags = kAutoConvertFlags;
    if (m_mode == Mode::Loopback) {
        flags |= AUDCLNT_STREAMFLAGS_LOOPBACK;
    }

    bool useResampler = false;
    WAVEFORMATEX* mixFormat = nullptr;

    hr = audioClient->Initialize(
        AUDCLNT_SHAREMODE_SHARED,
        flags,
        AudioFormat::kBufferDuration,
        0,
        reinterpret_cast<WAVEFORMATEX*>(&floatFormat),
        nullptr);

    if (FAILED(hr)) {
        Log::WriteHResult(Log::Level::Warn, L"AUTOCONVERTPCM Initialize failed; trying mix-format + miniaudio resampler", hr);

        hr = audioClient->GetMixFormat(&mixFormat);
        if (FAILED(hr) || mixFormat == nullptr) {
            m_fatalError.store(hr, std::memory_order_release);
            return;
        }

        // Re-Activate a fresh client after a failed Initialize.
        audioClient.Reset();
        hr = m_device->Activate(__uuidof(IAudioClient), CLSCTX_ALL, nullptr, &audioClient);
        if (FAILED(hr) || !audioClient) {
            ::CoTaskMemFree(mixFormat);
            m_fatalError.store(hr, std::memory_order_release);
            return;
        }

        if (m_mode == Mode::Microphone && m_communicationsProcessing) {
            Microsoft::WRL::ComPtr<IAudioClient2> audioClient2;
            if (SUCCEEDED(audioClient.As(&audioClient2)) && audioClient2) {
                AudioClientProperties props{};
                props.cbSize = sizeof(props);
                props.bIsOffload = FALSE;
                props.eCategory = AudioCategory_Communications;
                props.Options = AUDCLNT_STREAMOPTIONS_NONE;
                audioClient2->SetClientProperties(&props);
            }
        }

        DWORD rawFlags = kRawEventFlags;
        if (m_mode == Mode::Loopback) {
            rawFlags |= AUDCLNT_STREAMFLAGS_LOOPBACK;
        }

        hr = audioClient->Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            rawFlags,
            AudioFormat::kBufferDuration,
            0,
            mixFormat,
            nullptr);
        if (FAILED(hr)) {
            ::CoTaskMemFree(mixFormat);
            m_fatalError.store(hr, std::memory_order_release);
            return;
        }

        if (!m_resampler.Initialize(mixFormat)) {
            ::CoTaskMemFree(mixFormat);
            m_fatalError.store(E_FAIL, std::memory_order_release);
            return;
        }
        useResampler = true;
        m_usedResampler.store(true, std::memory_order_release);
    }

    HANDLE eventHandle = ::CreateEventW(nullptr, FALSE, FALSE, nullptr);
    if (eventHandle == nullptr) {
        if (mixFormat != nullptr) {
            ::CoTaskMemFree(mixFormat);
        }
        m_fatalError.store(HRESULT_FROM_WIN32(::GetLastError()), std::memory_order_release);
        return;
    }

    hr = audioClient->SetEventHandle(eventHandle);
    if (FAILED(hr)) {
        ::CloseHandle(eventHandle);
        if (mixFormat != nullptr) {
            ::CoTaskMemFree(mixFormat);
        }
        m_fatalError.store(hr, std::memory_order_release);
        return;
    }

    Microsoft::WRL::ComPtr<IAudioCaptureClient> captureClient;
    hr = audioClient->GetService(__uuidof(IAudioCaptureClient), &captureClient);
    if (FAILED(hr) || !captureClient) {
        ::CloseHandle(eventHandle);
        if (mixFormat != nullptr) {
            ::CoTaskMemFree(mixFormat);
        }
        m_fatalError.store(hr, std::memory_order_release);
        return;
    }

    UINT32 bufferFrames = 0;
    audioClient->GetBufferSize(&bufferFrames);
    if (bufferFrames == 0) {
        bufferFrames = AudioFormat::kSampleRate / 5;
    }
    std::vector<float> silence(static_cast<size_t>(bufferFrames) * AudioFormat::kChannels, 0.0f);

    hr = audioClient->Start();
    if (FAILED(hr)) {
        ::CloseHandle(eventHandle);
        if (mixFormat != nullptr) {
            ::CoTaskMemFree(mixFormat);
        }
        m_fatalError.store(hr, std::memory_order_release);
        return;
    }

    // Init succeeded — unblock Start().
    if (m_initEvent != nullptr) {
        ::SetEvent(m_initEvent);
    }

    bool pendingDiscontinuity = false;
    const UINT32 mixBlockAlign = (mixFormat != nullptr) ? mixFormat->nBlockAlign : 0;

    while (!m_stop.load(std::memory_order_acquire)) {
        const DWORD wait = ::WaitForSingleObject(eventHandle, AudioFormat::kEventTimeoutMs);
        if (wait != WAIT_OBJECT_0 && wait != WAIT_TIMEOUT) {
            break;
        }

        for (;;) {
            UINT32 packetFrames = 0;
            hr = captureClient->GetNextPacketSize(&packetFrames);
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
            hr = captureClient->GetBuffer(&data, &frames, &bufferFlags, &devicePosition, &qpcPosition);
            if (hr == AUDCLNT_E_DEVICE_INVALIDATED) {
                m_deviceInvalidated.store(true, std::memory_order_release);
                m_stop.store(true, std::memory_order_release);
                break;
            }
            if (FAILED(hr)) {
                break;
            }

            const bool discontinuity =
                ((bufferFlags & AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY) != 0) || pendingDiscontinuity;
            pendingDiscontinuity = false;
            if ((bufferFlags & AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY) != 0) {
                m_sawDiscontinuityFlag.store(true, std::memory_order_release);
            }

            if (frames > 0) {
                if ((bufferFlags & AUDCLNT_BUFFERFLAGS_SILENT) != 0) {
                    m_sawSilent.store(true, std::memory_order_release);
                    if (frames > bufferFrames) {
                        frames = bufferFrames;
                    }
                    if (!m_ring->Push(silence.data(), frames, qpcPosition, discontinuity)) {
                        pendingDiscontinuity = true;
                    }
                } else if (useResampler) {
                    if (data == nullptr || mixBlockAlign == 0) {
                        captureClient->ReleaseBuffer(frames);
                        continue;
                    }
                    uint32_t outFrames = 0;
                    if (!m_resampler.Process(
                            data,
                            frames * mixBlockAlign,
                            frames,
                            m_resampleScratch,
                            outFrames)) {
                        pendingDiscontinuity = true;
                    } else if (outFrames > 0
                               && !m_ring->Push(
                                      m_resampleScratch.data(),
                                      outFrames,
                                      qpcPosition,
                                      discontinuity)) {
                        pendingDiscontinuity = true;
                    }
                } else {
                    const float* samples = reinterpret_cast<const float*>(data);
                    if (samples == nullptr) {
                        if (frames > bufferFrames) {
                            frames = bufferFrames;
                        }
                        samples = silence.data();
                    }
                    if (!m_ring->Push(samples, frames, qpcPosition, discontinuity)) {
                        pendingDiscontinuity = true;
                    }
                }
            }

            captureClient->ReleaseBuffer(frames);
        }
    }

    audioClient->Stop();
    ::CloseHandle(eventHandle);
    if (mixFormat != nullptr) {
        ::CoTaskMemFree(mixFormat);
    }
}
