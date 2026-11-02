#include "audio/SilentRenderKeepAlive.hpp"

#include "audio/AudioFormat.hpp"
#include "util/Scoped.hpp"

#include <atlbase.h>

#include <audioclient.h>
#include <avrt.h>
#include <mmdeviceapi.h>

#include <algorithm>
#include <vector>

SilentRenderKeepAlive::~SilentRenderKeepAlive() {
    Stop();
}

bool SilentRenderKeepAlive::Start(IMMDevice* renderDevice) {
    Stop();
    if (renderDevice == nullptr) {
        return false;
    }

    m_device = renderDevice;
    m_stop.store(false, std::memory_order_release);
    m_running.store(true, std::memory_order_release);

    try {
        m_thread = std::thread([this]() { ThreadMain(); });
    } catch (...) {
        m_running.store(false, std::memory_order_release);
        m_device.Reset();
        return false;
    }
    return true;
}

void SilentRenderKeepAlive::Stop() {
    m_stop.store(true, std::memory_order_release);
    if (m_thread.joinable()) {
        m_thread.join();
    }
    m_running.store(false, std::memory_order_release);
    m_device.Reset();
}

void SilentRenderKeepAlive::ThreadMain() {
    {
        ScopedComInit com(COINIT_MULTITHREADED);
        ScopedMmcss mmcss(L"Audio");
        RenderLoop();
    }
    m_running.store(false, std::memory_order_release);
}

void SilentRenderKeepAlive::RenderLoop() {
    if (!m_device) {
        return;
    }

    Microsoft::WRL::ComPtr<IAudioClient> audioClient;
    auto hr = m_device->Activate(__uuidof(IAudioClient), CLSCTX_ALL, nullptr, &audioClient);
    if (FAILED(hr) || !audioClient) {
        return;
    }

    auto format = AudioFormat::MakeFloatStereo48k();
    hr = audioClient->Initialize(
        AUDCLNT_SHAREMODE_SHARED,
        AUDCLNT_STREAMFLAGS_EVENTCALLBACK
            | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
            | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
        AudioFormat::kBufferDuration,
        0,
        reinterpret_cast<WAVEFORMATEX*>(&format),
        nullptr);
    if (FAILED(hr)) {
        return;
    }

    ATL::CHandle eventHandle(::CreateEventW(nullptr, FALSE, FALSE, nullptr));
    if (eventHandle.m_h == nullptr) {
        return;
    }

    hr = audioClient->SetEventHandle(eventHandle);
    if (FAILED(hr)) {
        return;
    }

    Microsoft::WRL::ComPtr<IAudioRenderClient> renderClient;
    hr = audioClient->GetService(__uuidof(IAudioRenderClient), &renderClient);
    if (FAILED(hr) || !renderClient) {
        return;
    }

    UINT32 bufferFrames = 0;
    audioClient->GetBufferSize(&bufferFrames);
    if (bufferFrames == 0) {
        return;
    }

    // Pre-fill with silence.
    BYTE* buffer = nullptr;
    hr = renderClient->GetBuffer(bufferFrames, &buffer);
    if (SUCCEEDED(hr) && buffer != nullptr) {
        std::fill(
            reinterpret_cast<float*>(buffer),
            reinterpret_cast<float*>(buffer) + (static_cast<size_t>(bufferFrames) * AudioFormat::kChannels),
            0.0f);
        renderClient->ReleaseBuffer(bufferFrames, 0);
    }

    hr = audioClient->Start();
    if (FAILED(hr)) {
        return;
    }

    while (!m_stop.load(std::memory_order_acquire)) {
        const auto wait = ::WaitForSingleObject(eventHandle, AudioFormat::kEventTimeoutMs);
        if (wait != WAIT_OBJECT_0 && wait != WAIT_TIMEOUT) {
            break;
        }

        UINT32 padding = 0;
        hr = audioClient->GetCurrentPadding(&padding);
        if (FAILED(hr)) {
            break;
        }

        const UINT32 framesToWrite = bufferFrames - padding;
        if (framesToWrite == 0) {
            continue;
        }

        buffer = nullptr;
        hr = renderClient->GetBuffer(framesToWrite, &buffer);
        if (FAILED(hr) || buffer == nullptr) {
            break;
        }

        std::fill(
            reinterpret_cast<float*>(buffer),
            reinterpret_cast<float*>(buffer) + (static_cast<size_t>(framesToWrite) * AudioFormat::kChannels),
            0.0f);
        renderClient->ReleaseBuffer(framesToWrite, 0);
    }

    audioClient->Stop();
}
