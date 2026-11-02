#include "infrastructure/ai/WhisperEngine.hpp"

#include "infrastructure/common/Log.hpp"
#include "infrastructure/common/Text.hpp"

#include <algorithm>
#include <chrono>
#include <thread>
#include <whisper.h>

namespace {

uint64_t NowMs() {
    using clock = std::chrono::steady_clock;
    return static_cast<uint64_t>(
        std::chrono::duration_cast<std::chrono::milliseconds>(clock::now().time_since_epoch())
            .count());
}

struct CallbackState {
    std::atomic_bool* cancel = nullptr;
    WhisperEngine::ProgressFn progress;
};

bool AbortCallback(void* userData) {
    auto* state = static_cast<CallbackState*>(userData);
    return state != nullptr && state->cancel != nullptr && state->cancel->load();
}

void ProgressCallback(whisper_context* /*ctx*/, whisper_state* /*state*/, int progress,
                      void* userData) {
    auto* cb = static_cast<CallbackState*>(userData);
    if (cb != nullptr && cb->progress) {
        cb->progress(std::clamp(progress, 0, 100));
    }
}

} // namespace

WhisperEngine::~WhisperEngine() {
    Unload();
}

bool WhisperEngine::IsLoaded() const {
    std::lock_guard lock(m_mutex);
    return m_ctx != nullptr;
}

bool WhisperEngine::EnsureLoaded(const std::wstring& modelPath, std::wstring& error) {
    std::lock_guard lock(m_mutex);
    if (m_ctx != nullptr && m_modelPath == modelPath) {
        m_lastUseTick = NowMs();
        return true;
    }

    if (m_ctx != nullptr) {
        whisper_free(m_ctx);
        m_ctx = nullptr;
        m_modelPath.clear();
    }

    const auto utf8 = Text::WideToUtf8(modelPath);
    whisper_context_params cparams = whisper_context_default_params();
    m_ctx = whisper_init_from_file_with_params(utf8.c_str(), cparams);
    if (m_ctx == nullptr) {
        error = L"Failed to load whisper model.";
        Log::Error(L"whisper_init_from_file_with_params failed for " + modelPath);
        return false;
    }

    m_modelPath = modelPath;
    m_lastUseTick = NowMs();
    Log::Info(L"Loaded whisper model: " + modelPath);
    return true;
}

void WhisperEngine::Unload() {
    std::lock_guard lock(m_mutex);
    if (m_ctx != nullptr) {
        whisper_free(m_ctx);
        m_ctx = nullptr;
        m_modelPath.clear();
        Log::Info(L"Unloaded whisper model");
    }
}

void WhisperEngine::UnloadIfIdle(uint64_t idleMs) {
    std::lock_guard lock(m_mutex);
    if (m_ctx == nullptr) {
        return;
    }
    const uint64_t now = NowMs();
    if (now - m_lastUseTick >= idleMs) {
        whisper_free(m_ctx);
        m_ctx = nullptr;
        m_modelPath.clear();
        Log::Info(L"Unloaded idle whisper model");
    }
}

bool WhisperEngine::Transcribe(const std::vector<float>& pcm16kMono, const std::string& language,
                               int threads, bool useVad, const std::wstring& vadModelPath,
                               std::atomic_bool& cancelFlag, ProgressFn onProgress,
                               std::vector<TranscriptSegment>& outSegments,
                               std::string& outDetectedLanguage, std::wstring& error) {
    outSegments.clear();
    outDetectedLanguage.clear();

    if (pcm16kMono.empty()) {
        error = L"No audio samples to transcribe.";
        return false;
    }

    std::lock_guard lock(m_mutex);
    if (m_ctx == nullptr) {
        error = L"Whisper model is not loaded.";
        return false;
    }

    whisper_full_params params = whisper_full_default_params(WHISPER_SAMPLING_GREEDY);
    params.translate = false;
    params.print_special = false;
    params.print_progress = false;
    params.print_realtime = false;
    params.print_timestamps = false;
    params.language = language.empty() ? "auto" : language.c_str();

    const unsigned hw = std::max(1u, std::thread::hardware_concurrency());
    if (threads > 0) {
        params.n_threads = threads;
    } else {
        params.n_threads = static_cast<int>(std::max(1u, hw - 1));
    }

    CallbackState state{};
    state.cancel = &cancelFlag;
    state.progress = std::move(onProgress);
    params.progress_callback = ProgressCallback;
    params.progress_callback_user_data = &state;
    params.abort_callback = AbortCallback;
    params.abort_callback_user_data = &state;

    std::string vadUtf8;
    if (useVad) {
        if (!vadModelPath.empty()) {
            vadUtf8 = Text::WideToUtf8(vadModelPath);
            params.vad = true;
            params.vad_model_path = vadUtf8.c_str();
        } else {
            Log::Warn(L"UseVad is on but Silero VAD model is missing; continuing without VAD");
            params.vad = false;
        }
    }

    if (cancelFlag.load()) {
        error = L"Cancelled.";
        return false;
    }

    const int rc =
        whisper_full(m_ctx, params, pcm16kMono.data(), static_cast<int>(pcm16kMono.size()));
    m_lastUseTick = NowMs();

    if (cancelFlag.load()) {
        error = L"Cancelled.";
        return false;
    }
    if (rc != 0) {
        error = L"whisper_full failed.";
        Log::Error(L"whisper_full returned " + std::to_wstring(rc));
        return false;
    }

    const int n = whisper_full_n_segments(m_ctx);
    outSegments.reserve(static_cast<size_t>(n));
    for (int i = 0; i < n; ++i) {
        TranscriptSegment seg{};
        seg.t0 = whisper_full_get_segment_t0(m_ctx, i);
        seg.t1 = whisper_full_get_segment_t1(m_ctx, i);
        const char* text = whisper_full_get_segment_text(m_ctx, i);
        seg.text = text != nullptr ? text : "";
        outSegments.push_back(std::move(seg));
    }

    const int langId = whisper_full_lang_id(m_ctx);
    if (langId >= 0) {
        const char* lang = whisper_lang_str(langId);
        if (lang != nullptr) {
            outDetectedLanguage = lang;
        }
    }
    return true;
}
