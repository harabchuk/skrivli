//! Background PCM16 WAV player via cpal/WASAPI.
//!
//! Selection changes and Stop command halt playback. No file I/O runs on the
//! GPUI thread — the host thread owns the reader and output stream.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, SizedSample};

use crate::common::error::AppError;
use crate::domain::ids::RecordingId;
use crate::infrastructure::files::reader::WavReader;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PlayerPhase {
    Idle,
    Playing,
    Error,
}

impl PlayerPhase {
    #[cfg(test)]
    fn from_raw(raw: u8) -> Self {
        match raw {
            1 => Self::Playing,
            2 => Self::Error,
            _ => Self::Idle,
        }
    }

    fn as_raw(self) -> u8 {
        match self {
            Self::Idle => 0,
            Self::Playing => 1,
            Self::Error => 2,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PlayerSnapshot {
    pub(crate) phase: PlayerPhase,
    pub(crate) recording_id: Option<RecordingId>,
    pub(crate) notice: Option<&'static str>,
}

enum PlayerCommand {
    Play { id: RecordingId, path: PathBuf },
    Stop,
    Shutdown,
}

/// Owns a dedicated player worker. Commands are non-blocking for the UI.
#[derive(Clone)]
pub(crate) struct PlayerHost {
    tx: Sender<PlayerCommand>,
    /// Mirrored for tests via [`Self::snapshot`]; UI uses bridge events.
    #[allow(dead_code)]
    phase: Arc<AtomicU8>,
    #[allow(dead_code)]
    playing_id: Arc<Mutex<Option<RecordingId>>>,
    #[allow(dead_code)]
    notice: Arc<Mutex<Option<&'static str>>>,
}

impl PlayerHost {
    pub(crate) fn spawn(on_change: Box<dyn Fn(PlayerSnapshot) + Send>) -> (Self, JoinHandle<()>) {
        let (tx, rx) = mpsc::channel();
        let phase = Arc::new(AtomicU8::new(PlayerPhase::Idle.as_raw()));
        let playing_id = Arc::new(Mutex::new(None));
        let notice = Arc::new(Mutex::new(None));
        let worker_phase = Arc::clone(&phase);
        let worker_id = Arc::clone(&playing_id);
        let worker_notice = Arc::clone(&notice);
        let handle = thread::spawn(move || {
            player_loop(rx, worker_phase, worker_id, worker_notice, on_change);
        });
        (
            Self {
                tx,
                phase,
                playing_id,
                notice,
            },
            handle,
        )
    }

    pub(crate) fn play(&self, id: RecordingId, path: PathBuf) {
        let _ = self.tx.send(PlayerCommand::Play { id, path });
    }

    pub(crate) fn stop(&self) {
        let _ = self.tx.send(PlayerCommand::Stop);
    }

    pub(crate) fn shutdown(&self) {
        let _ = self.tx.send(PlayerCommand::Shutdown);
    }

    #[cfg(test)]
    pub(crate) fn snapshot(&self) -> PlayerSnapshot {
        let phase = PlayerPhase::from_raw(self.phase.load(Ordering::Acquire));
        let recording_id = *self
            .playing_id
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let notice = *self
            .notice
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        PlayerSnapshot {
            phase,
            recording_id,
            notice,
        }
    }
}

fn player_loop(
    rx: mpsc::Receiver<PlayerCommand>,
    phase: Arc<AtomicU8>,
    playing_id: Arc<Mutex<Option<RecordingId>>>,
    notice: Arc<Mutex<Option<&'static str>>>,
    on_change: Box<dyn Fn(PlayerSnapshot) + Send>,
) {
    let mut active: Option<ActivePlayback> = None;
    while let Ok(command) = rx.recv() {
        match command {
            PlayerCommand::Shutdown => {
                drop(active.take());
                publish(
                    &phase,
                    &playing_id,
                    &notice,
                    PlayerPhase::Idle,
                    None,
                    None,
                    &on_change,
                );
                break;
            }
            PlayerCommand::Stop => {
                drop(active.take());
                publish(
                    &phase,
                    &playing_id,
                    &notice,
                    PlayerPhase::Idle,
                    None,
                    None,
                    &on_change,
                );
            }
            PlayerCommand::Play { id, path } => {
                drop(active.take());
                match start_playback(id, path, &phase, &playing_id, &notice, &on_change) {
                    Ok(playback) => active = Some(playback),
                    Err(error) => {
                        tracing::error!(error = %error, "playback failed to start");
                        publish(
                            &phase,
                            &playing_id,
                            &notice,
                            PlayerPhase::Error,
                            None,
                            Some(error.user_message()),
                            &on_change,
                        );
                    }
                }
            }
        }
        if active
            .as_ref()
            .is_some_and(|playback| playback.finished.load(Ordering::Acquire))
        {
            drop(active.take());
            publish(
                &phase,
                &playing_id,
                &notice,
                PlayerPhase::Idle,
                None,
                None,
                &on_change,
            );
        }
    }
}

struct ActivePlayback {
    _stream: cpal::Stream,
    finished: Arc<AtomicBool>,
}

fn start_playback(
    id: RecordingId,
    path: PathBuf,
    phase: &Arc<AtomicU8>,
    playing_id: &Arc<Mutex<Option<RecordingId>>>,
    notice: &Arc<Mutex<Option<&'static str>>>,
    on_change: &dyn Fn(PlayerSnapshot),
) -> Result<ActivePlayback, AppError> {
    let reader = WavReader::open(&path)?;
    let format = reader.format();
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| AppError::message("playback", "no output device"))?;
    let supported = device
        .default_output_config()
        .map_err(|error| AppError::message("playback", error.to_string()))?;
    let sample_format = supported.sample_format();
    let mut config: cpal::StreamConfig = supported.config();
    config.channels = format.channels();
    config.sample_rate = cpal::SampleRate(format.sample_rate());

    let finished = Arc::new(AtomicBool::new(false));
    let done = Arc::clone(&finished);
    let channels = usize::from(format.channels());
    let shared = Arc::new(Mutex::new(reader));

    let stream = match sample_format {
        SampleFormat::F32 => build_stream::<f32>(&device, &config, shared, channels, done)?,
        SampleFormat::I16 => build_stream::<i16>(&device, &config, shared, channels, done)?,
        other => {
            return Err(AppError::message(
                "playback",
                format!("unsupported output format {other}"),
            ));
        }
    };
    stream
        .play()
        .map_err(|error| AppError::message("playback", error.to_string()))?;

    publish(
        phase,
        playing_id,
        notice,
        PlayerPhase::Playing,
        Some(id),
        None,
        on_change,
    );
    Ok(ActivePlayback {
        _stream: stream,
        finished,
    })
}

fn build_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    reader: Arc<Mutex<WavReader>>,
    channels: usize,
    finished: Arc<AtomicBool>,
) -> Result<cpal::Stream, AppError>
where
    T: SizedSample + FromSample,
{
    let err_finished = Arc::clone(&finished);
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _| {
                if finished.load(Ordering::Acquire) {
                    silence(data);
                    return;
                }
                let frames = data.len() / channels.max(1);
                let mut pcm = vec![0_i16; frames * channels.max(1)];
                let read = {
                    let mut guard = match reader.lock() {
                        Ok(guard) => guard,
                        Err(_) => {
                            finished.store(true, Ordering::Release);
                            silence(data);
                            return;
                        }
                    };
                    match guard.read_i16_frames(&mut pcm, frames) {
                        Ok(read) => read,
                        Err(error) => {
                            tracing::error!(error = %error, "playback read failed");
                            finished.store(true, Ordering::Release);
                            0
                        }
                    }
                };
                if read == 0 {
                    finished.store(true, Ordering::Release);
                    silence(data);
                    return;
                }
                for (index, sample) in data.iter_mut().enumerate() {
                    let pcm_index = index;
                    if pcm_index / channels < read {
                        *sample = T::from_i16(pcm[pcm_index]);
                    } else {
                        *sample = T::equilibrium();
                    }
                }
            },
            move |error| {
                tracing::error!(error = %error, "playback stream error");
                err_finished.store(true, Ordering::Release);
            },
            None,
        )
        .map_err(|error| AppError::message("playback", error.to_string()))
}

