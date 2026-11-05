//! cpal/WASAPI output for a PCM16 WAV file.
//!
//! The playback service owns the command loop. This module only opens the
//! device, feeds samples, and reports when the stream finishes.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, SizedSample};

use crate::common::error::AppError;
use crate::infrastructure::files::reader::WavReader;

/// Live output stream. Dropping it stops device playback.
pub(crate) struct OutputSession {
    _stream: cpal::Stream,
    finished: Arc<AtomicBool>,
}

impl OutputSession {
    pub(crate) fn finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }
}

/// Open the WAV, negotiate a matching output format, and start streaming.
pub(crate) fn start_wav_playback(path: &Path) -> Result<OutputSession, AppError> {
    let reader = WavReader::open(path)?;
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

    Ok(OutputSession {
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
