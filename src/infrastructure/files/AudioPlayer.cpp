#include "infrastructure/files/AudioPlayer.hpp"

#include "infrastructure/common/Log.hpp"
#include "infrastructure/common/Scoped.hpp"

class AudioPlayer::Callback : public IMFPMediaPlayerCallback {
  public:
    explicit Callback(AudioPlayer* owner) : m_owner(owner) {}

    // IMFPMediaPlayerCallback
    void STDMETHODCALLTYPE OnMediaPlayerEvent(MFP_EVENT_HEADER* header) override {
        if (m_owner != nullptr && header != nullptr) {
            m_owner->OnPlayerEvent(header);
        }
    }

    // IUnknown
    HRESULT STDMETHODCALLTYPE QueryInterface(REFIID riid, void** object) override {
        if (object == nullptr) {
            return E_POINTER;
        }
        if (riid == IID_IUnknown || riid == __uuidof(IMFPMediaPlayerCallback)) {
            *object = static_cast<IMFPMediaPlayerCallback*>(this);
            AddRef();
            return S_OK;
        }
        *object = nullptr;
        return E_NOINTERFACE;
    }

    ULONG STDMETHODCALLTYPE AddRef() override {
        return static_cast<ULONG>(::InterlockedIncrement(&m_ref));
    }

    ULONG STDMETHODCALLTYPE Release() override {
        const auto ref = static_cast<ULONG>(::InterlockedDecrement(&m_ref));
        if (ref == 0) {
            delete this;
        }
        return ref;
    }

    void Detach() {
        m_owner = nullptr;
    }

  private:
    AudioPlayer* m_owner = nullptr;
    long m_ref = 1;
};

AudioPlayer::~AudioPlayer() {
    Shutdown();
}

bool AudioPlayer::Initialize(HWND notifyHwnd, UINT notifyMsg) {
    Shutdown();
    m_notifyHwnd = notifyHwnd;
    m_notifyMsg = notifyMsg;

    if (!m_mf) {
        m_mf = std::make_unique<ScopedMfStartup>(MFSTARTUP_LITE);
    }
    if (!m_mf->IsStarted()) {
        Log::Error(L"MFStartup failed for AudioPlayer");
        m_mf.reset();
        return false;
    }

    auto* callback = new Callback(this);
    m_callback.attach(callback);

    const HRESULT hr =
        ::MFPCreateMediaPlayer(nullptr, FALSE, 0, m_callback.get(), nullptr, m_player.put());
    if (FAILED(hr) || !m_player) {
        Log::Error(L"MFPCreateMediaPlayer failed");
        Shutdown();
        return false;
    }
    return true;
}

void AudioPlayer::Shutdown() {
    if (m_callback) {
        static_cast<Callback*>(m_callback.get())->Detach();
    }
    if (m_player) {
        m_player->Shutdown();
        m_player.reset();
    }
    m_callback.reset();
    m_path.clear();
    m_playing = false;
    m_mf.reset();
    m_notifyHwnd = nullptr;
    m_notifyMsg = 0;
}

bool AudioPlayer::Play(const std::wstring& path) {
    if (!m_player || path.empty()) {
        return false;
    }

    Stop();
    m_path = path;

    // TRUE = create asynchronously; MEDIAITEM_CREATED arrives on the callback.
    const HRESULT hr = m_player->CreateMediaItemFromURL(path.c_str(), TRUE, 0, nullptr);
    if (FAILED(hr)) {
        Log::Warn(L"CreateMediaItemFromURL failed for " + path);
        m_path.clear();
        return false;
    }
    return true;
}

void AudioPlayer::Stop() {
    if (m_player) {
        m_player->Stop();
    }
    SetPlaying(false);
}

void AudioPlayer::SetPlaying(bool playing) {
    if (m_playing == playing) {
        return;
    }
    m_playing = playing;
    if (m_notifyHwnd != nullptr && ::IsWindow(m_notifyHwnd) && m_notifyMsg != 0) {
        ::PostMessageW(m_notifyHwnd, m_notifyMsg, playing ? 1u : 0u, 0);
    }
}

void AudioPlayer::OnPlayerEvent(MFP_EVENT_HEADER* header) {
    if (header == nullptr) {
        return;
    }
    if (FAILED(header->hrEvent)) {
        Log::Warn(L"AudioPlayer event error");
        SetPlaying(false);
        return;
    }

    switch (header->eEventType) {
    case MFP_EVENT_TYPE_MEDIAITEM_CREATED: {
        const auto* created = MFP_GET_MEDIAITEM_CREATED_EVENT(header);
        if (created != nullptr && created->pMediaItem != nullptr && m_player) {
            m_player->SetMediaItem(created->pMediaItem);
        }
        break;
    }
    case MFP_EVENT_TYPE_MEDIAITEM_SET:
        if (m_player) {
            m_player->Play();
        }
        break;
    case MFP_EVENT_TYPE_PLAY:
        SetPlaying(true);
        break;
    case MFP_EVENT_TYPE_PAUSE:
    case MFP_EVENT_TYPE_STOP:
        SetPlaying(false);
        break;
    case MFP_EVENT_TYPE_PLAYBACK_ENDED:
        Stop();
        break;
    default:
        break;
    }
}
