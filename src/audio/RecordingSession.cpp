#include "audio/RecordingSession.hpp"

#include "Ids.hpp"
#include "util/Log.hpp"
#include "util/Paths.hpp"

#include <audioclient.h>
#include <windows.h>

namespace {

constexpr DWORD kDiskCheckIntervalMs = 30000;

}  // namespace

RecordingSession::~RecordingSession() {
    Stop(StopMode::Save);
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

void RecordingSession::PostDeviceSwitched(const std::wstring& message) {
    if (m_notifyHwnd == nullptr || !::IsWindow(m_notifyHwnd)) {
        return;
    }
    auto* heapMessage = new std::wstring(message);
    if (!::PostMessageW(m_notifyHwnd, WM_APP_DEVICE_SWITCHED, 0, reinterpret_cast<LPARAM>(heapMessage))) {
        delete heapMessage;
    }
}

void RecordingSession::PostLowDisk() {
    if (m_notifyHwnd != nullptr && ::IsWindow(m_notifyHwnd)) {
        ::PostMessageW(m_notifyHwnd, WM_APP_LOW_DISK, 0, 0);
    }
}

void RecordingSession::PostRescued(const std::wstring& path) {
    if (m_notifyHwnd == nullptr || !::IsWindow(m_notifyHwnd)) {
        return;
    }
    auto* heapPath = new std::wstring(path);
    if (!::PostMessageW(m_notifyHwnd, WM_APP_RECORDING_RESCUED, 0, reinterpret_cast<LPARAM>(heapPath))) {
        delete heapPath;
    }
}

bool RecordingSession::IsPrivacyDenied(HRESULT hr) {
    return hr == E_ACCESSDENIED;
}

bool RecordingSession::IsDeviceBusy(HRESULT hr) {
    return hr == AUDCLNT_E_DEVICE_IN_USE
        || hr == AUDCLNT_E_EXCLUSIVE_MODE_ONLY
        || hr == AUDCLNT_E_ENDPOINT_CREATE_FAILED;
}

std::wstring RecordingSession::DescribeCaptureError(HRESULT hr, bool isMic) {
    const wchar_t* which = isMic ? L"microphone" : L"system audio";
    if (IsPrivacyDenied(hr)) {
        return L"Microphone access is denied. Enable microphone privacy in Windows Settings.";
    }
    if (IsDeviceBusy(hr)) {
        return std::wstring(L"The ") + which + L" is in exclusive use by another application.";
    }
    wchar_t buf[128]{};
    std::swprintf(buf, 128, L"Failed to open the %s (HRESULT 0x%08X).", which, static_cast<unsigned>(hr));
    return buf;
}

void RecordingSession::AddDeviceNote(const std::wstring& note) {
    std::lock_guard<std::mutex> lock(m_notesMutex);
    m_deviceNotes.push_back(note);
    Log::Info(L"Device note: " + note);
}

std::vector<std::wstring> RecordingSession::TakeDeviceNotes() {
    std::lock_guard<std::mutex> lock(m_notesMutex);
    std::vector<std::wstring> notes;
    notes.swap(m_deviceNotes);
    return notes;
}

std::wstring RecordingSession::MicDeviceName() const {
    return m_micName;
}

std::wstring RecordingSession::SystemDeviceName() const {
    return m_systemName;
}

double RecordingSession::ElapsedSeconds() const {
    const uint64_t start = m_startQpc.load(std::memory_order_acquire);
    if (start == 0) {
        return 0.0;
    }
    LARGE_INTEGER now{};
    LARGE_INTEGER freq{};
    ::QueryPerformanceCounter(&now);
    ::QueryPerformanceFrequency(&freq);
    if (freq.QuadPart == 0) {
        return 0.0;
    }
    return static_cast<double>(static_cast<uint64_t>(now.QuadPart) - start)
        / static_cast<double>(freq.QuadPart);
}

void RecordingSession::FailStart(RecordingStartResult& result, const std::wstring& message) {
    Log::Error(message);
    TeardownCapture();
    m_micStemWriter.Discard();
    m_sysStemWriter.Discard();
    m_stemWritersOpen = false;
    m_writer.Discard();
    SetState(State::Error);
    result.started = false;
    result.error = message;
    SetState(State::Idle);
}

void RecordingSession::TeardownCapture() {
    m_micCapture.Stop();
    m_sysCapture.Stop();
    m_keepAlive.Stop();
}

void RecordingSession::OnDevicesChanged() {
    if (IsActive()) {
        m_devicesChanged.store(true, std::memory_order_release);
    }
}

RecordingStartResult RecordingSession::Start(
    const std::wstring& folder,
    const std::wstring& micDeviceId,
    const std::wstring& systemDeviceId,
    float micGain,
    float systemGain,
    bool communicationsProcessing,
    bool saveStems) {
    std::lock_guard<std::mutex> lock(m_startStopMutex);
    RecordingStartResult result{};

    const State state = GetState();
    if (state != State::Idle && state != State::Error) {
        Log::Warn(L"RecordingSession::Start ignored — invalid state");
        result.error = L"A recording is already in progress.";
        return result;
    }
    if (m_devices == nullptr) {
        FailStart(result, L"Audio device manager is not ready.");
        return result;
    }

    SetState(State::Starting);
    m_stopWriter.store(false, std::memory_order_release);
    m_devicesChanged.store(false, std::memory_order_release);
    m_lowDiskStop.store(false, std::memory_order_release);
    m_writeFailed.store(false, std::memory_order_release);
    m_micRing.Reset();
    m_sysRing.Reset();
    {
        std::lock_guard<std::mutex> notesLock(m_notesMutex);
        m_deviceNotes.clear();
    }

    m_micDeviceId = micDeviceId;
    m_systemDeviceId = systemDeviceId;
    m_micUsesDefault = micDeviceId.empty();
    m_systemUsesDefault = systemDeviceId.empty();
    m_communicationsProcessing = communicationsProcessing;
    m_saveStems = saveStems;
    m_stemWritersOpen = false;
    m_micGain = micGain;
    m_systemGain = systemGain;
    m_micEnabled = false;
    m_systemEnabled = false;
    m_micName.clear();
    m_systemName.clear();

    if (!WavWriter::IsWritableDirectory(folder)) {
        FailStart(result, L"The recordings folder is missing or not writable.");
        return result;
    }
    if (!WavWriter::FolderHasFreeSpace(folder)) {
        FailStart(result, L"Not enough free disk space to start recording (need at least 200 MB).");
        return result;
    }

    if (!m_writer.Open(folder)) {
        FailStart(result, L"Could not create a recording file in the chosen folder.");
        return result;
    }

    if (m_saveStems) {
        const std::wstring mixFinal = m_writer.FinalPath();
        std::wstring stemBase = mixFinal;
        const std::wstring wavExt = L".wav";
        if (stemBase.size() >= wavExt.size()
            && _wcsicmp(stemBase.c_str() + stemBase.size() - wavExt.size(), wavExt.c_str()) == 0) {
            stemBase.erase(stemBase.size() - wavExt.size());
        }
        if (!m_micStemWriter.OpenFinalPath(stemBase + L".mic.wav")
            || !m_sysStemWriter.OpenFinalPath(stemBase + L".system.wav")) {
            FailStart(result, L"Could not create stem recording files.");
            return result;
        }
        m_stemWritersOpen = true;
    }

    auto micDevice = m_devices->ResolveCaptureDevice(micDeviceId);
    auto renderDevice = m_devices->ResolveRenderDevice(systemDeviceId);
    const bool micPresent = static_cast<bool>(micDevice);
    const bool sysPresent = static_cast<bool>(renderDevice);

    if (!micPresent && !sysPresent) {
        FailStart(result, L"No microphone or system audio device is available.");
        return result;
    }

    if (!micPresent) {
        result.micMissing = true;
        result.warning = L"No microphone — recording system audio only.";
        AddDeviceNote(L"Started without microphone");
    }
    if (!sysPresent) {
        result.systemMissing = true;
        result.warning = result.warning.empty()
            ? L"No system audio device — recording microphone only."
            : result.warning;
        AddDeviceNote(L"Started without system audio");
    }

    if (sysPresent) {
        if (!m_keepAlive.Start(renderDevice.Get())) {
            Log::Warn(L"Failed to start system-audio keep-alive; continuing without it");
            AddDeviceNote(L"System keep-alive failed to start");
        }

        if (!m_sysCapture.Start(renderDevice.Get(), CaptureStream::Mode::Loopback, m_sysRing, false)) {
            const HRESULT hr = m_sysCapture.FatalError();
            Log::Warn(DescribeCaptureError(hr, false));
            m_keepAlive.Stop();
            if (IsDeviceBusy(hr) && micPresent) {
                result.warning = L"System audio unavailable (in use). Recording microphone only.";
                AddDeviceNote(L"System audio exclusive/busy at start — mic only");
            } else if (!micPresent) {
                FailStart(result, DescribeCaptureError(hr, false));
                return result;
            } else {
                result.warning = L"System audio failed to open. Recording microphone only.";
                AddDeviceNote(L"System audio failed to open at start — mic only");
            }
        } else {
            m_systemEnabled = true;
            m_systemName = m_devices->FriendlyName(renderDevice.Get());
            if (m_sysCapture.UsedResampler()) {
                AddDeviceNote(L"System audio used Media Foundation resampler fallback");
            }
        }
    }

    if (micPresent) {
        if (!m_micCapture.Start(
                micDevice.Get(),
                CaptureStream::Mode::Microphone,
                m_micRing,
                communicationsProcessing)) {
            const HRESULT hr = m_micCapture.FatalError();
            if (IsPrivacyDenied(hr)) {
                TeardownCapture();
                m_micStemWriter.Discard();
                m_sysStemWriter.Discard();
                m_stemWritersOpen = false;
                m_writer.Discard();
                SetState(State::Idle);
                result.privacyDenied = true;
                result.error = DescribeCaptureError(hr, true);
                return result;
            }
            if (IsDeviceBusy(hr) && m_systemEnabled) {
                result.warning = L"Microphone unavailable (in use). Recording system audio only.";
                AddDeviceNote(L"Microphone exclusive/busy at start — system only");
            } else if (!m_systemEnabled) {
                FailStart(result, DescribeCaptureError(hr, true));
                return result;
            } else {
                result.warning = L"Microphone failed to open. Recording system audio only.";
                AddDeviceNote(L"Microphone failed to open at start — system only");
            }
        } else {
            m_micEnabled = true;
            m_micName = m_devices->FriendlyName(micDevice.Get());
            if (m_micCapture.UsedResampler()) {
                AddDeviceNote(L"Microphone used Media Foundation resampler fallback");
            }
        }
    }

    if (!m_micEnabled && !m_systemEnabled) {
        FailStart(result, L"Could not open any audio device.");
        return result;
    }

    ::GetLocalTime(&m_startLocal);

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
        FailStart(result, L"Failed to start the recording writer thread.");
        return result;
    }

    SetState(State::Recording);
    PostStarted();
    result.started = true;
    return result;
}

