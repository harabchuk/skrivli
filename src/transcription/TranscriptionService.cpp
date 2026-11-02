#include "transcription/TranscriptionService.hpp"

#include "Ids.hpp"
#include "storage/RecordingLibrary.hpp"
#include "storage/Settings.hpp"
#include "transcription/AudioFileReader.hpp"
#include "transcription/ChunkPlan.hpp"
#include "transcription/MarkdownFormatter.hpp"
#include "transcription/ModelManager.hpp"
#include "transcription/Resampler.hpp"
#include "util/Log.hpp"
#include "util/PostOwned.hpp"
#include "util/Scoped.hpp"
#include "util/Text.hpp"

#include <objbase.h>

#include <algorithm>
#include <chrono>
#include <cmath>
#include <filesystem>
#include <sstream>

namespace {

constexpr uint64_t kIdleUnloadMs = 5ull * 60ull * 1000ull;

std::string TitleFromStartLocal(const std::string& startLocal) {
    return startLocal.empty() ? "Meeting" : Text::IsoToDisplayMinute(startLocal);
}

std::string RecordedRange(const RecordingSidecar& side) {
    const std::string start = Text::IsoToDisplayMinute(side.startLocal);
    std::string end = Text::IsoToDisplayMinute(side.endLocal);
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
    std::wstring modelFile = settings.model.empty() ? ModelManager::kDefaultModel : settings.model;
    const std::wstring modelPath = ModelManager::EnsureModel(ownerForModelUi, modelFile);
    if (modelPath.empty()) {
        return false;
    }
    modelFile = std::filesystem::path(modelPath).filename().wstring();

    Job job{};
    job.audioPath = audioPath;
    job.modelPath = modelPath;
    job.modelFileName = modelFile;
    job.language = Text::WideToUtf8(settings.language.empty() ? L"auto" : settings.language);
    job.threads = settings.threads;
    job.useVad = settings.useVad;

    {
        std::lock_guard lock(m_mutex);
        for (const auto& existing : m_queue) {
            if (Text::PathEquals(existing.audioPath, audioPath)) {
                return true;
            }
        }
        if (Text::PathEquals(m_activePath, audioPath)) {
            return true;
        }
        m_queue.push_back(std::move(job));
    }

    UpdateSidecarStatus(audioPath, TranscriptStatus::Queued, {}, Text::WideToUtf8(modelFile), {});
    Notify(WM_APP_TRANSCRIBE_QUEUED, audioPath);
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
                [&](const Job& job) { return Text::PathEquals(job.audioPath, audioPath); }),
            m_queue.end());
        removedQueued = m_queue.size() != before;
        if (Text::PathEquals(m_activePath, audioPath)) {
            m_cancel.store(true);
        }
    }

    if (removedQueued) {
        UpdateSidecarStatus(audioPath, TranscriptStatus::Cancelled, "Cancelled", {}, {});
        Notify(WM_APP_TRANSCRIBE_FAILED, audioPath, 0, L"Cancelled");
    }
}

bool TranscriptionService::IsActivePath(const std::wstring& audioPath) const {
    std::lock_guard lock(m_mutex);
    return Text::PathEquals(m_activePath, audioPath);
}

