//! Rodio output for a WAV file, behind the [`PlaybackOutput`] port.
//!
//! The playback service owns the command loop. This module only opens the
//! default output device, decodes the WAV and exposes Rodio's sink.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::time::Duration;

use rodio::{Decoder, OutputStream, OutputStreamBuilder, Sink, Source};

use crate::common::error::AppError;
use crate::services::ports::{PlaybackOutput, PlaybackSession};

/// Plays through the default Windows output device.
pub(crate) struct RodioOutput;

impl PlaybackOutput for RodioOutput {
    fn open(&self, path: &Path, start_ms: u64) -> Result<Box<dyn PlaybackSession>, AppError> {
        let file = File::open(path).map_err(|source| AppError::io("playback", source))?;
        let decoder = Decoder::new_wav(BufReader::new(file))
            .map_err(|error| AppError::message("playback", error.to_string()))?;
        let duration_ms = decoder
            .total_duration()
            .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX));
        let mut stream = OutputStreamBuilder::open_default_stream()
            .map_err(|error| AppError::message("playback", error.to_string()))?;
        stream.log_on_drop(false);
        let sink = Sink::connect_new(stream.mixer());
        sink.append(decoder);
        let session = RodioSession {
            sink,
            _stream: stream,
            duration_ms,
        };
        if start_ms > 0 {
            session.seek(start_ms)?;
        }
        Ok(Box::new(session))
    }
}

struct RodioSession {
    // Field order matters: the sink drops before the stream that feeds it.
    sink: Sink,
    _stream: OutputStream,
    duration_ms: Option<u64>,
}

impl PlaybackSession for RodioSession {
    fn position_ms(&self) -> u64 {
        u64::try_from(self.sink.get_pos().as_millis()).unwrap_or(u64::MAX)
    }

    fn duration_ms(&self) -> Option<u64> {
        self.duration_ms
    }

    fn finished(&self) -> bool {
        self.sink.empty()
    }

    fn set_paused(&self, paused: bool) {
        if paused {
            self.sink.pause();
        } else {
            self.sink.play();
        }
    }

    fn seek(&self, position_ms: u64) -> Result<(), AppError> {
        self.sink
            .try_seek(Duration::from_millis(position_ms))
            .map_err(|error| AppError::message("playback", error.to_string()))
    }
}
