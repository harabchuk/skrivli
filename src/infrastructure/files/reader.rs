//! Streaming WAV reader for library playback (PCM16).
//!
//! Runs off the GPUI thread. Does not log sample values.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::common::error::AppError;
use crate::domain::audio::{AudioFormat, SampleEncoding};
use crate::infrastructure::files::wav::WAV_HEADER_BYTES;

/// Opened PCM16 WAV ready for chunked reads.
pub(crate) struct WavReader {
    file: File,
    format: AudioFormat,
    data_bytes: u64,
    position: u64,
}

impl WavReader {
    pub(crate) fn open(path: &Path) -> Result<Self, AppError> {
        let mut file = File::open(path).map_err(|source| AppError::io("playback", source))?;
        let (format, data_start, data_bytes) = parse_wav_header(&mut file)?;
        file.seek(SeekFrom::Start(data_start))
            .map_err(|source| AppError::io("playback", source))?;
        let _ = data_start;
        Ok(Self {
            file,
            format,
            data_bytes,
            position: 0,
        })
    }

    pub(crate) fn format(&self) -> AudioFormat {
        self.format
    }

    #[cfg(test)]
    pub(crate) fn duration_ms(&self) -> Option<u64> {
        let frames = self.format.frames_from_bytes(self.data_bytes)?;
        Some(frames.saturating_mul(1_000) / u64::from(self.format.sample_rate()))
    }

    /// Read up to `max_frames` interleaved i16 frames into `out` (must hold
    /// `max_frames * channels` samples). Returns frames read; `0` at EOF.
    pub(crate) fn read_i16_frames(
        &mut self,
        out: &mut [i16],
        max_frames: usize,
    ) -> Result<usize, AppError> {
        let channels = usize::from(self.format.channels());
        if channels == 0 {
            return Err(AppError::message("playback", "wav has no channels"));
        }
        let capacity_frames = out.len() / channels;
        let frames = max_frames.min(capacity_frames);
        if frames == 0 {
            return Ok(0);
        }
        let want = frames
            .checked_mul(channels)
            .and_then(|samples| samples.checked_mul(2))
            .ok_or_else(|| AppError::message("playback", "frame size overflow"))?
            as u64;
        let remaining = self.data_bytes.saturating_sub(self.position);
        let take = want.min(remaining);
        if take == 0 {
            return Ok(0);
        }
        if !take.is_multiple_of(2) {
            return Err(AppError::message("playback", "truncated pcm length"));
        }
        let mut bytes = vec![0_u8; take as usize];
        self.file
            .read_exact(&mut bytes)
            .map_err(|source| AppError::io("playback", source))?;
        let samples = take as usize / 2;
        for (index, chunk) in bytes.as_chunks::<2>().0.iter().enumerate() {
            out[index] = i16::from_le_bytes(*chunk);
        }
        self.position = self.position.saturating_add(take);
        Ok(samples / channels)
    }
}

fn parse_wav_header(file: &mut File) -> Result<(AudioFormat, u64, u64), AppError> {
    let mut header = [0_u8; WAV_HEADER_BYTES as usize];
    file.read_exact(&mut header)
        .map_err(|source| AppError::io("playback", source))?;
    if &header[0..4] != b"RIFF" || &header[8..12] != b"WAVE" {
        return Err(AppError::message("playback", "not a RIFF WAVE file"));
    }
    if &header[12..16] != b"fmt " {
        return Err(AppError::message("playback", "missing fmt chunk"));
    }
    let audio_format = u16::from_le_bytes([header[20], header[21]]);
    if audio_format != 1 {
        return Err(AppError::message("playback", "only PCM WAV is supported"));
    }
    let channels = u16::from_le_bytes([header[22], header[23]]);
    let sample_rate = u32::from_le_bytes([header[24], header[25], header[26], header[27]]);
    let bits = u16::from_le_bytes([header[34], header[35]]);
    if bits != 16 {
        return Err(AppError::message(
            "playback",
            "only 16-bit PCM is supported",
        ));
    }
    if &header[36..40] != b"data" {
        return Err(AppError::message(
            "playback",
            "canonical 44-byte WAV expected",
        ));
    }
    let data_bytes = u32::from_le_bytes([header[40], header[41], header[42], header[43]]) as u64;
    let format = AudioFormat::new(sample_rate, channels, SampleEncoding::I16)
        .ok_or_else(|| AppError::message("playback", "invalid wav format"))?;
    Ok((format, WAV_HEADER_BYTES, data_bytes))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::WavReader;
    use crate::domain::audio::CivilTime;
    use crate::infrastructure::files::wav::{quantize_sample, WavWriter};

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("skrivli-reader-{tag}-{id}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("dir");
        dir
    }

    #[test]
    fn reader_streams_pcm16_from_writer_output() {
        // Arrange
        let dir = temp_dir("pcm");
        let when = CivilTime::new(2026, 11, 4, 12, 0, 0).expect("time");
        let mut writer = WavWriter::create(&dir, when).expect("create");
        let mut frames = Vec::new();
        for _ in 0..480 {
            frames.push(0.5);
            frames.push(-0.5);
        }
        writer.write_samples(&frames).expect("write");
        let path = writer.finalize().expect("finalize");

        // Act
        let mut reader = WavReader::open(&path).expect("open");
        let mut buffer = vec![0_i16; 960];
        let read = reader.read_i16_frames(&mut buffer, 480).expect("read");

        // Assert
        assert_eq!(read, 480);
        assert_eq!(buffer[0], quantize_sample(0.5));
        assert_eq!(buffer[1], quantize_sample(-0.5));
        assert_eq!(reader.duration_ms(), Some(10));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn reader_rejects_non_wav() {
        // Arrange
        let dir = temp_dir("bad");
        let path = dir.join("nope.wav");
        let mut file = fs::File::create(&path).expect("create");
        file.write_all(b"not a wav").expect("write");

        // Act
        let result = WavReader::open(&path);

        // Assert
        assert!(result.is_err());
        let _ = fs::remove_dir_all(dir);
    }
}
