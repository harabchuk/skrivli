#pragma once

#include "storage/RecordingSidecar.hpp"
#include "transcription/WhisperEngine.hpp"

#include <atomic>
#include <condition_variable>
#include <cstdint>
#include <deque>
#include <mutex>
#include <string>
#include <thread>
#include <vector>
#include <windows.h>

struct DecodedAudio;

struct TranscribeNotify {
    std::wstring audioPath;
    std::wstring message;
    int percent = 0;
};

class TranscriptionService {
public:
    static TranscriptionService& Instance();

    void SetNotifyHwnd(HWND hwnd);
    void Start();
    void Stop();

    // Enqueues a job. Returns false if the model could not be ensured.
    bool Enqueue(HWND ownerForModelUi, const std::wstring& audioPath);
    // Request cancel for a specific path (or the active job if path matches).
    void Cancel(const std::wstring& audioPath);
    bool IsQueuedOrActive(const std::wstring& audioPath) const;

private:
    TranscriptionService() = default;
    ~TranscriptionService();

    struct Job {
        std::wstring audioPath;
        std::wstring modelPath;
        std::wstring modelFileName;
        std::string language;
        int threads = 0;
        bool useVad = false;
    };

    void WorkerMain();
    void ProcessJob(Job job);
    // Transcribes every planned chunk, appending offset-corrected segments. On failure it
    // records the failure through FailJob and returns false.
    bool TranscribeChunks(
        const Job& job,
        const DecodedAudio& decoded,
        std::vector<TranscriptSegment>& segments,
        std::string& language);
    void FailJob(const Job& job, TranscriptStatus status, const std::wstring& message);
    void UpdateSidecarStatus(
        const std::wstring& audioPath,
        TranscriptStatus status,
        const std::string& error,
        const std::string& model,
        const std::string& language);
    void Notify(UINT msg, const std::wstring& audioPath, int percent = 0, std::wstring message = {});

    HWND m_notifyHwnd = nullptr;
    mutable std::mutex m_mutex;
    std::condition_variable m_cv;
    std::deque<Job> m_queue;
    std::thread m_worker;
    std::atomic_bool m_running{false};
    std::atomic_bool m_cancel{false};
    std::wstring m_activePath;
    WhisperEngine m_engine;
};
