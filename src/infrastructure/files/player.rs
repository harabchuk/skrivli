//! Rodio output for a FLAC file, behind the [`PlaybackOutput`] port.
//!
//! The playback service owns the command loop. This module opens the default
//! output device, wraps a `flac-codec` decoder as a seekable Rodio [`Source`]
//! and exposes Rodio's sink.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::time::Duration;

use flac_codec::decode::FlacSampleReader;
use flac_codec::metadata::Metadata;
use rodio::source::SeekError;
use rodio::{OutputStream, OutputStreamBuilder, Sink, Source};

use crate::common::error::AppError;
use crate::infrastructure::files::flac::map_flac;
use crate::services::ports::{PlaybackOutput, PlaybackSession};

/// Plays through the default Windows output device.
pub(crate) struct RodioOutput;

impl PlaybackOutput for RodioOutput {
    fn open(&self, path: &Path, start_ms: u64) -> Result<Box<dyn PlaybackSession>, AppError> {
        let source = FlacSource::open(path)?;
        let duration_ms = source
            .total_duration()
            .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX));
        let mut stream = OutputStreamBuilder::open_default_stream()
            .map_err(|error| AppError::message("playback", error.to_string()))?;
        stream.log_on_drop(false);
        let sink = Sink::connect_new(stream.mixer());
        sink.append(source);
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

/// Rodio [`Source`] over a finalized FLAC file. 16-bit PCM is scaled to `f32`.
pub(crate) struct FlacSource {
    reader: FlacSampleReader<BufReader<File>>,
    channels: u16,
    sample_rate: u32,
    total_frames: Option<u64>,
    block: Vec<f32>,
    cursor: usize,
}

impl FlacSource {
    pub(crate) fn open(path: &Path) -> Result<Self, AppError> {
        let file = File::open(path).map_err(|source| AppError::io("playback", source))?;
        let reader = FlacSampleReader::new_seekable(BufReader::new(file))
            .map_err(|error| map_flac("playback", error))?;
        Ok(Self {
            channels: u16::from(reader.channel_count()),
            sample_rate: reader.sample_rate(),
            total_frames: reader.total_samples(),
            reader,
            block: Vec::new(),
            cursor: 0,
        })
    }

    fn refill(&mut self) -> bool {
        let bits = self.reader.bits_per_sample().clamp(1, 24);
        let scale = (1_u32 << (bits - 1)) as f32;
        let decoded = match self.reader.fill_buf() {
            Ok(decoded) if !decoded.is_empty() => decoded,
            _ => return false,
        };
        let count = decoded.len();
        self.block.clear();
        self.block
            .extend(decoded.iter().map(|sample| *sample as f32 / scale));
        self.reader.consume(count);
        self.cursor = 0;
        true
    }
}

impl Iterator for FlacSource {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if self.cursor >= self.block.len() && !self.refill() {
            return None;
        }
        let sample = self.block[self.cursor];
        self.cursor += 1;
        Some(sample)
    }
}

impl Source for FlacSource {
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> u16 {
        self.channels
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn total_duration(&self) -> Option<Duration> {
        let frames = self.total_frames?;
        if self.sample_rate == 0 {
            return None;
        }
        Some(Duration::from_secs_f64(
            frames as f64 / f64::from(self.sample_rate),
        ))
    }

    fn try_seek(&mut self, position: Duration) -> Result<(), SeekError> {
        let wanted = (position.as_secs_f64() * f64::from(self.sample_rate)) as u64;
        let frame = self.total_frames.map_or(wanted, |total| wanted.min(total));
        self.reader
            .seek(frame)
            .map_err(|error| SeekError::Other(Box::new(error)))?;
        self.block.clear();
        self.cursor = 0;
        Ok(())
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::Duration;

    use rodio::Source;

    use super::FlacSource;
    use crate::domain::audio::CivilTime;
    use crate::infrastructure::files::flac::FlacWriter;

    fn ramp_file(label: &str, frames: usize) -> (std::path::PathBuf, std::path::PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("skrivli-player-{label}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("dir");
        let when = CivilTime::new(2026, 11, 6, 9, 0, 0).expect("time");
        let mut writer = FlacWriter::create(&dir, when).expect("create");
        let mut samples = Vec::with_capacity(frames * 2);
        for index in 0..frames {
            let value = (index % 30_000) as f32 / 32_767.0;
            samples.push(value);
            samples.push(value);
        }
        writer.write_samples(&samples).expect("write");
        (dir, writer.finalize().expect("finalize"))
    }

    #[test]
    fn source_reports_format_and_duration_and_decodes_samples() {
        // Arrange
        let (dir, path) = ramp_file("meta", 96_000);

        // Act
        let mut source = FlacSource::open(&path).expect("open");
        let first: Vec<f32> = source.by_ref().take(4).collect();

        // Assert
        assert_eq!(source.channels(), 2);
        assert_eq!(source.sample_rate(), 48_000);
        assert_eq!(source.total_duration(), Some(Duration::from_secs(2)));
        assert_eq!(first, vec![0.0, 0.0, 1.0 / 32_768.0, 1.0 / 32_768.0]);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn try_seek_jumps_to_the_requested_frame() {
        // Arrange
        let (dir, path) = ramp_file("seek", 144_000);
        let mut source = FlacSource::open(&path).expect("open");
        let _ = source.by_ref().take(20_000).count();

        // Act
        source.try_seek(Duration::from_secs(1)).expect("seek");
        let after: Vec<f32> = source.by_ref().take(2).collect();
        source.try_seek(Duration::from_millis(500)).expect("back");
        let back: Vec<f32> = source.by_ref().take(2).collect();

        // Assert
        assert_eq!(after, vec![18_000.0 / 32_768.0; 2]);
        assert_eq!(back, vec![24_000.0 / 32_768.0; 2]);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn seeking_past_the_end_stops_the_source() {
        // Arrange
        let (dir, path) = ramp_file("end", 48_000);
        let mut source = FlacSource::open(&path).expect("open");

        // Act
        source.try_seek(Duration::from_secs(60)).expect("seek");
        let next = source.next();

        // Assert
        assert_eq!(next, None);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn open_refuses_data_that_is_not_flac() {
        // Arrange
        let dir = std::env::temp_dir().join(format!("skrivli-player-junk-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("junk.flac");
        fs::write(&path, b"OggS....junk").expect("junk");

        // Act
        let result = FlacSource::open(&path);

        // Assert
        assert!(result.is_err());
        let _ = fs::remove_dir_all(dir);
    }
}
