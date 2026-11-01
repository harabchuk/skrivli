#include "audio/AudioResampler.hpp"

#include "audio/AudioFormat.hpp"
#include "util/Log.hpp"

#include <mfapi.h>
#include <mferror.h>
#include <mfidl.h>
#include <mftransform.h>
#include <wmcodecdsp.h>

#include <algorithm>
#include <cstring>

AudioResampler::~AudioResampler() {
    Reset();
}

void AudioResampler::Reset() {
    if (m_transform) {
        m_transform->ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0);
        m_transform->ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0);
        m_transform->ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
        m_transform.Reset();
    }
    if (m_mfStarted) {
        ::MFShutdown();
        m_mfStarted = false;
    }
    m_inputBlockAlign = 0;
    m_outputBufferBytes = 0;
}

bool AudioResampler::Initialize(const void* inputFormatBytes) {
    Reset();
    const auto* inputFormat = static_cast<const WAVEFORMATEX*>(inputFormatBytes);
    if (inputFormat == nullptr || inputFormat->nChannels == 0 || inputFormat->nSamplesPerSec == 0) {
        return false;
    }

    HRESULT hr = ::MFStartup(MF_VERSION, MFSTARTUP_LITE);
    if (FAILED(hr)) {
        Log::WriteHResult(Log::Level::Error, L"MFStartup for resampler failed", hr);
        return false;
    }
    m_mfStarted = true;

    hr = ::CoCreateInstance(
        CLSID_CResamplerMediaObject,
        nullptr,
        CLSCTX_INPROC_SERVER,
        IID_PPV_ARGS(&m_transform));
    if (FAILED(hr) || !m_transform) {
        Log::WriteHResult(Log::Level::Error, L"CoCreateInstance(CLSID_CResamplerMediaObject) failed", hr);
        Reset();
        return false;
    }

    if (!SetMediaTypes(inputFormat)) {
        Reset();
        return false;
    }

    hr = m_transform->ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0);
    if (FAILED(hr)) {
        Log::WriteHResult(Log::Level::Warn, L"Resampler flush failed", hr);
    }
    hr = m_transform->ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0);
    if (FAILED(hr)) {
        Log::WriteHResult(Log::Level::Error, L"Resampler BEGIN_STREAMING failed", hr);
        Reset();
        return false;
    }
    hr = m_transform->ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0);
    if (FAILED(hr)) {
        Log::WriteHResult(Log::Level::Error, L"Resampler START_OF_STREAM failed", hr);
        Reset();
        return false;
    }

    m_inputBlockAlign = inputFormat->nBlockAlign;
    // Generous output buffer: up to ~4x rate change plus stereo float.
    m_outputBufferBytes = std::max<uint32_t>(
        4096u,
        static_cast<uint32_t>(AudioFormat::kSampleRate / 5u) * AudioFormat::kChannels * sizeof(float) * 4u);

    Log::Info(L"Audio resampler ready (mix-format → 48 kHz stereo float)");
    return true;
}

bool AudioResampler::SetMediaTypes(const void* inputFormatBytes) {
    const auto* inputFormat = static_cast<const WAVEFORMATEX*>(inputFormatBytes);
    if (inputFormat == nullptr) {
        return false;
    }
    Microsoft::WRL::ComPtr<IMFMediaType> inputType;
    HRESULT hr = ::MFCreateMediaType(&inputType);
    if (FAILED(hr) || !inputType) {
        return false;
    }
    hr = ::MFInitMediaTypeFromWaveFormatEx(
        inputType.Get(),
        inputFormat,
        sizeof(WAVEFORMATEX) + inputFormat->cbSize);
    if (FAILED(hr)) {
        Log::WriteHResult(Log::Level::Error, L"MFInitMediaTypeFromWaveFormatEx (input) failed", hr);
        return false;
    }
    hr = m_transform->SetInputType(0, inputType.Get(), 0);
    if (FAILED(hr)) {
        Log::WriteHResult(Log::Level::Error, L"Resampler SetInputType failed", hr);
        return false;
    }

    auto outWave = AudioFormat::MakeFloatStereo48k();
    Microsoft::WRL::ComPtr<IMFMediaType> outputType;
    hr = ::MFCreateMediaType(&outputType);
    if (FAILED(hr) || !outputType) {
        return false;
    }
    hr = ::MFInitMediaTypeFromWaveFormatEx(
        outputType.Get(),
        reinterpret_cast<const WAVEFORMATEX*>(&outWave),
        sizeof(WAVEFORMATEXTENSIBLE));
    if (FAILED(hr)) {
        Log::WriteHResult(Log::Level::Error, L"MFInitMediaTypeFromWaveFormatEx (output) failed", hr);
        return false;
    }
    hr = m_transform->SetOutputType(0, outputType.Get(), 0);
    if (FAILED(hr)) {
        Log::WriteHResult(Log::Level::Error, L"Resampler SetOutputType failed", hr);
        return false;
    }
    return true;
}

