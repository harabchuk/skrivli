#include "services/RecordingSession.hpp"

#include "Ids.hpp"
#include "infrastructure/common/FileIo.hpp"
#include "infrastructure/common/Log.hpp"
#include "infrastructure/common/Paths.hpp"
#include "infrastructure/common/PostOwned.hpp"
#include "infrastructure/common/Scoped.hpp"
#include "infrastructure/common/Text.hpp"

#include <windows.h>

#include <algorithm>
#include <audioclient.h>
#include <cmath>

namespace {

constexpr DWORD kDiskCheckIntervalMs = 30000;

} // namespace

RecordingSession::CaptureSource::CaptureSource(RecordingSession& session, RingBuffer& sourceRing,
                                               CaptureStream::Mode sourceMode)
    : owner(session), ring(sourceRing), mode(sourceMode),
      label(sourceMode == CaptureStream::Mode::Microphone ? L"Microphone" : L"System audio") {
    if (sourceMode == CaptureStream::Mode::Loopback) {
        keepAlive.emplace();
    }
}

bool RecordingSession::CaptureSource::Open(IMMDevice* device, const wchar_t* reason) {
    const bool reopen = reason != nullptr;

    if (keepAlive && !keepAlive->Start(device)) {
        if (reopen) {
            owner.AddDeviceNote(L"Keep-alive failed after system device switch");
        } else {
            Log::Warn(L"Failed to start system-audio keep-alive; continuing without it");
            owner.AddDeviceNote(L"System keep-alive failed to start");
        }
    }

    const bool communications =
        mode == CaptureStream::Mode::Microphone && owner.m_communicationsProcessing;
    if (!stream.Start(device, mode, ring, communications)) {
        enabled = false;
        if (keepAlive) {
            keepAlive->Stop();
        }
        if (reopen) {
            owner.AddDeviceNote(std::wstring(label) + L" reopen failed (" + reason + L")");
        }
        return false;
    }

    enabled = true;
    name = owner.m_devices->FriendlyName(device);
    if (reopen) {
        owner.AddDeviceNote(std::wstring(label) + L" switched (" + reason + L") → " + name);
        owner.Post(WM_APP_DEVICE_SWITCHED, L"Device lost — recording continues on " + name);
    } else if (stream.UsedResampler()) {
        owner.AddDeviceNote(std::wstring(label) + L" used Media Foundation resampler fallback");
    }
    return true;
}

void RecordingSession::CaptureSource::NoteBufferFlags() {
    // Once each; not on the capture hot path.
    if (!notedSilent && stream.SawSilentPacket()) {
        notedSilent = true;
        owner.AddDeviceNote(std::wstring(label) +
                            L" delivered silent packets (AUDCLNT_BUFFERFLAGS_SILENT)");
    }
    if (!notedDiscontinuity && stream.SawDiscontinuity()) {
        notedDiscontinuity = true;
        owner.AddDeviceNote(std::wstring(label) + L" reported DATA_DISCONTINUITY");
    }
}

void RecordingSession::CaptureSource::Stop() {
    stream.Stop();
    if (keepAlive) {
        keepAlive->Stop();
    }
}

auto RecordingSession::CaptureSource::Resolve(const DeviceManager& devices) const
    -> wil::com_ptr_nothrow<IMMDevice> {
    const auto id = usesDefault ? std::wstring{} : deviceId;
    return mode == CaptureStream::Mode::Microphone ? devices.ResolveCaptureDevice(id)
                                                   : devices.ResolveRenderDevice(id);
}

bool RecordingSession::StemWriters::Open(const std::wstring& stemBase) {
    if (!m_mic.OpenFinalPath(stemBase + L".mic.wav") ||
        !m_sys.OpenFinalPath(stemBase + L".system.wav")) {
        return false;
    }
    m_open = true;
    return true;
}

bool RecordingSession::StemWriters::Write(const float* mic, const float* sys) {
    return m_mic.WriteFloatFrames(mic, AudioFormat::kBlockFrames) &&
           m_sys.WriteFloatFrames(sys, AudioFormat::kBlockFrames);
}

