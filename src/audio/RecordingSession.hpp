#pragma once

#include "audio/CaptureStream.hpp"
#include "audio/DeviceManager.hpp"
#include "audio/Mixer.hpp"
#include "audio/RingBuffer.hpp"
#include "audio/SilentRenderKeepAlive.hpp"
#include "audio/WavWriter.hpp"

#include <windows.h>

#include <atomic>
#include <mutex>
#include <optional>
#include <string>
#include <thread>
#include <vector>

struct RecordingStartResult {
    bool started = false;
    bool privacyDenied = false;
    bool micMissing = false;
    bool systemMissing = false;
    std::wstring warning; // non-fatal (e.g. one source missing)
    std::wstring error;
};

class RecordingSession {
public:
    enum class State {
        Idle,
        Starting,
        Recording,
        Stopping,
        Error,
    };

    enum class StopMode {
        Save,
        Discard,
    };

    RecordingSession();
    ~RecordingSession();

    RecordingSession(const RecordingSession&) = delete;
    RecordingSession& operator=(const RecordingSession&) = delete;

    void SetNotifyHwnd(HWND hwnd) { m_notifyHwnd = hwnd; }
    void SetDeviceManager(DeviceManager* devices) { m_devices = devices; }

    RecordingStartResult Start(
        const std::wstring& folder,
        const std::wstring& micDeviceId,
        const std::wstring& systemDeviceId,
        float micGain,
        bool micAgc,
        float systemGain,
        bool communicationsProcessing,
        bool saveStems = false);

    void Stop(StopMode mode = StopMode::Save);

    // Called from UI when IMMNotificationClient reports a change.
    void OnDevicesChanged();

    // Live mic mix gain / AGC (applied immediately while recording).
    void SetMicGain(float micGain);
    void SetMicAgc(bool enabled);

    State GetState() const { return m_state.load(std::memory_order_acquire); }
    bool IsActive() const {
        const State state = GetState();
        return state == State::Starting || state == State::Recording || state == State::Stopping;
    }

    float PeakMic() const { return m_mixer.PeakMic(); }
    float PeakSys() const { return m_mixer.PeakSys(); }
    float EffectiveMicGain() const { return m_mixer.EffectiveMicGain(); }
    uint64_t StartQpc() const { return m_startQpc.load(std::memory_order_acquire); }

    std::vector<std::wstring> TakeDeviceNotes();
    std::wstring MicDeviceName() const;
    std::wstring SystemDeviceName() const;
    SYSTEMTIME StartLocalTime() const { return m_startLocal; }
    double ElapsedSeconds() const;

private:
    // One capture input (mic or loopback) plus its ring buffer and device bookkeeping.
    struct CaptureSource {
        CaptureSource(RecordingSession& session, RingBuffer& sourceRing, CaptureStream::Mode sourceMode);

        // `reason == nullptr` is the initial open at Start; otherwise a reopen after a device change.
        bool Open(IMMDevice* device, const wchar_t* reason);
        void NoteBufferFlags();
        void Stop();
        Microsoft::WRL::ComPtr<IMMDevice> Resolve(const DeviceManager& devices) const;

        RecordingSession& owner;
        CaptureStream stream;
        RingBuffer& ring;
        std::wstring deviceId;
        bool usesDefault = true;
        bool enabled = false;
        std::wstring name;
        CaptureStream::Mode mode;
        const wchar_t* label;
        std::optional<SilentRenderKeepAlive> keepAlive;
        bool notedSilent = false;
        bool notedDiscontinuity = false;
    };

    // Optional per-source WAV stems written next to the mix.
    class StemWriters {
    public:
        bool Open(const std::wstring& stemBase);
        bool Write(const float* mic, const float* sys);
        void Finalize();
        void Discard();
        bool IsOpen() const { return m_open; }

    private:
        WavWriter m_mic;
        WavWriter m_sys;
        bool m_open = false;
    };

    void SetState(State next);
    void WriterMain();
    void ServiceDevices();
    bool CheckDiskSpace(ULONGLONG& lastCheck);
    bool WriteBlock(
        const float* block,
        const float* micStem,
        const float* sysStem,
        bool flushing,
        bool& rescued);
    void Post(UINT msg, std::optional<std::wstring> payload = {});
    void FailStart(RecordingStartResult& result, const std::wstring& message);
    void ResetForStart(
        const std::wstring& micDeviceId,
        const std::wstring& systemDeviceId,
        float micGain,
        bool micAgc,
        float systemGain,
        bool communicationsProcessing,
        bool saveStems);
    bool OpenOutputFiles(const std::wstring& folder, RecordingStartResult& result);
    bool OpenSources(RecordingStartResult& result);
    void TeardownCapture();
    bool Reopen(CaptureSource& source, const wchar_t* reason);
    void AddDeviceNote(const std::wstring& note);
    static bool IsPrivacyDenied(HRESULT hr);
    static bool IsDeviceBusy(HRESULT hr);
    static std::wstring DescribeCaptureError(HRESULT hr, bool isMic);

    HWND m_notifyHwnd = nullptr;
    DeviceManager* m_devices = nullptr;

    std::atomic<State> m_state{State::Idle};
    std::atomic<bool> m_stopWriter{false};
    std::atomic<bool> m_devicesChanged{false};
    std::atomic<bool> m_lowDiskStop{false};
    std::atomic<bool> m_writeFailed{false};
    std::atomic<uint64_t> m_startQpc{0};

    RingBuffer m_micRing{48000};
    RingBuffer m_sysRing{48000};
    CaptureSource m_mic;
    CaptureSource m_sys;
    Mixer m_mixer;
    WavWriter m_writer;
    StemWriters m_stems;
    bool m_saveStems = false;

    std::thread m_writerThread;
    std::mutex m_startStopMutex;
    std::mutex m_reopenMutex;
    std::mutex m_notesMutex;

    bool m_communicationsProcessing = false;
    float m_micGain = 1.0f;
    float m_systemGain = 0.8f;
    bool m_micAgc = false;

    std::vector<std::wstring> m_deviceNotes;
    SYSTEMTIME m_startLocal{};
};
