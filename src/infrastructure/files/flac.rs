//! Streaming 16-bit stereo 48 kHz FLAC writer via [`flac_codec`].
//!
//! Frames land in `Name.flac.partial`. Encoded blocks are flushed and synced
//! about every two seconds. Finalize writes STREAMINFO (sample count, MD5) and
//! the seek table, then renames the file to `.flac`.
//!
//! A recording stops at the caller's frame cap, at most [`MAX_RECORDING_SECONDS`]. Accepted frames are written
//! and [`FlacWrite::SafeStop`] tells the caller to finalize.
//!
//! Cleanup policy: never auto-delete unknown `*.flac.partial` files.
//! [`FlacWriter::abandon`] removes only the partial it created.

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use flac_codec::encode::{FlacSampleWriter, Options};

use crate::common::error::AppError;
use crate::domain::audio::{CivilTime, INTERNAL_CHANNELS, INTERNAL_SAMPLE_RATE};
use crate::domain::recording::MAX_RECORDING_SECONDS;

const BITS_PER_SAMPLE: u32 = 16;
const FLUSH_EVERY_FRAMES: u64 = INTERNAL_SAMPLE_RATE as u64 * 2;
/// Room for a seek table covering the longest recording (one point per 10 s,
/// 18 bytes each) so long files can seek without decoding from the start.
const METADATA_PADDING_BYTES: u32 = 80_000;

pub(crate) fn max_recording_frames() -> u64 {
    MAX_RECORDING_SECONDS * u64::from(INTERNAL_SAMPLE_RATE)
}

pub(crate) fn quantize_sample(sample: f32) -> i16 {
    let clamped = sample.clamp(-1.0, 1.0);
    let scaled = clamped * 32767.0;
    let rounded = if scaled >= 0.0 {
        scaled + 0.5
    } else {
        scaled - 0.5
    };
    let truncated = rounded as i32;
    truncated.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
}

pub(crate) fn map_flac(context: &'static str, error: flac_codec::Error) -> AppError {
    match error {
        flac_codec::Error::Io(source) => AppError::io(context, source),
        other => AppError::message(context, other.to_string()),
    }
}

fn encoder_options() -> Result<Options, AppError> {
    Options::default()
        .padding(METADATA_PADDING_BYTES)
        .map_err(|error| AppError::message("save recording", error.to_string()))
}

/// Buffered file shared between the encoder and the writer, so the writer can
/// flush and `sync_all` for crash durability while the encoder owns its handle.
#[derive(Clone)]
struct SharedFile {
    inner: Arc<Mutex<BufWriter<File>>>,
}

impl SharedFile {
    fn new(file: File) -> Self {
        Self {
            inner: Arc::new(Mutex::new(BufWriter::new(file))),
        }
    }

    fn lock(&self) -> std::io::Result<MutexGuard<'_, BufWriter<File>>> {
        self.inner
            .lock()
            .map_err(|_| std::io::Error::other("recording file lock was poisoned"))
    }

    fn sync(&self) -> std::io::Result<()> {
        let mut guard = self.lock()?;
        guard.flush()?;
        guard.get_ref().sync_all()
    }
}

impl Write for SharedFile {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.lock()?.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.lock()?.flush()
    }
}

impl Seek for SharedFile {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.lock()?.seek(pos)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FlacWrite {
    Wrote {
        frames: u64,
    },
    /// Accepted frames were written. The caller must finalize.
    SafeStop {
        frames: u64,
    },
}

pub(crate) struct FlacWriter {
    encoder: Option<FlacSampleWriter<SharedFile>>,
    file: Option<SharedFile>,
    partial: PathBuf,
    final_path: PathBuf,
    frames_written: u64,
    frame_cap: u64,
    frames_since_flush: u64,
    scratch: Vec<i32>,
    finalized: bool,
}

impl FlacWriter {
    /// `frame_cap` is clamped to [`max_recording_frames`], the ceiling the seek-table padding covers.
    pub(crate) fn create(
        directory: &Path,
        when: CivilTime,
        frame_cap: u64,
    ) -> Result<Self, AppError> {
        Self::create_with_cap(directory, when, frame_cap.min(max_recording_frames()))
    }

