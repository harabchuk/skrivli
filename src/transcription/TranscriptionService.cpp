#include "transcription/TranscriptionService.hpp"

#include "Ids.hpp"
#include "storage/RecordingLibrary.hpp"
#include "storage/Settings.hpp"
#include "transcription/AudioFileReader.hpp"
#include "transcription/MarkdownFormatter.hpp"
#include "transcription/ModelManager.hpp"
#include "transcription/Resampler.hpp"
#include "util/Log.hpp"

#include <objbase.h>

#include <algorithm>
#include <chrono>
#include <cmath>
#include <filesystem>

namespace {

constexpr uint64_t kIdleUnloadMs = 5ull * 60ull * 1000ull;
constexpr double kChunkSeconds = 30.0 * 60.0;
constexpr double kLongThresholdSeconds = 60.0 * 60.0;
constexpr double kOverlapSeconds = 1.0;

std::string TitleFromStartLocal(const std::string& startLocal) {
    // 2026-11-01T13:18:00 → 2026-11-01 13:18
    if (startLocal.size() >= 16 && startLocal[10] == 'T') {
        std::string out = startLocal.substr(0, 16);
        out[10] = ' ';
        return out;
    }
    return startLocal.empty() ? "Meeting" : startLocal;
}

std::string RecordedRange(const RecordingSidecar& side) {
    auto shortStamp = [](const std::string& stamp) {
        if (stamp.size() >= 16 && stamp[10] == 'T') {
            std::string out = stamp.substr(0, 16);
            out[10] = ' ';
            return out;
        }
        return stamp;
    };
    const std::string start = shortStamp(side.startLocal);
    std::string end = shortStamp(side.endLocal);
    if (end.size() >= 16 && start.size() >= 10 && end.compare(0, 10, start, 0, 10) == 0) {
        // Same day → show end time only after en-dash.
        end = end.substr(11);
    }
    if (start.empty()) {
        return end;
    }
    if (end.empty()) {
        return start;
    }
    return start + " – " + end;
}

}  // namespace

TranscriptionService& TranscriptionService::Instance() {
    static TranscriptionService instance;
    return instance;
}

TranscriptionService::~TranscriptionService() {
    Stop();
}

void TranscriptionService::SetNotifyHwnd(HWND hwnd) {
    m_notifyHwnd = hwnd;
}

void TranscriptionService::Start() {
    bool expected = false;
    if (!m_running.compare_exchange_strong(expected, true)) {
        return;
    }
    m_worker = std::thread([this] { WorkerMain(); });
}

void TranscriptionService::Stop() {
    {
        std::lock_guard lock(m_mutex);
        if (!m_running.load()) {
            return;
        }
        m_running.store(false);
        m_cancel.store(true);
        m_queue.clear();
    }
    m_cv.notify_all();
    if (m_worker.joinable()) {
        m_worker.join();
    }
    m_engine.Unload();
}

bool TranscriptionService::Enqueue(HWND ownerForModelUi, const std::wstring& audioPath) {
    if (audioPath.empty()) {
        return false;
    }

    Start();

    const auto& settings = Settings::Instance().Data();
    std::wstring modelFile = settings.model.empty() ? L"ggml-base-q5_1.bin" : settings.model;
    const std::wstring modelPath = ModelManager::EnsureModel(ownerForModelUi, modelFile);
    if (modelPath.empty()) {
        return false;
    }
    modelFile = std::filesystem::path(modelPath).filename().wstring();

    Job job{};
    job.audioPath = audioPath;
    job.modelPath = modelPath;
    job.modelFileName = modelFile;
    job.language = Sidecar::WideToUtf8(settings.language.empty() ? L"auto" : settings.language);
    job.threads = settings.threads;
    job.useVad = settings.useVad;

    {
        std::lock_guard lock(m_mutex);
        for (const auto& existing : m_queue) {
            if (_wcsicmp(existing.audioPath.c_str(), audioPath.c_str()) == 0) {
                return true;
            }
        }
        if (_wcsicmp(m_activePath.c_str(), audioPath.c_str()) == 0) {
            return true;
        }
        m_queue.push_back(std::move(job));
    }

    UpdateSidecarStatus(audioPath, TranscriptStatus::Queued, {}, Sidecar::WideToUtf8(modelFile), {});
    auto* notify = new TranscribeNotify{};
    notify->audioPath = audioPath;
    PostNotify(WM_APP_TRANSCRIBE_QUEUED, notify);
    m_cv.notify_one();
    return true;
}