trait FromSample {
    fn from_i16(sample: i16) -> Self;
    fn equilibrium() -> Self;
}

impl FromSample for f32 {
    fn from_i16(sample: i16) -> Self {
        f32::from(sample) / 32768.0
    }

    fn equilibrium() -> Self {
        0.0
    }
}

impl FromSample for i16 {
    fn from_i16(sample: i16) -> Self {
        sample
    }

    fn equilibrium() -> Self {
        0
    }
}

fn silence<T: FromSample>(data: &mut [T]) {
    for sample in data.iter_mut() {
        *sample = T::equilibrium();
    }
}

fn publish(
    phase: &Arc<AtomicU8>,
    playing_id: &Arc<Mutex<Option<RecordingId>>>,
    notice: &Arc<Mutex<Option<&'static str>>>,
    next: PlayerPhase,
    id: Option<RecordingId>,
    message: Option<&'static str>,
    on_change: &dyn Fn(PlayerSnapshot),
) {
    phase.store(next.as_raw(), Ordering::Release);
    if let Ok(mut guard) = playing_id.lock() {
        *guard = id;
    }
    if let Ok(mut guard) = notice.lock() {
        *guard = message;
    }
    on_change(PlayerSnapshot {
        phase: next,
        recording_id: id,
        notice: message,
    });
}

#[cfg(test)]
mod tests {
    use super::{PlayerHost, PlayerPhase};
    use crate::domain::ids::RecordingId;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[test]
    fn stop_returns_to_idle_without_device() {
        // Arrange
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&events);
        let (host, handle) = PlayerHost::spawn(Box::new(move |snapshot| {
            sink.lock().expect("lock").push(snapshot.phase);
        }));

        // Act
        host.play(RecordingId::from_raw(1), PathBuf::from("missing.wav"));
        std::thread::sleep(Duration::from_millis(200));
        host.stop();
        std::thread::sleep(Duration::from_millis(50));
        host.shutdown();
        let _ = handle.join();
        let snapshot = host.snapshot();

        // Assert
        assert_eq!(snapshot.phase, PlayerPhase::Idle);
        let phases = events.lock().expect("lock");
        assert!(
            phases.contains(&PlayerPhase::Error) || phases.contains(&PlayerPhase::Idle),
            "phases={phases:?}"
        );
    }
}