void RecordingSession::Stop(StopMode mode) {
    std::lock_guard<std::mutex> lock(m_startStopMutex);

    const State state = GetState();
    if (state != State::Recording && state != State::Starting) {
        if (m_writerThread.joinable()) {
            m_stopWriter.store(true, std::memory_order_release);
            m_writerThread.join();
        }
        if (mode == StopMode::Discard) {
            m_micStemWriter.Discard();
            m_sysStemWriter.Discard();
            m_stemWritersOpen = false;
            m_writer.Discard();
        }
        return;
    }

    SetState(State::Stopping);
    m_stopWriter.store(true, std::memory_order_release);
    TeardownCapture();

    if (m_writerThread.joinable()) {
        m_writerThread.join();
    }

    if (mode == StopMode::Discard) {
        m_micStemWriter.Discard();
        m_sysStemWriter.Discard();
        m_stemWritersOpen = false;
        m_writer.Discard();
        SetState(State::Idle);
        return;
    }

    if (m_stemWritersOpen) {
        std::wstring micPath;
        std::wstring sysPath;
        if (!m_micStemWriter.Finalize(micPath)) {
            m_micStemWriter.Abandon();
            Log::Warn(L"Failed to finalize mic stem");
        }
        if (!m_sysStemWriter.Finalize(sysPath)) {
            m_sysStemWriter.Abandon();
            Log::Warn(L"Failed to finalize system stem");
        }
        m_stemWritersOpen = false;
    }

    // Prefer rescue if the writer already moved the file.
    if (!m_writer.IsOpen()) {
        if (m_stemWritersOpen) {
            m_micStemWriter.Discard();
            m_sysStemWriter.Discard();
            m_stemWritersOpen = false;
        }
        SetState(State::Idle);
        return;
    }

    std::wstring finalPath;
    if (m_writer.Finalize(finalPath)) {
        SetState(State::Idle);
        PostFinished(finalPath);
        return;
    }

    // Finalize failed (folder gone?): try rescue.
    std::wstring rescued;
    if (m_writer.IsOpen() && m_writer.RescueTo(Paths::RescueDir(), rescued)) {
        SetState(State::Idle);
        PostRescued(rescued);
        return;
    }

    m_writer.Abandon();
    SetState(State::Error);
    PostError(L"Failed to finalize the recording file.");
    SetState(State::Idle);
}