bool AudioResampler::Process(
    const void* inputBytes,
    uint32_t inputByteCount,
    uint32_t inputFrames,
    std::vector<float>& outInterleavedStereo,
    uint32_t& outFrames) {
    outFrames = 0;
    outInterleavedStereo.clear();
    if (!m_transform || inputBytes == nullptr || inputByteCount == 0 || inputFrames == 0) {
        return false;
    }
    if (m_inputBlockAlign == 0 || inputByteCount < inputFrames * m_inputBlockAlign) {
        return false;
    }

    Microsoft::WRL::ComPtr<IMFSample> inputSample;
    HRESULT hr = ::MFCreateSample(&inputSample);
    if (FAILED(hr) || !inputSample) {
        return false;
    }
    Microsoft::WRL::ComPtr<IMFMediaBuffer> inputBuffer;
    hr = ::MFCreateMemoryBuffer(inputByteCount, &inputBuffer);
    if (FAILED(hr) || !inputBuffer) {
        return false;
    }
    BYTE* dest = nullptr;
    hr = inputBuffer->Lock(&dest, nullptr, nullptr);
    if (FAILED(hr) || dest == nullptr) {
        return false;
    }
    std::memcpy(dest, inputBytes, inputByteCount);
    inputBuffer->Unlock();
    inputBuffer->SetCurrentLength(inputByteCount);
    if (FAILED(inputSample->AddBuffer(inputBuffer.Get()))) {
        return false;
    }

    hr = m_transform->ProcessInput(0, inputSample.Get(), 0);
    if (FAILED(hr)) {
        return false;
    }

    for (;;) {
        Microsoft::WRL::ComPtr<IMFSample> outputSample;
        Microsoft::WRL::ComPtr<IMFMediaBuffer> outputBuffer;
        hr = ::MFCreateSample(&outputSample);
        if (FAILED(hr) || !outputSample) {
            return false;
        }
        hr = ::MFCreateMemoryBuffer(m_outputBufferBytes, &outputBuffer);
        if (FAILED(hr) || !outputBuffer) {
            return false;
        }
        outputSample->AddBuffer(outputBuffer.Get());

        MFT_OUTPUT_DATA_BUFFER outData{};
        outData.pSample = outputSample.Get();
        DWORD status = 0;
        hr = m_transform->ProcessOutput(0, 1, &outData, &status);
        if (hr == MF_E_TRANSFORM_NEED_MORE_INPUT) {
            break;
        }
        if (FAILED(hr)) {
            return false;
        }

        Microsoft::WRL::ComPtr<IMFMediaBuffer> resultBuffer;
        if (FAILED(outputSample->ConvertToContiguousBuffer(&resultBuffer)) || !resultBuffer) {
            return false;
        }

        BYTE* data = nullptr;
        DWORD maxLen = 0;
        DWORD curLen = 0;
        if (FAILED(resultBuffer->Lock(&data, &maxLen, &curLen)) || data == nullptr) {
            return false;
        }

        const uint32_t frames = curLen / (AudioFormat::kChannels * sizeof(float));
        const size_t sampleCount = static_cast<size_t>(frames) * AudioFormat::kChannels;
        const size_t oldSize = outInterleavedStereo.size();
        outInterleavedStereo.resize(oldSize + sampleCount);
        std::memcpy(outInterleavedStereo.data() + oldSize, data, sampleCount * sizeof(float));
        outFrames += frames;
        resultBuffer->Unlock();
    }

    return true;
}
