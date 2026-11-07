//! Bounded PCM16 WAV reader for offline transcription.
//!
//! Separate from playback, which decodes through Rodio. Opens the finalized
//! path only — never `.wav.partial`. MP3 stays out of v1. FLAC decode will arrive
//! in M8 through `flac-codec`.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::common::error::AppError;
use crate::domain::audio::{AudioFormat, SampleEncoding};
use crate::infrastructure::files::wav::WAV_HEADER_BYTES;

/// Default read size: about one second of 48 kHz stereo PCM16.
pub(crate) const DEFAULT_CHUNK_FRAMES: usize = 48_000;

/// Opened finalized PCM16 WAV for chunked transcription reads.
#[derive(Debug)]
pub(crate) struct TranscriptWavReader {
    file: File,
    format: AudioFormat,
    data_bytes: u64,
    position: u64,
}

impl TranscriptWavReader {
    /// Open a finalized recording. Refuses `.wav.partial` and non-WAV containers.
    pub(crate) fn open(path: &Path) -> Result<Self, AppError> {
        refuse_unsupported_path(path)?;
        let mut file = File::open(path).map_err(|source| AppError::io("transcription", source))?;
        let (format, data_start, data_bytes) = parse_wav_header(&mut file)?;
        file.seek(SeekFrom::Start(data_start))
            .map_err(|source| AppError::io("transcription", source))?;
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

    #[allow(dead_code)] // Inspected by diagnostics / future FLAC path.
    pub(crate) fn data_bytes(&self) -> u64 {
        self.data_bytes
    }

    /// Frames remaining in the data chunk.
    pub(crate) fn frames_remaining(&self) -> Option<u64> {
        let remaining = self.data_bytes.saturating_sub(self.position);
        self.format.frames_from_bytes(remaining)
    }

    /// Read up to `max_frames` interleaved i16 frames into `out`.
    ///
    /// `out` must hold at least `max_frames * channels` samples. Returns frames
    /// read; `0` at EOF.
    pub(crate) fn read_i16_frames(
        &mut self,
        out: &mut [i16],
        max_frames: usize,
    ) -> Result<usize, AppError> {
        let channels = usize::from(self.format.channels());
        if channels == 0 {
            return Err(AppError::message("transcription", "wav has no channels"));
        }
        let capacity_frames = out.len() / channels;
        let frames = max_frames.min(capacity_frames);
        if frames == 0 {
            return Ok(0);
        }
        let want = frames
            .checked_mul(channels)
            .and_then(|samples| samples.checked_mul(2))
            .ok_or_else(|| AppError::message("transcription", "frame size overflow"))?
            as u64;
        let remaining = self.data_bytes.saturating_sub(self.position);
        let take = want.min(remaining);
        if take == 0 {
            return Ok(0);
        }
        if !take.is_multiple_of(2) {
            return Err(AppError::message("transcription", "truncated pcm length"));
        }
        let mut bytes = vec![0_u8; take as usize];
        self.file
            .read_exact(&mut bytes)
            .map_err(|source| AppError::io("transcription", source))?;
        let samples = take as usize / 2;
        for (index, chunk) in bytes.as_chunks::<2>().0.iter().enumerate() {
            out[index] = i16::from_le_bytes(*chunk);
        }
        self.position = self.position.saturating_add(take);
        Ok(samples / channels)
    }
}

fn refuse_unsupported_path(path: &Path) -> Result<(), AppError> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if name.ends_with(".wav.partial") || name.ends_with(".partial") {
        return Err(AppError::message(
            "transcription",
            "refusing to open .wav.partial; wait for finalize",
        ));
    }
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".mp3") {
        return Err(AppError::message(
            "transcription",
            "unsupported container/codec: mp3 (out of v1)",
        ));
    }
    if lower.ends_with(".flac") {
        // M8 will accept FLAC through flac-codec on this reader.
        return Err(AppError::message(
            "transcription",
            "unsupported container/codec: flac (deferred to M8 / flac-codec)",
        ));
    }
    if !lower.ends_with(".wav") {
        let ext = path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("unknown");
        return Err(AppError::message(
            "transcription",
            format!("unsupported container/codec: {ext}"),
        ));
    }
    Ok(())
}