void RecordingSession::StemWriters::Finalize() {
    if (!m_open) {
        return;
    }
    std::wstring micPath;
    std::wstring sysPath;
    if (!m_mic.Finalize(micPath)) {
        m_mic.Abandon();
        Log::Warn(L"Failed to finalize mic stem");
    }
    if (!m_sys.Finalize(sysPath)) {
        m_sys.Abandon();
        Log::Warn(L"Failed to finalize system stem");
    }
    m_open = false;
}

void RecordingSession::StemWriters::Discard() {
    m_mic.Discard();
    m_sys.Discard();
    m_open = false;
}

RecordingSession::RecordingSession()
    : m_mic(*this, m_micRing, CaptureStream::Mode::Microphone),
      m_sys(*this, m_sysRing, CaptureStream::Mode::Loopback) {}

RecordingSession::~RecordingSession() {
    Stop(StopMode::Save);
}

void RecordingSession::SetState(State next) {
    m_state.store(next, std::memory_order_release);
}

void RecordingSession::Post(UINT msg, std::optional<std::wstring> payload) {
    std::unique_ptr<std::wstring> owned;
    if (payload) {
        owned = std::make_unique<std::wstring>(std::move(*payload));
    }
    PostOwned(m_notifyHwnd, msg, std::move(owned));
}

bool RecordingSession::IsPrivacyDenied(HRESULT hr) {
    return hr == E_ACCESSDENIED;
}

bool RecordingSession::IsDeviceBusy(HRESULT hr) {
    return hr == AUDCLNT_E_DEVICE_IN_USE || hr == AUDCLNT_E_EXCLUSIVE_MODE_ONLY ||
           hr == AUDCLNT_E_ENDPOINT_CREATE_FAILED;
}

auto RecordingSession::DescribeCaptureError(HRESULT hr, bool isMic) -> std::wstring {
    const wchar_t* which = isMic ? L"microphone" : L"system audio";
    if (IsPrivacyDenied(hr)) {
        return L"Microphone access is denied. Enable microphone privacy in Windows Settings.";
    }
    if (IsDeviceBusy(hr)) {
        return std::wstring(L"The ") + which + L" is in exclusive use by another application.";
    }
    wchar_t buf[128]{};
    std::swprintf(buf, 128, L"Failed to open the %s (HRESULT 0x%08X).", which,
                  static_cast<unsigned>(hr));
    return buf;
}

void RecordingSession::AddDeviceNote(const std::wstring& note) {
    std::lock_guard<std::mutex> lock(m_notesMutex);
    m_deviceNotes.push_back(note);
    Log::Info(L"Device note: " + note);
}

auto RecordingSession::TakeDeviceNotes() -> std::vector<std::wstring> {
    std::lock_guard<std::mutex> lock(m_notesMutex);
    std::vector<std::wstring> notes;
    notes.swap(m_deviceNotes);
    return notes;
}

auto RecordingSession::MicDeviceName() const -> std::wstring {
    return m_mic.name;
}

auto RecordingSession::SystemDeviceName() const -> std::wstring {
    return m_sys.name;
}

double RecordingSession::ElapsedSeconds() const {
    const auto start = m_startQpc.load(std::memory_order_acquire);
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
    return static_cast<double>(static_cast<uint64_t>(now.QuadPart) - start) /
           static_cast<double>(freq.QuadPart);
}

void RecordingSession::FailStart(RecordingStartResult& result, const std::wstring& message) {
    Log::Error(message);
    TeardownCapture();
    m_stems.Discard();
    m_writer.Discard();
    SetState(State::Error);
    result.started = false;
    result.error = message;
    SetState(State::Idle);
}

void RecordingSession::TeardownCapture() {
    m_mic.Stop();
    m_sys.Stop();
}

void RecordingSession::OnDevicesChanged() {
    if (IsActive()) {
        m_devicesChanged.store(true, std::memory_order_release);
    }
}

void RecordingSession::SetMicGain(float micGain) {
    if (!std::isfinite(micGain)) {
        return;
    }
    m_micGain = std::clamp(micGain, 0.0f, 4.0f);
    m_mixer.SetGains(m_micGain, m_systemGain);
}