    fn create_with_cap(
        directory: &Path,
        when: CivilTime,
        frame_cap: u64,
    ) -> Result<Self, AppError> {
        std::fs::create_dir_all(directory)
            .map_err(|source| AppError::io("recordings folder", source))?;
        let (partial, final_path) = allocate_paths(directory, when)?;
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .read(true)
            .open(&partial)
            .map_err(|source| AppError::io("save recording", source))?;
        let shared = SharedFile::new(file);
        let encoder = FlacSampleWriter::new(
            shared.clone(),
            encoder_options()?,
            INTERNAL_SAMPLE_RATE,
            BITS_PER_SAMPLE,
            INTERNAL_CHANNELS as u8,
            None,
        )
        .map_err(|error| map_flac("save recording", error))?;
        let mut writer = Self {
            encoder: Some(encoder),
            file: Some(shared),
            partial,
            final_path,
            frames_written: 0,
            frame_cap,
            frames_since_flush: 0,
            scratch: Vec::new(),
            finalized: false,
        };
        writer.flush()?;
        Ok(writer)
    }

    #[cfg(test)]
    pub(crate) fn final_path(&self) -> &Path {
        &self.final_path
    }

    #[cfg(test)]
    pub(crate) fn partial_path(&self) -> &Path {
        &self.partial
    }

    #[cfg(test)]
    pub(crate) fn frames_written(&self) -> u64 {
        self.frames_written
    }

    pub(crate) fn write_samples(&mut self, stereo: &[f32]) -> Result<FlacWrite, AppError> {
        let frames = (stereo.len() / 2) as u64;
        if frames == 0 {
            return Ok(FlacWrite::Wrote { frames: 0 });
        }
        let room = self.frame_cap.saturating_sub(self.frames_written);
        let accept = frames.min(room);
        let stop = accept < frames || self.frames_written + accept >= self.frame_cap;
        if accept > 0 {
            self.scratch.clear();
            self.scratch.extend(
                stereo
                    .iter()
                    .take(accept as usize * 2)
                    .map(|sample| i32::from(quantize_sample(*sample))),
            );
            let encoder = self
                .encoder
                .as_mut()
                .ok_or_else(|| AppError::message("save recording", "the flac file is closed"))?;
            encoder
                .write(&self.scratch)
                .map_err(|error| map_flac("save recording", error))?;
            self.frames_written += accept;
            self.frames_since_flush += accept;
        }
        if self.frames_since_flush >= FLUSH_EVERY_FRAMES || stop {
            self.flush()?;
        }
        if stop {
            Ok(FlacWrite::SafeStop { frames: accept })
        } else {
            Ok(FlacWrite::Wrote { frames: accept })
        }
    }

    /// Push encoded blocks to disk and sync them.
    pub(crate) fn flush(&mut self) -> Result<(), AppError> {
        self.frames_since_flush = 0;
        self.file
            .as_ref()
            .ok_or_else(|| AppError::message("save recording", "the flac file is closed"))?
            .sync()
            .map_err(|source| AppError::io("save recording", source))
    }

    pub(crate) fn finalize(mut self) -> Result<PathBuf, AppError> {
        self.close_encoder()?;
        if let Some(file) = self.file.take() {
            file.sync()
                .map_err(|source| AppError::io("save recording", source))?;
        }
        std::fs::rename(&self.partial, &self.final_path)
            .map_err(|source| AppError::io("save recording", source))?;
        self.finalized = true;
        Ok(self.final_path.clone())
    }

    pub(crate) fn abandon(mut self) -> Result<(), AppError> {
        self.finalized = true;
        let _ = self.close_encoder();
        self.file = None;
        if self.partial.exists() {
            std::fs::remove_file(&self.partial)
                .map_err(|source| AppError::io("save recording", source))?;
        }
        Ok(())
    }