fn parse_wav_header(file: &mut File) -> Result<(AudioFormat, u64, u64), AppError> {
    let mut header = [0_u8; WAV_HEADER_BYTES as usize];
    file.read_exact(&mut header)
        .map_err(|source| AppError::io("transcription", source))?;
    if &header[0..4] != b"RIFF" || &header[8..12] != b"WAVE" {
        return Err(AppError::message(
            "transcription",
            "unsupported container/codec: not RIFF WAVE",
        ));
    }
    if &header[12..16] != b"fmt " {
        return Err(AppError::message("transcription", "missing fmt chunk"));
    }
    let audio_format = u16::from_le_bytes([header[20], header[21]]);
    if audio_format != 1 {
        return Err(AppError::message(
            "transcription",
            format!("unsupported container/codec: WAV format tag {audio_format}"),
        ));
    }
    let channels = u16::from_le_bytes([header[22], header[23]]);
    let sample_rate = u32::from_le_bytes([header[24], header[25], header[26], header[27]]);
    let bits = u16::from_le_bytes([header[34], header[35]]);
    if bits != 16 {
        return Err(AppError::message(
            "transcription",
            format!("unsupported container/codec: {bits}-bit PCM"),
        ));
    }
    if &header[36..40] != b"data" {
        return Err(AppError::message(
            "transcription",
            "unsupported container/codec: non-canonical WAV (expected 44-byte header)",
        ));
    }
    let data_bytes = u32::from_le_bytes([header[40], header[41], header[42], header[43]]) as u64;
    let format = AudioFormat::new(sample_rate, channels, SampleEncoding::I16).ok_or_else(|| {
        AppError::message(
            "transcription",
            "unsupported container/codec: invalid pcm16 format",
        )
    })?;
    Ok((format, WAV_HEADER_BYTES, data_bytes))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::TranscriptWavReader;
    use crate::common::error::AppError;
    use crate::domain::audio::CivilTime;
    use crate::infrastructure::files::wav::{quantize_sample, WavWriter};

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("skrivli-txreader-{tag}-{id}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("dir");
        dir
    }

    #[test]
    fn reads_finalized_wav_in_bounded_chunks() {
        // Arrange
        let dir = temp_dir("ok");
        let when = CivilTime::new(2026, 11, 5, 12, 0, 0).expect("time");
        let mut writer = WavWriter::create(&dir, when).expect("create");
        let mut frames = Vec::new();
        for _ in 0..960 {
            frames.push(0.25);
            frames.push(-0.25);
        }
        writer.write_samples(&frames).expect("write");
        let path = writer.finalize().expect("finalize");

        // Act
        let mut reader = TranscriptWavReader::open(&path).expect("open");
        let mut buffer = vec![0_i16; 480 * 2];
        let first = reader.read_i16_frames(&mut buffer, 480).expect("read1");
        let second = reader.read_i16_frames(&mut buffer, 480).expect("read2");
        let eof = reader.read_i16_frames(&mut buffer, 480).expect("eof");

        // Assert
        assert_eq!(first, 480);
        assert_eq!(second, 480);
        assert_eq!(eof, 0);
        assert_eq!(buffer[0], quantize_sample(0.25));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn refuses_partial_and_names_unsupported_codec() {
        // Arrange
        let dir = temp_dir("bad");
        let partial = dir.join("clip.wav.partial");
        fs::write(&partial, b"RIFF").expect("partial");
        let mp3 = dir.join("clip.mp3");
        fs::write(&mp3, b"ID3").expect("mp3");
        let flac = dir.join("clip.flac");
        fs::write(&flac, b"fLaC").expect("flac");

        // Act
        let partial_err = TranscriptWavReader::open(&partial).expect_err("partial");
        let mp3_err = TranscriptWavReader::open(&mp3).expect_err("mp3");
        let flac_err = TranscriptWavReader::open(&flac).expect_err("flac");

        // Assert
        assert!(partial_err.to_string().contains("partial"));
        assert!(mp3_err.to_string().contains("mp3"));
        assert!(flac_err.to_string().contains("flac"));
        assert_eq!(
            AppError::message("transcription", "x").user_message(),
            "Could not prepare the transcript."
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn refuses_non_pcm_wav_naming_format_tag() {
        // Arrange — RIFF/WAVE with format tag 3 (IEEE float).
        let dir = temp_dir("float");
        let path = dir.join("float.wav");
        let mut bytes = vec![0_u8; 44];
        bytes[0..4].copy_from_slice(b"RIFF");
        bytes[8..12].copy_from_slice(b"WAVE");
        bytes[12..16].copy_from_slice(b"fmt ");
        bytes[16..20].copy_from_slice(&16_u32.to_le_bytes());
        bytes[20..22].copy_from_slice(&3_u16.to_le_bytes());
        bytes[22..24].copy_from_slice(&1_u16.to_le_bytes());
        bytes[24..28].copy_from_slice(&16_000_u32.to_le_bytes());
        bytes[34..36].copy_from_slice(&32_u16.to_le_bytes());
        bytes[36..40].copy_from_slice(b"data");
        let mut file = fs::File::create(&path).expect("create");
        file.write_all(&bytes).expect("write");

        // Act
        let error = TranscriptWavReader::open(&path).expect_err("float");

        // Assert
        assert!(error.to_string().contains("format tag 3"));
        assert_eq!(error.user_message(), "Could not prepare the transcript.");
        let _ = fs::remove_dir_all(dir);
    }
}
