#include "audio/RecordingSession.hpp"

#include "Ids.hpp"
#include "util/Log.hpp"

#include <windows.h>

RecordingSession::~RecordingSession() {
    Stop();
}

void RecordingSession::SetState(State next) {
    m_state.store(next, std::memory_order_release);
}

void RecordingSession::PostStarted() {
    if (m_notifyHwnd != nullptr && ::IsWindow(m_notifyHwnd)) {
        ::PostMessageW(m_notifyHwnd, WM_APP_RECORDING_STARTED, 0, 0);
    }
}

void RecordingSession::PostFinished(const std::wstring& path) {
    if (m_notifyHwnd == nullptr || !::IsWindow(m_notifyHwnd)) {
        return;
    }
    auto* heapPath = new std::wstring(path);
    if (!::PostMessageW(m_notifyHwnd, WM_APP_RECORDING_FINISHED, 0, reinterpret_cast<LPARAM>(heapPath))) {
        delete heapPath;
    }
}

void RecordingSession::PostError(const std::wstring& message) {
    if (m_notifyHwnd == nullptr || !::IsWindow(m_notifyHwnd)) {
        return;
    }
    auto* heapMessage = new std::wstring(message);
    if (!::PostMessageW(m_notifyHwnd, WM_APP_RECORDING_ERROR, 0, reinterpret_cast<LPARAM>(heapMessage))) {
        delete heapMessage;
    }
}

void RecordingSession::FailStart(const std::wstring& message) {
    Log::Error(message);
    TeardownCapture();
    const std::wstring partial = m_writer.PartialPath();
    m_writer.Abandon();
    if (!partial.empty()) {
        ::DeleteFileW(partial.c_str());
    }
    SetState(State::Error);
    PostError(message);
    SetState(State::Idle);
}

void RecordingSession::TeardownCapture() {
    m_micCapture.Stop();
    m_sysCapture.Stop();
    m_keepAlive.Stop();
}

bool RecordingSession::Start(
    const std::wstring& folder,
    const std::wstring& micDeviceId,
    const std::wstring& systemDeviceId,
    float micGain,
    float systemGain,
    bool communicationsProcessing) {
    std::lock_guard<std::mutex> lock(m_startStopMutex);

    const State state = GetState();
    if (state != State::Idle && state != State::Error) {
        Log::Warn(L"RecordingSession::Start ignored — invalid state");
        return false;
    }
    if (m_devices == nullptr) {
        FailStart(L"Audio device manager is not ready.");
        return false;
    }

    SetState(State::Starting);
    m_stopWriter.store(false, std::memory_order_release);
    m_micRing.Reset();
    m_sysRing.Reset();

    if (!m_writer.Open(folder)) {
        FailStart(L"Could not create a recording file in the chosen folder.");
        return false;
    }

    auto micDevice = m_devices->ResolveCaptureDevice(micDeviceId);
    auto renderDevice = m_devices->ResolveRenderDevice(systemDeviceId);
    if (!micDevice) {
        FailStart(L"No microphone is available.");
        return false;
    }
    if (!renderDevice) {
        FailStart(L"No system audio output device is available.");
        return false;
    }

    if (!m_keepAlive.Start(renderDevice.Get())) {
        FailStart(L"Failed to start system-audio keep-alive.");
        return false;
    }

    if (!m_micCapture.Start(
            micDevice.Get(),
            CaptureStream::Mode::Microphone,
            m_micRing,
            communicationsProcessing)) {
        FailStart(L"Failed to open the microphone.");
        return false;
    }

    if (!m_sysCapture.Start(renderDevice.Get(), CaptureStream::Mode::Loopback, m_sysRing, false)) {
        FailStart(L"Failed to open system audio (loopback).");
        return false;
    }

    ::Sleep(50);

    if (FAILED(m_micCapture.FatalError())) {
        FailStart(L"Microphone stream failed to start. The device may reject 48 kHz float conversion.");
        return false;
    }
    if (FAILED(m_sysCapture.FatalError())) {
        FailStart(L"System audio stream failed to start. The device may reject 48 kHz float conversion.");
        return false;
    }

    LARGE_INTEGER qpc{};
    LARGE_INTEGER freq{};
    ::QueryPerformanceCounter(&qpc);
    ::QueryPerformanceFrequency(&freq);
    m_startQpc.store(static_cast<uint64_t>(qpc.QuadPart), std::memory_order_release);
    m_mixer.Reset(
        static_cast<uint64_t>(qpc.QuadPart),
        static_cast<uint64_t>(freq.QuadPart),
        micGain,
        systemGain);

    try {
        m_writerThread = std::thread([this]() { WriterMain(); });
    } catch (...) {
        FailStart(L"Failed to start the recording writer thread.");
        return false;
    }

    SetState(State::Recording);
    PostStarted();
    return true;
}

void RecordingSession::Stop() {
    std::lock_guard<std::mutex> lock(m_startStopMutex);

    const State state = GetState();
    if (state != State::Recording && state != State::Starting) {
        if (m_writerThread.joinable()) {
            m_stopWriter.store(true, std::memory_order_release);
            m_writerThread.join();
        }
        return;
    }

    SetState(State::Stopping);
    m_stopWriter.store(true, std::memory_order_release);
    TeardownCapture();

    if (m_writerThread.joinable()) {
        m_writerThread.join();
    }

    std::wstring finalPath;
    if (m_writer.IsOpen()) {
        if (m_writer.Finalize(finalPath)) {
            SetState(State::Idle);
            PostFinished(finalPath);
            return;
        }
        m_writer.Abandon();
        SetState(State::Error);
        PostError(L"Failed to finalize the recording file.");
        SetState(State::Idle);
        return;
    }

    SetState(State::Idle);
}

void RecordingSession::WriterMain() {
    const HRESULT comHr = ::CoInitializeEx(nullptr, COINIT_MULTITHREADED);
    const bool comInitialized = SUCCEEDED(comHr) || comHr == S_FALSE || comHr == RPC_E_CHANGED_MODE;

    float block[AudioFormat::kBlockFrames * AudioFormat::kChannels]{};
    bool postedDeviceError = false;

    while (!m_stopWriter.load(std::memory_order_acquire)) {
        if (!postedDeviceError
            && (FAILED(m_micCapture.FatalError()) || FAILED(m_sysCapture.FatalError()))) {
            postedDeviceError = true;
            PostError(L"An audio device was lost. Stopping the recording.");
            m_stopWriter.store(true, std::memory_order_release);
            break;
        }

        LARGE_INTEGER now{};
        ::QueryPerformanceCounter(&now);
        if (m_mixer.Process(m_micRing, m_sysRing, static_cast<uint64_t>(now.QuadPart), block)) {
            if (!m_writer.WriteFloatFrames(block, AudioFormat::kBlockFrames)) {
                PostError(L"Failed while writing the recording.");
                m_stopWriter.store(true, std::memory_order_release);
                break;
            }
        } else {
            ::Sleep(2);
        }
    }

    while (m_mixer.Flush(m_micRing, m_sysRing, block)) {
        if (!m_writer.WriteFloatFrames(block, AudioFormat::kBlockFrames)) {
            break;
        }
    }

    if (comInitialized && comHr != RPC_E_CHANGED_MODE) {
        ::CoUninitialize();
    }
}
