#pragma once

#include "infrastructure/common/Scoped.hpp"

#include <windows.h>

#include <memory>
#include <mfplay.h>
#include <string>
#include <wil/com.h>

// Simple MFPlay wrapper. Create on the UI thread; events post WM_APP_PLAYBACK
// with wParam 1 while playing and 0 when stopped.
class AudioPlayer {
  public:
    AudioPlayer() = default;
    ~AudioPlayer();

    AudioPlayer(const AudioPlayer&) = delete;
    auto operator=(const AudioPlayer&) -> AudioPlayer& = delete;

    bool Initialize(HWND notifyHwnd, UINT notifyMsg);
    void Shutdown();

    bool Play(const std::wstring& path);
    void Stop();
    bool IsPlaying() const {
        return m_playing;
    }
    auto Path() const -> const std::wstring& {
        return m_path;
    }

  private:
    class Callback;

    void OnPlayerEvent(MFP_EVENT_HEADER* header);
    void SetPlaying(bool playing);

    std::unique_ptr<ScopedMfStartup> m_mf;
    wil::com_ptr_nothrow<IMFPMediaPlayer> m_player;
    wil::com_ptr_nothrow<IMFPMediaPlayerCallback> m_callback;
    HWND m_notifyHwnd = nullptr;
    UINT m_notifyMsg = 0;
    std::wstring m_path;
    bool m_playing = false;
};