void RecordingSession::SetMicAgc(bool enabled) {
    m_micAgc = enabled;
    m_mixer.SetMicAgc(enabled);
}

void RecordingSession::ResetForStart(const std::wstring& micDeviceId,
                                     const std::wstring& systemDeviceId, float micGain, bool micAgc,
                                     float systemGain, bool communicationsProcessing,
                                     bool saveStems) {
    m_stopWriter.store(false, std::memory_order_release);
    m_devicesChanged.store(false, std::memory_order_release);
    m_micRing.Reset();
    m_sysRing.Reset();
    {
        std::lock_guard<std::mutex> notesLock(m_notesMutex);
        m_deviceNotes.clear();
    }

    m_mic.deviceId = micDeviceId;
    m_sys.deviceId = systemDeviceId;
    m_mic.usesDefault = micDeviceId.empty();
    m_sys.usesDefault = systemDeviceId.empty();
    m_communicationsProcessing = communicationsProcessing;
    m_saveStems = saveStems;
    m_micGain = micGain;
    m_micAgc = micAgc;
    m_systemGain = systemGain;
    for (auto* source : {&m_mic, &m_sys}) {
        source->enabled = false;
        source->name.clear();
        source->notedSilent = false;
        source->notedDiscontinuity = false;
    }
}

bool RecordingSession::OpenOutputFiles(const std::wstring& folder, RecordingStartResult& result) {
    if (!FileIo::IsWritableDirectory(folder)) {
        FailStart(result, L"The recordings folder is missing or not writable.");
        return false;
    }
    if (!WavWriter::FolderHasFreeSpace(folder)) {
        FailStart(result, L"Not enough free disk space to start recording (need at least 200 MB).");
        return false;
    }

    if (!m_writer.Open(folder)) {
        FailStart(result, L"Could not create a recording file in the chosen folder.");
        return false;
    }

    if (m_saveStems) {
        auto stemBase = m_writer.FinalPath();
        const std::wstring wavExt = L".wav";
        if (Text::EndsWithIgnoreCase(stemBase, wavExt)) {
            stemBase.erase(stemBase.size() - wavExt.size());
        }
        if (!m_stems.Open(stemBase)) {
            FailStart(result, L"Could not create stem recording files.");
            return false;
        }
    }
    return true;
}

bool RecordingSession::OpenSources(RecordingStartResult& result) {
    auto micDevice = m_devices->ResolveCaptureDevice(m_mic.deviceId);
    auto renderDevice = m_devices->ResolveRenderDevice(m_sys.deviceId);
    const bool micPresent = static_cast<bool>(micDevice);
    const bool sysPresent = static_cast<bool>(renderDevice);

    if (!micPresent && !sysPresent) {
        FailStart(result, L"No microphone or system audio device is available.");
        return false;
    }

    if (!micPresent) {
        result.warning = L"No microphone — recording system audio only.";
        AddDeviceNote(L"Started without microphone");
    }
    if (!sysPresent) {
        result.warning = result.warning.empty()
                             ? L"No system audio device — recording microphone only."
                             : result.warning;
        AddDeviceNote(L"Started without system audio");
    }

    if (sysPresent && !m_sys.Open(renderDevice.get(), nullptr)) {
        const auto hr = m_sys.stream.FatalError();
        Log::Warn(DescribeCaptureError(hr, false));
        if (IsDeviceBusy(hr) && micPresent) {
            result.warning = L"System audio unavailable (in use). Recording microphone only.";
            AddDeviceNote(L"System audio exclusive/busy at start — mic only");
        } else if (!micPresent) {
            FailStart(result, DescribeCaptureError(hr, false));
            return false;
        } else {
            result.warning = L"System audio failed to open. Recording microphone only.";
            AddDeviceNote(L"System audio failed to open at start — mic only");
        }
    }

    if (micPresent && !m_mic.Open(micDevice.get(), nullptr)) {
        const auto hr = m_mic.stream.FatalError();
        if (IsPrivacyDenied(hr)) {
            TeardownCapture();
            m_stems.Discard();
            m_writer.Discard();
            SetState(State::Idle);
            result.privacyDenied = true;
            result.error = DescribeCaptureError(hr, true);
            return false;
        }
        if (IsDeviceBusy(hr) && m_sys.enabled) {
            result.warning = L"Microphone unavailable (in use). Recording system audio only.";
            AddDeviceNote(L"Microphone exclusive/busy at start — system only");
        } else if (!m_sys.enabled) {
            FailStart(result, DescribeCaptureError(hr, true));
            return false;
        } else {
            result.warning = L"Microphone failed to open. Recording system audio only.";
            AddDeviceNote(L"Microphone failed to open at start — system only");
        }
    }

    if (!m_mic.enabled && !m_sys.enabled) {
        FailStart(result, L"Could not open any audio device.");
        return false;
    }
    return true;
}

