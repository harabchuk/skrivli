#include "audio/CaptureStream.hpp"

#include "audio/AudioFormat.hpp"

#include <audioclient.h>
#include <avrt.h>
#include <mmdeviceapi.h>

#include <vector>

namespace {

constexpr DWORD kStartFlags =
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK
    | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
    | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;

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
    m_running.store(true, std::memory_order_release);

    try {
        m_thread = std::thread([this]() { ThreadMain(); });
    } catch (...) {
        m_running.store(false, std::memory_order_release);
        m_fatalError.store(E_FAIL, std::memory_order_release);
        return false;
    }
    return true;
}

void CaptureStream::Stop() {
    m_stop.store(true, std::memory_order_release);
    if (m_thread.joinable()) {
        m_thread.join();
    }
    m_running.store(false, std::memory_order_release);
    m_device.Reset();
    m_ring = nullptr;
}

void CaptureStream::ThreadMain() {
    const HRESULT comHr = ::CoInitializeEx(nullptr, COINIT_MULTITHREADED);
    const bool comInitialized = SUCCEEDED(comHr) || comHr == S_FALSE || comHr == RPC_E_CHANGED_MODE;

    DWORD taskIndex = 0;
    m_mmcssHandle = ::AvSetMmThreadCharacteristicsW(L"Audio", &taskIndex);

    CaptureLoop();

    if (m_mmcssHandle != nullptr) {
        ::AvRevertMmThreadCharacteristics(m_mmcssHandle);
        m_mmcssHandle = nullptr;
    }

    if (comInitialized && comHr != RPC_E_CHANGED_MODE) {
        ::CoUninitialize();
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

    auto format = AudioFormat::MakeFloatStereo48k();
    DWORD flags = kStartFlags;
    if (m_mode == Mode::Loopback) {
        flags |= AUDCLNT_STREAMFLAGS_LOOPBACK;
    }

    hr = audioClient->Initialize(
        AUDCLNT_SHAREMODE_SHARED,
        flags,
        AudioFormat::kBufferDuration,
        0,
        reinterpret_cast<WAVEFORMATEX*>(&format),
        nullptr);
    if (FAILED(hr)) {
        m_fatalError.store(hr, std::memory_order_release);
        return;
    }

    HANDLE eventHandle = ::CreateEventW(nullptr, FALSE, FALSE, nullptr);
    if (eventHandle == nullptr) {
        m_fatalError.store(HRESULT_FROM_WIN32(::GetLastError()), std::memory_order_release);
        return;
    }

    hr = audioClient->SetEventHandle(eventHandle);
    if (FAILED(hr)) {
        ::CloseHandle(eventHandle);
        m_fatalError.store(hr, std::memory_order_release);
        return;
    }

    Microsoft::WRL::ComPtr<IAudioCaptureClient> captureClient;
    hr = audioClient->GetService(__uuidof(IAudioCaptureClient), &captureClient);
    if (FAILED(hr) || !captureClient) {
        ::CloseHandle(eventHandle);
        m_fatalError.store(hr, std::memory_order_release);
        return;
    }

    UINT32 bufferFrames = 0;
    audioClient->GetBufferSize(&bufferFrames);
    if (bufferFrames == 0) {
        bufferFrames = AudioFormat::kSampleRate / 5; // 200 ms fallback
    }
    std::vector<float> silence(static_cast<size_t>(bufferFrames) * AudioFormat::kChannels, 0.0f);

    hr = audioClient->Start();
    if (FAILED(hr)) {
        ::CloseHandle(eventHandle);
        m_fatalError.store(hr, std::memory_order_release);
        return;
    }

    bool sawDiscontinuity = false;

    while (!m_stop.load(std::memory_order_acquire)) {
        const DWORD wait = ::WaitForSingleObject(eventHandle, AudioFormat::kEventTimeoutMs);
        if (wait != WAIT_OBJECT_0 && wait != WAIT_TIMEOUT) {
            break;
        }

        for (;;) {
            UINT32 packetFrames = 0;
            hr = captureClient->GetNextPacketSize(&packetFrames);
            if (hr == AUDCLNT_E_DEVICE_INVALIDATED) {
                m_fatalError.store(hr, std::memory_order_release);
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
                m_fatalError.store(hr, std::memory_order_release);
                m_stop.store(true, std::memory_order_release);
                break;
            }
            if (FAILED(hr)) {
                break;
            }

            const bool discontinuity =
                ((bufferFlags & AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY) != 0) || sawDiscontinuity;
            sawDiscontinuity = false;

            if (frames > 0) {
                const float* samples = reinterpret_cast<const float*>(data);
                if ((bufferFlags & AUDCLNT_BUFFERFLAGS_SILENT) != 0 || samples == nullptr) {
                    if (frames > bufferFrames) {
                        frames = bufferFrames;
                    }
                    samples = silence.data();
                }

                if (!m_ring->Push(samples, frames, qpcPosition, discontinuity)) {
                    sawDiscontinuity = true;
                }
            }

            captureClient->ReleaseBuffer(frames);
        }
    }

    audioClient->Stop();
    ::CloseHandle(eventHandle);
}