bool RecordingSession::TryReopenMic(const wchar_t* reason) {
    std::lock_guard<std::mutex> lock(m_reopenMutex);
    if (m_devices == nullptr || m_stopWriter.load(std::memory_order_acquire)) {
        return false;
    }

    m_micCapture.Stop();
    auto device = m_devices->ResolveCaptureDevice(m_micUsesDefault ? std::wstring{} : m_micDeviceId);
    if (!device) {
        m_micEnabled = false;
        AddDeviceNote(std::wstring(L"Microphone lost (") + reason + L") — no replacement");
        return false;
    }

    if (!m_micCapture.Start(
            device.Get(),
            CaptureStream::Mode::Microphone,
            m_micRing,
            m_communicationsProcessing)) {
        m_micEnabled = false;
        AddDeviceNote(std::wstring(L"Microphone reopen failed (") + reason + L")");
        return false;
    }

    m_micEnabled = true;
    m_micName = m_devices->FriendlyName(device.Get());
    const std::wstring note =
        std::wstring(L"Microphone switched (") + reason + L") → " + m_micName;
    AddDeviceNote(note);
    PostDeviceSwitched(L"Device lost — recording continues on " + m_micName);
    return true;
}

bool RecordingSession::TryReopenSystem(const wchar_t* reason) {
    std::lock_guard<std::mutex> lock(m_reopenMutex);
    if (m_devices == nullptr || m_stopWriter.load(std::memory_order_acquire)) {
        return false;
    }

    m_sysCapture.Stop();
    m_keepAlive.Stop();

    auto device = m_devices->ResolveRenderDevice(m_systemUsesDefault ? std::wstring{} : m_systemDeviceId);
    if (!device) {
        m_systemEnabled = false;
        AddDeviceNote(std::wstring(L"System audio lost (") + reason + L") — no replacement");
        return false;
    }

    if (!m_keepAlive.Start(device.Get())) {
        AddDeviceNote(L"Keep-alive failed after system device switch");
    }

    if (!m_sysCapture.Start(device.Get(), CaptureStream::Mode::Loopback, m_sysRing, false)) {
        m_systemEnabled = false;
        m_keepAlive.Stop();
        AddDeviceNote(std::wstring(L"System audio reopen failed (") + reason + L")");
        return false;
    }

    m_systemEnabled = true;
    m_systemName = m_devices->FriendlyName(device.Get());
    const std::wstring note =
        std::wstring(L"System audio switched (") + reason + L") → " + m_systemName;
    AddDeviceNote(note);
    PostDeviceSwitched(L"Device lost — recording continues on " + m_systemName);
    return true;
}

