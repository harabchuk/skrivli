#pragma once

#include "infrastructure/audio/MiniAudio.hpp"

#include <windows.h>

#include <atomic>
#include <string>

// Plays a recording with miniaudio (ma_decoder + ma_device). Create on the UI
// thread; play-state changes post notifyMsg with wParam 1/0 (lParam 0). End of
// file posts wParam 0 with lParam != 0 so the owner can call StopFromEof() and
// release the device (do not uninit from the data callback).
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
    // UI-thread handler for an EOF notify (WM_APP_PLAYBACK with lParam != 0).
    void StopFromEof();
    bool IsPlaying() const {
        return m_playing;
    }
    auto Path() const -> const std::wstring& {
        return m_path;
    }

  private:
    static void DataCallback(ma_device* device, void* output, const void* input,
                             ma_uint32 frameCount);
    void SetPlaying(bool playing);
    void UninitDeviceAndDecoder();

    HWND m_notifyHwnd = nullptr;
    UINT m_notifyMsg = 0;
    std::wstring m_path;
    bool m_playing = false;
    bool m_decoderInit = false;
    bool m_deviceInit = false;
    std::atomic<bool> m_eofPosted{false};
    ma_decoder m_decoder{};
    ma_device m_device{};
};
