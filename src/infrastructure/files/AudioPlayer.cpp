#include "infrastructure/files/AudioPlayer.hpp"

#include "infrastructure/common/Log.hpp"

AudioPlayer::~AudioPlayer() {
    Shutdown();
}

bool AudioPlayer::Initialize(HWND notifyHwnd, UINT notifyMsg) {
    Shutdown();
    m_notifyHwnd = notifyHwnd;
    m_notifyMsg = notifyMsg;
    return notifyHwnd != nullptr && notifyMsg != 0;
}

void AudioPlayer::Shutdown() {
    Stop();
    m_notifyHwnd = nullptr;
    m_notifyMsg = 0;
}

bool AudioPlayer::Play(const std::wstring& path) {
    if (path.empty() || m_notifyHwnd == nullptr || m_notifyMsg == 0) {
        return false;
    }

    Stop();
    m_path = path;
    m_eofPosted.store(false, std::memory_order_relaxed);

    ma_decoder_config decoderConfig = ma_decoder_config_init(ma_format_f32, 2, 0);
    decoderConfig.ditherMode = ma_dither_mode_none;
    decoderConfig.resampling.linear.lpfOrder = 8;

    if (ma_decoder_init_file_w(path.c_str(), &decoderConfig, &m_decoder) != MA_SUCCESS) {
        Log::Warn(L"ma_decoder_init_file_w failed for " + path);
        m_path.clear();
        return false;
    }
    m_decoderInit = true;

    ma_device_config deviceConfig = ma_device_config_init(ma_device_type_playback);
    deviceConfig.playback.format = m_decoder.outputFormat;
    deviceConfig.playback.channels = m_decoder.outputChannels;
    deviceConfig.sampleRate = m_decoder.outputSampleRate;
    deviceConfig.dataCallback = DataCallback;
    deviceConfig.pUserData = this;

    if (ma_device_init(nullptr, &deviceConfig, &m_device) != MA_SUCCESS) {
        Log::Error(L"ma_device_init failed for playback");
        UninitDeviceAndDecoder();
        m_path.clear();
        return false;
    }
    m_deviceInit = true;

    if (ma_device_start(&m_device) != MA_SUCCESS) {
        Log::Error(L"ma_device_start failed for playback");
        UninitDeviceAndDecoder();
        m_path.clear();
        return false;
    }

    SetPlaying(true);
    return true;
}

void AudioPlayer::Stop() {
    UninitDeviceAndDecoder();
    m_path.clear();
    SetPlaying(false);
}

void AudioPlayer::StopFromEof() {
    // Called on the UI thread in response to an EOF notify (lParam != 0). Do not
    // post again — the message that got us here already carries wParam 0.
    UninitDeviceAndDecoder();
    m_path.clear();
    m_playing = false;
}

void AudioPlayer::SetPlaying(bool playing) {
    if (m_playing == playing) {
        return;
    }
    m_playing = playing;
    if (m_notifyHwnd != nullptr && ::IsWindow(m_notifyHwnd) && m_notifyMsg != 0) {
        // lParam 0 = UI state only. EOF uses lParam 1 so the view can release the device
        // without racing a subsequent Play() that already posted a stop state message.
        ::PostMessageW(m_notifyHwnd, m_notifyMsg, playing ? 1u : 0u, 0);
    }
}

void AudioPlayer::UninitDeviceAndDecoder() {
    if (m_deviceInit) {
        ma_device_uninit(&m_device);
        m_device = {};
        m_deviceInit = false;
    }
    if (m_decoderInit) {
        ma_decoder_uninit(&m_decoder);
        m_decoder = {};
        m_decoderInit = false;
    }
    m_eofPosted.store(false, std::memory_order_relaxed);
}

void AudioPlayer::DataCallback(ma_device* device, void* output, const void* input,
                               ma_uint32 frameCount) {
    (void)input;
    auto* self = static_cast<AudioPlayer*>(device->pUserData);
    if (self == nullptr || output == nullptr || frameCount == 0 || !self->m_decoderInit) {
        return;
    }

    ma_uint64 framesRead = 0;
    ma_decoder_read_pcm_frames(&self->m_decoder, output, frameCount, &framesRead);
    if (framesRead >= frameCount) {
        return;
    }

    const ma_format format = device->playback.format;
    const ma_uint32 channels = device->playback.channels;
    void* silence = ma_offset_pcm_frames_ptr(output, framesRead, format, channels);
    ma_silence_pcm_frames(silence, frameCount - static_cast<ma_uint32>(framesRead), format,
                          channels);

    bool expected = false;
    if (self->m_eofPosted.compare_exchange_strong(expected, true, std::memory_order_acq_rel)) {
        if (self->m_notifyHwnd != nullptr && ::IsWindow(self->m_notifyHwnd) &&
            self->m_notifyMsg != 0) {
            ::PostMessageW(self->m_notifyHwnd, self->m_notifyMsg, 0, 1);
        }
    }
}
