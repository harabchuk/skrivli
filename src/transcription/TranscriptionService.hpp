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
#include <windows.h>

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
    bool IsActivePath(const std::wstring& audioPath) const;
    bool IsQueuedOrActive(const std::wstring& audioPath) const;
    int ActivePercent() const;
    std::wstring ActivePath() const;

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
    void UpdateSidecarStatus(
        const std::wstring& audioPath,
        TranscriptStatus status,
        const std::string& error,
        const std::string& model,
        const std::string& language);
    void PostNotify(UINT msg, TranscribeNotify* notify);

    HWND m_notifyHwnd = nullptr;
    mutable std::mutex m_mutex;
    std::condition_variable m_cv;
    std::deque<Job> m_queue;
    std::thread m_worker;
    std::atomic_bool m_running{false};
    std::atomic_bool m_cancel{false};
    std::wstring m_activePath;
    std::atomic_int m_percent{0};
    WhisperEngine m_engine;
};