bool TranscriptionService::IsQueuedOrActive(const std::wstring& audioPath) const {
    std::lock_guard lock(m_mutex);
    if (Text::PathEquals(m_activePath, audioPath)) {
        return true;
    }
    for (const auto& job : m_queue) {
        if (Text::PathEquals(job.audioPath, audioPath)) {
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

void TranscriptionService::Notify(UINT msg, const std::wstring& audioPath, int percent, std::wstring message) {
    auto notify = std::make_unique<TranscribeNotify>();
    notify->audioPath = audioPath;
    notify->message = std::move(message);
    notify->percent = percent;
    PostOwned(m_notifyHwnd, msg, std::move(notify));
}

void TranscriptionService::FailJob(const Job& job, TranscriptStatus status, const std::wstring& message) {
    UpdateSidecarStatus(job.audioPath, status, Text::WideToUtf8(message), {}, {});
    Notify(WM_APP_TRANSCRIBE_FAILED, job.audioPath, 0, message);
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
        side = RecordingSidecar::ForAudio(audioPath);
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
    ScopedComInit com(COINIT_MULTITHREADED);

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
}

bool TranscriptionService::TranscribeChunks(
    const Job& job,
    const DecodedAudio& decoded,
    std::vector<TranscriptSegment>& segments,
    std::string& language) {
    const size_t totalFrames48 = decoded.interleavedStereo.size() / 2;
    const auto chunks = ChunkPlan::Plan(totalFrames48);
    const size_t chunkCount = (std::max)(size_t{1}, chunks.size());
    const bool chunked = chunks.size() > 1;

    UpdateSidecarStatus(
        job.audioPath,
        TranscriptStatus::Transcribing,
        {},
        Text::WideToUtf8(job.modelFileName),
        job.language);

    const auto wallStart = std::chrono::steady_clock::now();

    for (size_t chunkIndex = 0; chunkIndex < chunks.size(); ++chunkIndex) {
        if (m_cancel.load()) {
            FailJob(job, TranscriptStatus::Cancelled, L"Cancelled");
            return false;
        }

        const size_t frameOffset = chunks[chunkIndex].first;
        const size_t frames = chunks[chunkIndex].second;
        const float* stereo = decoded.interleavedStereo.data() + frameOffset * 2;
        auto mono16k = Resampler::Stereo48kToMono16k(stereo, frames);

        const int basePercent = chunked
            ? static_cast<int>((chunkIndex * 100) / chunkCount)
            : 0;
        const int span = chunked ? static_cast<int>(100 / chunkCount) : 100;

        std::vector<TranscriptSegment> chunkSegments;
        std::string chunkLang;
        std::wstring error;
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
                Notify(WM_APP_TRANSCRIBE_PROGRESS, job.audioPath, mapped);
            },
            chunkSegments,
            chunkLang,
            error);

        if (!ok) {
            const bool cancelled = m_cancel.load() || error == L"Cancelled.";
            FailJob(job, cancelled ? TranscriptStatus::Cancelled : TranscriptStatus::Failed, error);
            return false;
        }

        if (language.empty()) {
            language = chunkLang;
        }

        const int64_t offsetTenths = ChunkPlan::OffsetTenths(frameOffset);
        for (auto& seg : chunkSegments) {
            seg.t0 += offsetTenths;
            seg.t1 += offsetTenths;
            segments.push_back(std::move(seg));
        }
    }

    const double wallSeconds = std::chrono::duration<double>(
        std::chrono::steady_clock::now() - wallStart).count();
    const double audioSeconds =
        decoded.durationSeconds > 0.0 ? decoded.durationSeconds : 1.0;
    const double rtf = wallSeconds / audioSeconds;
    std::wostringstream rtfLine;
    rtfLine.setf(std::ios::fixed);
    rtfLine.precision(3);
    rtfLine << L"Transcription RTF " << rtf
            << L" (wall " << wallSeconds << L"s / audio " << audioSeconds << L"s"
            << L", model " << job.modelFileName << L")";
    Log::Info(rtfLine.str());
    return true;
}

void TranscriptionService::ProcessJob(Job job) {
    UpdateSidecarStatus(
        job.audioPath,
        TranscriptStatus::Preparing,
        {},
        Text::WideToUtf8(job.modelFileName),
        job.language);
    Notify(WM_APP_TRANSCRIBE_PROGRESS, job.audioPath, 0);

    std::wstring error;
    if (!m_engine.EnsureLoaded(job.modelPath, error)) {
        FailJob(job, TranscriptStatus::Failed, error);
        return;
    }

    DecodedAudio decoded{};
    if (!AudioFileReader::Read(job.audioPath, decoded, error)) {
        FailJob(job, TranscriptStatus::Failed, error);
        return;
    }

    if (m_cancel.load()) {
        FailJob(job, TranscriptStatus::Cancelled, L"Cancelled");
        return;
    }

    std::vector<TranscriptSegment> allSegments;
    std::string detectedLanguage;
    if (!TranscribeChunks(job, decoded, allSegments, detectedLanguage)) {
        return;
    }

    RecordingSidecar side{};
    Sidecar::Load(RecordingLibrary::JsonPathForAudio(job.audioPath), side);

    TranscriptMeta meta{};
    meta.titleDateTime = TitleFromStartLocal(side.startLocal);
    meta.durationSeconds = side.durationSeconds > 0.0 ? side.durationSeconds : decoded.durationSeconds;
    meta.recordedRange = RecordedRange(side);
    meta.modelFile = Text::WideToUtf8(job.modelFileName);
    meta.languageConfigured = job.language;
    meta.languageDetected = detectedLanguage;

    const std::string markdown = MarkdownFormatter::Format(meta, allSegments);
    const std::wstring mdPath = RecordingLibrary::MdPathForAudio(job.audioPath);
    if (!MarkdownFormatter::WriteUtf8NoBom(mdPath, markdown)) {
        FailJob(job, TranscriptStatus::Failed, L"Failed to write markdown");
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
        Text::WideToUtf8(job.modelFileName),
        langOut);

    Notify(WM_APP_TRANSCRIBE_FINISHED, job.audioPath, 100);
    Log::Info(L"Transcript ready: " + mdPath);
}