void RecordingSession::WriterMain() {
    const HRESULT comHr = ::CoInitializeEx(nullptr, COINIT_MULTITHREADED);
    const bool comInitialized = SUCCEEDED(comHr) || comHr == S_FALSE || comHr == RPC_E_CHANGED_MODE;

    float block[AudioFormat::kBlockFrames * AudioFormat::kChannels]{};
    float micStem[AudioFormat::kBlockFrames * AudioFormat::kChannels]{};
    float sysStem[AudioFormat::kBlockFrames * AudioFormat::kChannels]{};
    ULONGLONG lastDiskCheck = ::GetTickCount64();
    bool rescued = false;
    bool notedMicSilent = false;
    bool notedMicDisc = false;
    bool notedSysSilent = false;
    bool notedSysDisc = false;

    while (!m_stopWriter.load(std::memory_order_acquire)) {
        if (m_micEnabled && m_micCapture.DeviceInvalidated()) {
            TryReopenMic(L"invalidated");
        }
        if (m_systemEnabled && m_sysCapture.DeviceInvalidated()) {
            TryReopenSystem(L"invalidated");
        }

        if (m_devicesChanged.exchange(false, std::memory_order_acq_rel)) {
            if (m_micUsesDefault) {
                TryReopenMic(L"default changed");
            }
            if (m_systemUsesDefault) {
                TryReopenSystem(L"default changed");
            }
        }

        // Session notes for buffer flags (once each; not on the capture hot path).
        if (!notedMicSilent && m_micCapture.SawSilentPacket()) {
            notedMicSilent = true;
            AddDeviceNote(L"Microphone delivered silent packets (AUDCLNT_BUFFERFLAGS_SILENT)");
        }
        if (!notedMicDisc && m_micCapture.SawDiscontinuity()) {
            notedMicDisc = true;
            AddDeviceNote(L"Microphone reported DATA_DISCONTINUITY");
        }
        if (!notedSysSilent && m_sysCapture.SawSilentPacket()) {
            notedSysSilent = true;
            AddDeviceNote(L"System audio delivered silent packets (AUDCLNT_BUFFERFLAGS_SILENT)");
        }
        if (!notedSysDisc && m_sysCapture.SawDiscontinuity()) {
            notedSysDisc = true;
            AddDeviceNote(L"System audio reported DATA_DISCONTINUITY");
        }

        const ULONGLONG nowTick = ::GetTickCount64();
        if (nowTick - lastDiskCheck >= kDiskCheckIntervalMs) {
            lastDiskCheck = nowTick;
            const std::wstring folder = m_writer.Folder();
            if (!folder.empty() && !WavWriter::FolderHasFreeSpace(folder)) {
                m_lowDiskStop.store(true, std::memory_order_release);
                PostLowDisk();
                m_stopWriter.store(true, std::memory_order_release);
                break;
            }
        }

        LARGE_INTEGER now{};
        ::QueryPerformanceCounter(&now);
        float* micOut = m_stemWritersOpen ? micStem : nullptr;
        float* sysOut = m_stemWritersOpen ? sysStem : nullptr;
        if (m_mixer.Process(
                m_micRing,
                m_sysRing,
                static_cast<uint64_t>(now.QuadPart),
                block,
                micOut,
                sysOut)) {
            if (!m_writer.WriteFloatFrames(block, AudioFormat::kBlockFrames)) {
                m_writeFailed.store(true, std::memory_order_release);
                std::wstring rescuedPath;
                if (m_writer.RescueTo(Paths::RescueDir(), rescuedPath)) {
                    rescued = true;
                    PostRescued(rescuedPath);
                } else {
                    PostError(L"Failed while writing the recording.");
                }
                m_stopWriter.store(true, std::memory_order_release);
                break;
            }
            if (m_stemWritersOpen) {
                if (!m_micStemWriter.WriteFloatFrames(micStem, AudioFormat::kBlockFrames)
                    || !m_sysStemWriter.WriteFloatFrames(sysStem, AudioFormat::kBlockFrames)) {
                    Log::Warn(L"Stem write failed; continuing with mix only");
                    m_micStemWriter.Discard();
                    m_sysStemWriter.Discard();
                    m_stemWritersOpen = false;
                }
            }
        } else {
            ::Sleep(2);
        }
    }

    while (m_mixer.Flush(
        m_micRing,
        m_sysRing,
        block,
        m_stemWritersOpen ? micStem : nullptr,
        m_stemWritersOpen ? sysStem : nullptr)) {
        if (!m_writer.IsOpen()) {
            break;
        }
        if (!m_writer.WriteFloatFrames(block, AudioFormat::kBlockFrames)) {
            if (!rescued) {
                std::wstring rescuedPath;
                if (m_writer.RescueTo(Paths::RescueDir(), rescuedPath)) {
                    rescued = true;
                    PostRescued(rescuedPath);
                }
            }
            break;
        }
        if (m_stemWritersOpen) {
            if (!m_micStemWriter.WriteFloatFrames(micStem, AudioFormat::kBlockFrames)
                || !m_sysStemWriter.WriteFloatFrames(sysStem, AudioFormat::kBlockFrames)) {
                Log::Warn(L"Stem flush write failed");
                m_micStemWriter.Discard();
                m_sysStemWriter.Discard();
                m_stemWritersOpen = false;
            }
        }
    }

    if (comInitialized && comHr != RPC_E_CHANGED_MODE) {
        ::CoUninitialize();
    }
}