void TranscriptionService::Cancel(const std::wstring& audioPath) {
    bool removedQueued = false;
    {
        std::lock_guard lock(m_mutex);
        const auto before = m_queue.size();
        m_queue.erase(
            std::remove_if(
                m_queue.begin(),
                m_queue.end(),
                [&](const Job& job) { return _wcsicmp(job.audioPath.c_str(), audioPath.c_str()) == 0; }),
            m_queue.end());
        removedQueued = m_queue.size() != before;
        if (_wcsicmp(m_activePath.c_str(), audioPath.c_str()) == 0) {
            m_cancel.store(true);
        }
    }

    if (removedQueued) {
        UpdateSidecarStatus(audioPath, TranscriptStatus::Cancelled, "Cancelled", {}, {});
        auto* notify = new TranscribeNotify{};
        notify->audioPath = audioPath;
        notify->message = L"Cancelled";
        PostNotify(WM_APP_TRANSCRIBE_FAILED, notify);
    }
}

bool TranscriptionService::IsActivePath(const std::wstring& audioPath) const {
    std::lock_guard lock(m_mutex);
    return _wcsicmp(m_activePath.c_str(), audioPath.c_str()) == 0;
}

bool TranscriptionService::IsQueuedOrActive(const std::wstring& audioPath) const {
    std::lock_guard lock(m_mutex);
    if (_wcsicmp(m_activePath.c_str(), audioPath.c_str()) == 0) {
        return true;
    }
    for (const auto& job : m_queue) {
        if (_wcsicmp(job.audioPath.c_str(), audioPath.c_str()) == 0) {
            return true;
        }
    }
    return false;
}

int TranscriptionService::ActivePercent() const {
    return m_percent.load();
}

std::wstring TranscriptionService::ActivePath() const {
    std::lock_guard lock(m_mutex);
    return m_activePath;
}

void TranscriptionService::PostNotify(UINT msg, TranscribeNotify* notify) {
    const HWND hwnd = m_notifyHwnd;
    if (hwnd == nullptr || !::IsWindow(hwnd)) {
        delete notify;
        return;
    }
    if (!::PostMessageW(hwnd, msg, 0, reinterpret_cast<LPARAM>(notify))) {
        delete notify;
    }
}

void TranscriptionService::UpdateSidecarStatus(
    const std::wstring& audioPath,
    TranscriptStatus status,
    const std::string& error,
    const std::string& model,
    const std::string& language) {
    RecordingSidecar side{};
    const std::wstring jsonPath = RecordingLibrary::JsonPathForAudio(audioPath);
    if (!Sidecar::Load(jsonPath, side)) {
        side.startLocal = Sidecar::FormatLocalTimestampNow();
        side.endLocal = side.startLocal;
        side.durationSeconds = RecordingLibrary::ReadWavDurationSeconds(audioPath);
        side.sampleRate = 48000;
        side.channels = 2;
    }
    side.transcriptStatus = status;
    side.transcriptError = error;
    if (!model.empty()) {
        side.model = model;
    }
    if (!language.empty()) {
        side.language = language;
    }
    Sidecar::Save(jsonPath, side);
}

