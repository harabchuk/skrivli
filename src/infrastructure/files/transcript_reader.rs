//! Bounded FLAC reader for offline transcription.
//!
//! Separate from playback. Opens the finalized path only: never `.flac.partial`
//! and never a non-FLAC container. Decodes through `flac-codec` in chunks.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use flac_codec::decode::FlacSampleReader;
use flac_codec::metadata::Metadata;

use crate::common::error::AppError;
use crate::domain::audio::{AudioFormat, SampleEncoding};
use crate::infrastructure::files::flac::map_flac;

/// Default read size: about one second of 48 kHz stereo PCM16.
pub(crate) const DEFAULT_CHUNK_FRAMES: usize = 48_000;

/// Opened finalized 16-bit FLAC for chunked transcription reads.
pub(crate) struct TranscriptFlacReader {
    reader: FlacSampleReader<BufReader<File>>,
    format: AudioFormat,
    total_frames: Option<u64>,
    position: u64,
    scratch: Vec<i32>,
}

impl TranscriptFlacReader {
    /// Open a finalized recording. Refuses `.flac.partial` and non-FLAC files.
    pub(crate) fn open(path: &Path) -> Result<Self, AppError> {
        refuse_unsupported_path(path)?;
        let file = File::open(path).map_err(|source| AppError::io("transcription", source))?;
        let reader = FlacSampleReader::new(BufReader::new(file))
            .map_err(|error| map_flac("transcription", error))?;
        if reader.bits_per_sample() != 16 {
            return Err(AppError::message(
                "transcription",
                format!(
                    "unsupported container/codec: {}-bit FLAC",
                    reader.bits_per_sample()
                ),
            ));
        }
        let format = AudioFormat::new(
            reader.sample_rate(),
            u16::from(reader.channel_count()),
            SampleEncoding::I16,
        )
        .ok_or_else(|| {
            AppError::message(
                "transcription",
                "unsupported container/codec: invalid pcm16 format",
            )
        })?;
        let total_frames = reader.total_samples();
        Ok(Self {
            reader,
            format,
            total_frames,
            position: 0,
            scratch: Vec::new(),
        })
    }

    pub(crate) fn format(&self) -> AudioFormat {
        self.format
    }

    /// Frames remaining, when STREAMINFO records the total.
    pub(crate) fn frames_remaining(&self) -> Option<u64> {
        self.total_frames
            .map(|total| total.saturating_sub(self.position))
    }

    /// Read up to `max_frames` interleaved i16 frames into `out`.
    ///
    /// `out` must hold at least `max_frames * channels` samples. Returns frames
    /// read; `0` at EOF. Short reads happen only at the end of the stream.
    pub(crate) fn read_i16_frames(
        &mut self,
        out: &mut [i16],
        max_frames: usize,
    ) -> Result<usize, AppError> {
        let channels = usize::from(self.format.channels());
        let frames = max_frames.min(out.len() / channels);
        let wanted = frames * channels;
        if wanted == 0 {
            return Ok(0);
        }
        self.scratch.clear();
        self.scratch.resize(wanted, 0);
        let mut filled = 0;
        while filled < wanted {
            let read = self
                .reader
                .read(&mut self.scratch[filled..])
                .map_err(|error| map_flac("transcription", error))?;
            if read == 0 {
                break;
            }
            filled += read;
        }
        let whole = filled - filled % channels;
        for (slot, sample) in out.iter_mut().zip(&self.scratch[..whole]) {
            *slot = i16::try_from(*sample).map_err(|_| {
                AppError::message("transcription", "sample is outside the 16-bit range")
            })?;
        }
        let read_frames = whole / channels;
        self.position = self.position.saturating_add(read_frames as u64);
        Ok(read_frames)
    }
}

fn refuse_unsupported_path(path: &Path) -> Result<(), AppError> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".partial") {
        return Err(AppError::message(
            "transcription",
            "refusing to open .flac.partial; wait for finalize",
        ));
    }
    if !lower.ends_with(".flac") {
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::TranscriptFlacReader;
    use crate::common::error::AppError;
    use crate::domain::audio::CivilTime;
    use crate::infrastructure::files::flac::{quantize_sample, FlacWriter};

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("skrivli-txreader-{tag}-{id}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("dir");
        dir
    }

    #[test]
    fn reads_finalized_flac_in_bounded_chunks() {
        // Arrange
        let dir = temp_dir("ok");
        let when = CivilTime::new(2026, 11, 5, 12, 0, 0).expect("time");
        let mut writer = FlacWriter::create(
            &dir,
            when,
            crate::infrastructure::files::flac::max_recording_frames(),
        )
        .expect("create");
        let mut frames = Vec::new();
        for _ in 0..9_600 {
            frames.push(0.25);
            frames.push(-0.25);
        }
        writer.write_samples(&frames).expect("write");
        let path = writer.finalize().expect("finalize");

        // Act
        let mut reader = TranscriptFlacReader::open(&path).expect("open");
        let remaining = reader.frames_remaining();
        let mut buffer = vec![0_i16; 4_800 * 2];
        let first = reader.read_i16_frames(&mut buffer, 4_800).expect("read1");
        let second = reader.read_i16_frames(&mut buffer, 4_800).expect("read2");
        let eof = reader.read_i16_frames(&mut buffer, 4_800).expect("eof");

        // Assert
        assert_eq!(remaining, Some(9_600));
        assert_eq!(first, 4_800);
        assert_eq!(second, 4_800);
        assert_eq!(eof, 0);
        assert_eq!(buffer[0], quantize_sample(0.25));
        assert_eq!(buffer[1], quantize_sample(-0.25));
        assert_eq!(reader.format().sample_rate(), 48_000);
        assert_eq!(reader.format().channels(), 2);
        assert_eq!(reader.frames_remaining(), Some(0));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn refuses_partial_and_non_flac_files() {
        // Arrange
        let dir = temp_dir("bad");
        let partial = dir.join("clip.flac.partial");
        fs::write(&partial, b"fLaC").expect("partial");
        let ogg = dir.join("clip.ogg");
        fs::write(&ogg, b"OggS").expect("ogg");
        let mp3 = dir.join("clip.mp3");
        fs::write(&mp3, b"ID3").expect("mp3");

        // Act
        let partial_err = TranscriptFlacReader::open(&partial).err().expect("partial");
        let ogg_err = TranscriptFlacReader::open(&ogg).err().expect("ogg");
        let mp3_err = TranscriptFlacReader::open(&mp3).err().expect("mp3");

        // Assert
        assert!(partial_err.to_string().contains("partial"));
        assert!(ogg_err.to_string().contains("ogg"));
        assert!(mp3_err.to_string().contains("mp3"));
        assert_eq!(
            AppError::message("transcription", "x").user_message(),
            "Could not prepare the transcript."
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn refuses_a_flac_name_that_is_not_flac_data() {
        // Arrange
        let dir = temp_dir("junk");
        let path = dir.join("junk.flac");
        fs::write(&path, b"OggS....junk").expect("junk");

        // Act
        let error = TranscriptFlacReader::open(&path).err().expect("junk");

        // Assert
        assert_eq!(error.user_message(), "Could not prepare the transcript.");
        let _ = fs::remove_dir_all(dir);
    }
}