    /// Finish the stream. A recording with no frames stays a valid empty FLAC.
    fn close_encoder(&mut self) -> Result<(), AppError> {
        let Some(encoder) = self.encoder.take() else {
            return Ok(());
        };
        match encoder.finalize() {
            Ok(()) => Ok(()),
            Err(flac_codec::Error::NoSamples) if self.frames_written == 0 => Ok(()),
            Err(error) => Err(map_flac("save recording", error)),
        }
    }
}

impl Drop for FlacWriter {
    fn drop(&mut self) {
        if self.finalized {
            return;
        }
        let _ = self.close_encoder();
        if let Some(file) = self.file.take() {
            let _ = file.sync();
        }
    }
}

/// Write `frames` of silence as a finalized FLAC at `path` (test fixtures).
pub(crate) fn write_silence(path: &Path, frames: usize) -> Result<(), AppError> {
    let file = File::create(path).map_err(|source| AppError::io("save recording", source))?;
    let mut encoder = FlacSampleWriter::new(
        BufWriter::new(file),
        encoder_options()?,
        INTERNAL_SAMPLE_RATE,
        BITS_PER_SAMPLE,
        INTERNAL_CHANNELS as u8,
        None,
    )
    .map_err(|error| map_flac("save recording", error))?;
    encoder
        .write(&vec![0_i32; frames * usize::from(INTERNAL_CHANNELS)])
        .map_err(|error| map_flac("save recording", error))?;
    match encoder.finalize() {
        Ok(()) => Ok(()),
        Err(flac_codec::Error::NoSamples) if frames == 0 => Ok(()),
        Err(error) => Err(map_flac("save recording", error)),
    }
}

fn allocate_paths(directory: &Path, when: CivilTime) -> Result<(PathBuf, PathBuf), AppError> {
    let stem = format!(
        "Skrivli_{:04}-{:02}-{:02}_{:02}-{:02}-{:02}",
        when.year, when.month, when.day, when.hour, when.minute, when.second
    );
    for suffix in 0..10_000 {
        let name = if suffix == 0 {
            format!("{stem}.flac")
        } else {
            format!("{stem}-{}.flac", suffix + 1)
        };
        let final_path = directory.join(&name);
        let partial = directory.join(format!("{name}.partial"));
        if !final_path.exists() && !partial.exists() {
            return Ok((partial, final_path));
        }
    }
    Err(AppError::message(
        "save recording",
        "could not choose a recording name",
    ))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    use flac_codec::decode::FlacSampleReader;
    use flac_codec::metadata::Metadata;

    use super::{max_recording_frames, quantize_sample, write_silence, FlacWrite, FlacWriter};
    use crate::domain::audio::CivilTime;

    fn temp_dir(label: &str) -> std::path::PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("skrivli-flac-{label}-{}-{id}", std::process::id()));
        fs::create_dir_all(&dir).expect("temp");
        dir
    }

    fn stamp() -> CivilTime {
        CivilTime::new(2026, 11, 4, 13, 1, 2).expect("time")
    }

    #[test]
    fn duration_cap_is_twelve_hours_of_frames() {
        // Arrange
        let expected = 12 * 60 * 60 * 48_000;

        // Act
        let frames = max_recording_frames();

        // Assert
        assert_eq!(frames, expected);
    }

    #[test]
    fn quantize_clamps_to_i16() {
        // Arrange
        let samples = [0.0, 1.0, -1.0, 2.0, -2.0];

        // Act
        let pcm: Vec<_> = samples.into_iter().map(quantize_sample).collect();

        // Assert
        assert_eq!(pcm, vec![0, 32767, -32767, 32767, -32767]);
    }

    #[test]
    fn finalize_writes_streaminfo_and_round_trips_samples() {
        // Arrange
        let dir = temp_dir("final");
        let mut writer = FlacWriter::create(&dir, stamp(), max_recording_frames()).expect("create");
        let samples = vec![0.5_f32, -0.5, 0.0, 0.0];

        // Act
        let wrote = writer.write_samples(&samples).expect("write");
        let path = writer.finalize().expect("finalize");
        let mut reader = FlacSampleReader::open(&path).expect("open");
        let mut pcm = Vec::new();
        reader.read_to_end(&mut pcm).expect("decode");

        // Assert
        assert_eq!(wrote, FlacWrite::Wrote { frames: 2 });
        assert_eq!(reader.sample_rate(), 48_000);
        assert_eq!(reader.channel_count(), 2);
        assert_eq!(reader.bits_per_sample(), 16);
        assert_eq!(reader.total_samples(), Some(2));
        assert_eq!(
            pcm,
            vec![
                i32::from(quantize_sample(0.5)),
                i32::from(quantize_sample(-0.5)),
                0,
                0
            ]
        );
        assert!(!dir
            .join("Skrivli_2026-11-04_13-01-02.flac.partial")
            .exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn flush_pushes_encoded_blocks_into_the_partial() {
        // Arrange
        let dir = temp_dir("flush");
        let mut writer = FlacWriter::create(&dir, stamp(), max_recording_frames()).expect("create");
        let chunk = vec![0.25_f32; 9_600];
        let before = fs::metadata(writer.partial_path()).expect("meta").len();

        // Act
        for _ in 0..10 {
            writer.write_samples(&chunk).expect("write");
        }
        writer.flush().expect("flush");
        let after = fs::metadata(writer.partial_path()).expect("meta").len();

        // Assert
        assert_eq!(writer.frames_written(), 48_000);
        assert!(after > before, "encoded blocks should reach the file");
        writer.abandon().expect("abandon");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn collision_adds_a_numeric_suffix() {
        // Arrange
        let dir = temp_dir("name");
        let first = FlacWriter::create(&dir, stamp(), max_recording_frames()).expect("first");
        let first_name = first
            .final_path()
            .file_name()
            .expect("name")
            .to_string_lossy()
            .into_owned();

        // Act
        let second = FlacWriter::create(&dir, stamp(), max_recording_frames()).expect("second");
        let second_name = second
            .final_path()
            .file_name()
            .expect("name")
            .to_string_lossy()
            .into_owned();

        // Assert
        assert_eq!(first_name, "Skrivli_2026-11-04_13-01-02.flac");
        assert_eq!(second_name, "Skrivli_2026-11-04_13-01-02-2.flac");
        first.abandon().expect("abandon");
        second.abandon().expect("abandon");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn duration_cap_accepts_remaining_frames_then_safe_stops() {
        // Arrange
        let dir = temp_dir("cap");
        let mut writer = FlacWriter::create_with_cap(&dir, stamp(), 5).expect("create");
        let samples = vec![0.1_f32; 16];

        // Act
        let first = writer.write_samples(&samples).expect("first");
        let second = writer.write_samples(&samples).expect("second");
        let path = writer.finalize().expect("finalize");
        let reader = FlacSampleReader::open(&path).expect("open");

        // Assert
        assert_eq!(first, FlacWrite::SafeStop { frames: 5 });
        assert_eq!(second, FlacWrite::SafeStop { frames: 0 });
        assert_eq!(reader.total_samples(), Some(5));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn exactly_filling_the_cap_requests_a_stop() {
        // Arrange
        let dir = temp_dir("cap-exact");
        let mut writer = FlacWriter::create_with_cap(&dir, stamp(), 4).expect("create");

        // Act
        let wrote = writer.write_samples(&[0.0; 8]).expect("write");

        // Assert
        assert_eq!(wrote, FlacWrite::SafeStop { frames: 4 });
        writer.abandon().expect("abandon");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn finalizing_an_empty_recording_leaves_a_valid_flac() {
        // Arrange
        let dir = temp_dir("empty");
        let writer = FlacWriter::create(&dir, stamp(), max_recording_frames()).expect("create");

        // Act
        let path = writer.finalize().expect("finalize");
        let reader = FlacSampleReader::open(&path).expect("open");

        // Assert
        assert_eq!(reader.sample_rate(), 48_000);
        assert!(path.to_string_lossy().ends_with(".flac"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn dropping_an_unfinalized_writer_keeps_the_partial() {
        // Arrange
        let dir = temp_dir("drop");
        let mut writer = FlacWriter::create(&dir, stamp(), max_recording_frames()).expect("create");
        writer.write_samples(&[0.2; 200]).expect("write");
        let partial = writer.partial_path().to_path_buf();

        // Act
        drop(writer);

        // Assert
        assert!(partial.is_file());
        assert!(!dir.join("Skrivli_2026-11-04_13-01-02.flac").exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn write_silence_creates_a_decodable_file() {
        // Arrange
        let dir = temp_dir("silence");
        let path = dir.join("quiet.flac");

        // Act
        write_silence(&path, 9_000).expect("silence");
        let reader = FlacSampleReader::open(&path).expect("open");

        // Assert
        assert_eq!(reader.total_samples(), Some(9_000));
        let _ = fs::remove_dir_all(dir);
    }
}