RecordingStartResult RecordingSession::Start(const std::wstring& folder,
                                             const std::wstring& micDeviceId,
                                             const std::wstring& systemDeviceId, float micGain,
                                             bool micAgc, float systemGain,
                                             bool communicationsProcessing, bool saveStems) {
    std::lock_guard<std::mutex> lock(m_startStopMutex);
    RecordingStartResult result{};

    const auto state = GetState();
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
    ResetForStart(micDeviceId, systemDeviceId, micGain, micAgc, systemGain,
                  communicationsProcessing, saveStems);

    if (!OpenOutputFiles(folder, result) || !OpenSources(result)) {
        return result;
    }

    ::GetLocalTime(&m_startLocal);

    LARGE_INTEGER qpc{};
    LARGE_INTEGER freq{};
    ::QueryPerformanceCounter(&qpc);
    ::QueryPerformanceFrequency(&freq);
    m_startQpc.store(static_cast<uint64_t>(qpc.QuadPart), std::memory_order_release);
    m_mixer.Reset(static_cast<uint64_t>(qpc.QuadPart), static_cast<uint64_t>(freq.QuadPart),
                  micGain, systemGain, micAgc);

    try {
        m_writerThread = std::thread([this]() { WriterMain(); });
    } catch (...) {
        FailStart(result, L"Failed to start the recording writer thread.");
        return result;
    }

    SetState(State::Recording);
    Post(WM_APP_RECORDING_STARTED);
    result.started = true;
    return result;
}