void TranscriptionService::WorkerMain() {
    ::CoInitializeEx(nullptr, COINIT_MULTITHREADED);

    while (m_running.load()) {
        Job job{};
        {
            std::unique_lock lock(m_mutex);
            m_cv.wait_for(lock, std::chrono::seconds(5), [this] {
                return !m_running.load() || !m_queue.empty();
            });
            if (!m_running.load()) {
                break;
            }
            if (m_queue.empty()) {
                m_engine.UnloadIfIdle(kIdleUnloadMs);
                continue;
            }
            job = std::move(m_queue.front());
            m_queue.pop_front();
            m_activePath = job.audioPath;
            m_cancel.store(false);
            m_percent.store(0);
        }

        ProcessJob(std::move(job));

        {
            std::lock_guard lock(m_mutex);
            m_activePath.clear();
            m_percent.store(0);
        }
    }

    ::CoUninitialize();
}

void TranscriptionService::ProcessJob(Job job) {
    UpdateSidecarStatus(
        job.audioPath,
        TranscriptStatus::Preparing,
        {},
        Sidecar::WideToUtf8(job.modelFileName),
        job.language);

    auto* preparing = new TranscribeNotify{};
    preparing->audioPath = job.audioPath;
    preparing->percent = 0;
    PostNotify(WM_APP_TRANSCRIBE_PROGRESS, preparing);

    std::wstring error;
    if (!m_engine.EnsureLoaded(job.modelPath, error)) {
        UpdateSidecarStatus(job.audioPath, TranscriptStatus::Failed, Sidecar::WideToUtf8(error), {}, {});
        auto* notify = new TranscribeNotify{};
        notify->audioPath = job.audioPath;
        notify->message = error;
        PostNotify(WM_APP_TRANSCRIBE_FAILED, notify);
        return;
    }

    DecodedAudio decoded{};
    if (!AudioFileReader::Read(job.audioPath, decoded, error)) {
        UpdateSidecarStatus(job.audioPath, TranscriptStatus::Failed, Sidecar::WideToUtf8(error), {}, {});
        auto* notify = new TranscribeNotify{};
        notify->audioPath = job.audioPath;
        notify->message = error;
        PostNotify(WM_APP_TRANSCRIBE_FAILED, notify);
        return;
    }

    if (m_cancel.load()) {
        UpdateSidecarStatus(job.audioPath, TranscriptStatus::Cancelled, "Cancelled", {}, {});
        auto* notify = new TranscribeNotify{};
        notify->audioPath = job.audioPath;
        notify->message = L"Cancelled";
        PostNotify(WM_APP_TRANSCRIBE_FAILED, notify);
        return;
    }

    const size_t totalFrames48 = decoded.interleavedStereo.size() / 2;
    const bool chunked = decoded.durationSeconds > kLongThresholdSeconds;
    const size_t chunkFrames48 = static_cast<size_t>(kChunkSeconds * 48000.0);
    const size_t overlapFrames48 = static_cast<size_t>(kOverlapSeconds * 48000.0);

    std::vector<TranscriptSegment> allSegments;
    std::string detectedLanguage;

    UpdateSidecarStatus(
        job.audioPath,
        TranscriptStatus::Transcribing,
        {},
        Sidecar::WideToUtf8(job.modelFileName),
        job.language);

    const size_t chunkCount = chunked
        ? (std::max)(size_t{1}, (totalFrames48 + chunkFrames48 - 1) / chunkFrames48)
        : size_t{1};

    size_t frameOffset = 0;
    size_t chunkIndex = 0;
    while (frameOffset < totalFrames48) {
        if (m_cancel.load()) {
            UpdateSidecarStatus(job.audioPath, TranscriptStatus::Cancelled, "Cancelled", {}, {});
            auto* notify = new TranscribeNotify{};
            notify->audioPath = job.audioPath;
            notify->message = L"Cancelled";
            PostNotify(WM_APP_TRANSCRIBE_FAILED, notify);
            return;
        }

        size_t frames = chunked ? (std::min)(chunkFrames48, totalFrames48 - frameOffset) : totalFrames48;
        const float* stereo = decoded.interleavedStereo.data() + frameOffset * 2;
        auto mono16k = Resampler::Stereo48kToMono16k(stereo, frames);

        const int basePercent = chunked
            ? static_cast<int>((chunkIndex * 100) / chunkCount)
            : 0;
        const int span = chunked ? static_cast<int>(100 / (std::max)(size_t{1}, chunkCount)) : 100;

        std::vector<TranscriptSegment> segments;
        std::string chunkLang;
        const bool ok = m_engine.Transcribe(
            mono16k,
            job.language,
            job.threads,
            job.useVad,
            ModelManager::VadModelPresent() ? ModelManager::VadModelPath() : std::wstring{},
            m_cancel,
            [&](int percent) {
                const int mapped = basePercent + (percent * span) / 100;
                m_percent.store(mapped);
                auto* notify = new TranscribeNotify{};
                notify->audioPath = job.audioPath;
                notify->percent = mapped;
                PostNotify(WM_APP_TRANSCRIBE_PROGRESS, notify);
            },
            segments,
            chunkLang,
            error);

        if (!ok) {
            const bool cancelled = m_cancel.load() || error == L"Cancelled.";
            UpdateSidecarStatus(
                job.audioPath,
                cancelled ? TranscriptStatus::Cancelled : TranscriptStatus::Failed,
                Sidecar::WideToUtf8(error),
                {},
                {});
            auto* notify = new TranscribeNotify{};
            notify->audioPath = job.audioPath;
            notify->message = error;
            PostNotify(WM_APP_TRANSCRIBE_FAILED, notify);
            return;
        }

        if (detectedLanguage.empty()) {
            detectedLanguage = chunkLang;
        }

        const int64_t offsetTenths = static_cast<int64_t>((frameOffset * 100.0) / 48000.0);
        for (auto& seg : segments) {
            seg.t0 += offsetTenths;
            seg.t1 += offsetTenths;
            allSegments.push_back(std::move(seg));
        }

        if (!chunked) {
            break;
        }

        if (frameOffset + frames >= totalFrames48) {
            break;
        }
        const size_t advance = frames > overlapFrames48 ? frames - overlapFrames48 : frames;
        frameOffset += advance;
        ++chunkIndex;
    }

    RecordingSidecar side{};
    Sidecar::Load(RecordingLibrary::JsonPathForAudio(job.audioPath), side);

    TranscriptMeta meta{};
    meta.titleDateTime = TitleFromStartLocal(side.startLocal);
    meta.durationSeconds = side.durationSeconds > 0.0 ? side.durationSeconds : decoded.durationSeconds;
    meta.recordedRange = RecordedRange(side);
    meta.modelFile = Sidecar::WideToUtf8(job.modelFileName);
    meta.languageConfigured = job.language;
    meta.languageDetected = detectedLanguage;

    const std::string markdown = MarkdownFormatter::Format(meta, allSegments);
    const std::wstring mdPath = RecordingLibrary::MdPathForAudio(job.audioPath);
    if (!MarkdownFormatter::WriteUtf8NoBom(mdPath, markdown)) {
        UpdateSidecarStatus(job.audioPath, TranscriptStatus::Failed, "Failed to write markdown", {}, {});
        auto* notify = new TranscribeNotify{};
        notify->audioPath = job.audioPath;
        notify->message = L"Failed to write markdown";
        PostNotify(WM_APP_TRANSCRIBE_FAILED, notify);
        return;
    }

    std::string langOut = job.language;
    if (!detectedLanguage.empty()) {
        langOut = detectedLanguage;
    }
    UpdateSidecarStatus(
        job.audioPath,
        TranscriptStatus::Ready,
        {},
        Sidecar::WideToUtf8(job.modelFileName),
        langOut);

    auto* done = new TranscribeNotify{};
    done->audioPath = job.audioPath;
    done->percent = 100;
    PostNotify(WM_APP_TRANSCRIBE_FINISHED, done);
    Log::Info(L"Transcript ready: " + mdPath);
}