void RecordingSession::Stop(StopMode mode) {
    std::lock_guard<std::mutex> lock(m_startStopMutex);

    const auto state = GetState();
    if (state != State::Recording && state != State::Starting) {
        if (m_writerThread.joinable()) {
            m_stopWriter.store(true, std::memory_order_release);
            m_writerThread.join();
        }
        if (mode == StopMode::Discard) {
            m_stems.Discard();
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
        m_stems.Discard();
        m_writer.Discard();
        SetState(State::Idle);
        return;
    }

    m_stems.Finalize();

    // Prefer rescue if the writer already moved the file.
    if (!m_writer.IsOpen()) {
        SetState(State::Idle);
        return;
    }

    std::wstring finalPath;
    if (m_writer.Finalize(finalPath)) {
        SetState(State::Idle);
        Post(WM_APP_RECORDING_FINISHED, finalPath);
        return;
    }

    // Finalize failed (folder gone?): try rescue.
    std::wstring rescued;
    if (m_writer.IsOpen() && m_writer.RescueTo(Paths::RescueDir(), rescued)) {
        SetState(State::Idle);
        Post(WM_APP_RECORDING_RESCUED, rescued);
        return;
    }

    m_writer.Abandon();
    SetState(State::Error);
    Post(WM_APP_RECORDING_ERROR, L"Failed to finalize the recording file.");
    SetState(State::Idle);
}

bool RecordingSession::Reopen(CaptureSource& source, const wchar_t* reason) {
    std::lock_guard<std::mutex> lock(m_reopenMutex);
    if (m_devices == nullptr || m_stopWriter.load(std::memory_order_acquire)) {
        return false;
    }

    source.Stop();
    auto device = source.Resolve(*m_devices);
    if (!device) {
        source.enabled = false;
        AddDeviceNote(std::wstring(source.label) + L" lost (" + reason + L") — no replacement");
        return false;
    }
    return source.Open(device.get(), reason);
}

void RecordingSession::ServiceDevices() {
    if (m_mic.enabled && m_mic.stream.DeviceInvalidated()) {
        Reopen(m_mic, L"invalidated");
    }
    if (m_sys.enabled && m_sys.stream.DeviceInvalidated()) {
        Reopen(m_sys, L"invalidated");
    }

    if (m_devicesChanged.exchange(false, std::memory_order_acq_rel)) {
        if (m_mic.usesDefault) {
            Reopen(m_mic, L"default changed");
        }
        if (m_sys.usesDefault) {
            Reopen(m_sys, L"default changed");
        }
    }

    m_mic.NoteBufferFlags();
    m_sys.NoteBufferFlags();
}

bool RecordingSession::CheckDiskSpace(ULONGLONG& lastCheck) {
    const auto nowTick = ::GetTickCount64();
    if (nowTick - lastCheck < kDiskCheckIntervalMs) {
        return true;
    }
    lastCheck = nowTick;
    const auto folder = m_writer.Folder();
    if (!folder.empty() && !WavWriter::FolderHasFreeSpace(folder)) {
        Post(WM_APP_LOW_DISK);
        m_stopWriter.store(true, std::memory_order_release);
        return false;
    }
    return true;
}

bool RecordingSession::WriteBlock(const float* block, const float* micStem, const float* sysStem,
                                  bool flushing, bool& rescued) {
    if (!m_writer.WriteFloatFrames(block, AudioFormat::kBlockFrames)) {
        if (flushing) {
            if (!rescued) {
                std::wstring rescuedPath;
                if (m_writer.RescueTo(Paths::RescueDir(), rescuedPath)) {
                    rescued = true;
                    Post(WM_APP_RECORDING_RESCUED, rescuedPath);
                }
            }
            return false;
        }
        std::wstring rescuedPath;
        if (m_writer.RescueTo(Paths::RescueDir(), rescuedPath)) {
            rescued = true;
            Post(WM_APP_RECORDING_RESCUED, rescuedPath);
        } else {
            Post(WM_APP_RECORDING_ERROR, L"Failed while writing the recording.");
        }
        m_stopWriter.store(true, std::memory_order_release);
        return false;
    }
    if (m_stems.IsOpen() && !m_stems.Write(micStem, sysStem)) {
        Log::Warn(flushing ? L"Stem flush write failed"
                           : L"Stem write failed; continuing with mix only");
        m_stems.Discard();
    }
    return true;
}

void RecordingSession::WriterMain() {
    ScopedComInit com(COINIT_MULTITHREADED);

    float block[AudioFormat::kBlockFrames * AudioFormat::kChannels]{};
    float micStem[AudioFormat::kBlockFrames * AudioFormat::kChannels]{};
    float sysStem[AudioFormat::kBlockFrames * AudioFormat::kChannels]{};
    auto lastDiskCheck = ::GetTickCount64();
    bool rescued = false;

    while (!m_stopWriter.load(std::memory_order_acquire)) {
        ServiceDevices();
        if (!CheckDiskSpace(lastDiskCheck)) {
            break;
        }

        LARGE_INTEGER now{};
        ::QueryPerformanceCounter(&now);
        float* micOut = m_stems.IsOpen() ? micStem : nullptr;
        float* sysOut = m_stems.IsOpen() ? sysStem : nullptr;
        if (m_mixer.Process(m_micRing, m_sysRing, static_cast<uint64_t>(now.QuadPart), block,
                            micOut, sysOut)) {
            if (!WriteBlock(block, micStem, sysStem, false, rescued)) {
                break;
            }
        } else {
            ::Sleep(2);
        }
    }

    while (m_mixer.Flush(m_micRing, m_sysRing, block, m_stems.IsOpen() ? micStem : nullptr,
                         m_stems.IsOpen() ? sysStem : nullptr)) {
        if (!m_writer.IsOpen()) {
            break;
        }
        if (!WriteBlock(block, micStem, sysStem, true, rescued)) {
            break;
        }
    }
}
