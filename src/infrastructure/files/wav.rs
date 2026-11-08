//! Streaming PCM16 stereo 48 kHz RIFF writer via [`hound`].
//!
//! Bytes land in `Name.wav.partial`. Length fields are rewritten about every
//! two seconds and again on finalize, then the file is renamed to `.wav`.
//! A standard RIFF size is a `u32`. This writer stops before `36 + data`
//! would exceed that field. It does not switch to RF64.
//!
//! Cleanup policy: never auto-delete unknown `*.wav.partial` files. [`Self::abandon`]
//! removes a partial only after a failed start before usable audio must be kept.

use std::fs::{File, OpenOptions};
#[cfg(test)]
use std::io::Read;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::common::error::AppError;
use crate::domain::audio::{
    AudioFormat, CivilTime, SampleEncoding, INTERNAL_CHANNELS, INTERNAL_SAMPLE_RATE,
};

/// Canonical PCM header size.
pub(crate) const WAV_HEADER_BYTES: u64 = 44;
/// Bytes counted in the RIFF size besides the PCM payload (`WAVE` + `fmt` + `data` headers).
pub(crate) const RIFF_OVERHEAD_BYTES: u64 = 36;
/// Largest payload whose RIFF chunk size still fits in `u32`.
pub(crate) const MAX_SAFE_DATA_BYTES: u64 = u32::MAX as u64 - RIFF_OVERHEAD_BYTES;
const REPAIR_EVERY_FRAMES: u64 = INTERNAL_SAMPLE_RATE as u64 * 2;
const BYTES_PER_FRAME: u64 = 4;
const BYTES_PER_SAMPLE: u64 = 2;

fn wav_spec() -> hound::WavSpec {
    hound::WavSpec {
        channels: INTERNAL_CHANNELS,
        sample_rate: INTERNAL_SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    }
}

pub(crate) fn data_bytes_fit(existing: u64, additional: u64) -> bool {
    match existing.checked_add(additional) {
        Some(total) => total <= MAX_SAFE_DATA_BYTES,
        None => false,
    }
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

fn map_hound(context: &'static str, error: hound::Error) -> AppError {
    match error {
        hound::Error::IoError(source) => AppError::io(context, source),
        other => AppError::message(context, other.to_string()),
    }
}

/// File wrapper so [`hound::WavWriter::flush`] also `sync_all`s for crash durability.
struct SyncFile {
    file: File,
}

impl Write for SyncFile {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.file.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()?;
        self.file.sync_all()
    }
}

impl Seek for SyncFile {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.file.seek(pos)
    }
}

type HoundWriter = hound::WavWriter<SyncFile>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WavWrite {
    Wrote {
        frames: u64,
    },
    /// Accepted frames were written. The caller must finalize.
    SafeStop {
        frames: u64,
    },
}

pub(crate) struct WavWriter {
    writer: Option<HoundWriter>,
    partial: PathBuf,
    final_path: PathBuf,
    frames_since_repair: u64,
    finalized: bool,
}

impl WavWriter {
    pub(crate) fn create(directory: &Path, when: CivilTime) -> Result<Self, AppError> {
        std::fs::create_dir_all(directory)
            .map_err(|source| AppError::io("recordings folder", source))?;
        let (partial, final_path) = allocate_paths(directory, when)?;
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .read(true)
            .open(&partial)
            .map_err(|source| AppError::io("save recording", source))?;
        let writer = hound::WavWriter::new(SyncFile { file }, wav_spec())
            .map_err(|error| map_hound("save recording", error))?;
        let mut writer = Self {
            writer: Some(writer),
            partial,
            final_path,
            frames_since_repair: 0,
            finalized: false,
        };
        writer.repair()?;
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
    pub(crate) fn data_bytes(&self) -> u64 {
        self.data_bytes_written()
    }

    pub(crate) fn write_samples(&mut self, stereo: &[f32]) -> Result<WavWrite, AppError> {
        let frames = stereo.len() / 2;
        if frames == 0 {
            return Ok(WavWrite::Wrote { frames: 0 });
        }
        let sample_format =
            AudioFormat::new(INTERNAL_SAMPLE_RATE, INTERNAL_CHANNELS, SampleEncoding::I16)
                .ok_or_else(|| {
                    AppError::message("save recording", "internal format was rejected")
                })?;
        let wanted = sample_format
            .bytes_for_frames(frames as u64)
            .ok_or_else(|| {
                AppError::message("save recording", "frame count does not fit in a length")
            })?;
        let data_bytes = self.data_bytes_written();
        let (accept_frames, stop) = if data_bytes_fit(data_bytes, wanted) {
            (frames, false)
        } else {
            let room = MAX_SAFE_DATA_BYTES.saturating_sub(data_bytes);
            ((room / BYTES_PER_FRAME) as usize, true)
        };
        if accept_frames == 0 {
            self.repair()?;
            return Ok(WavWrite::SafeStop { frames: 0 });
        }
        let num_samples = u32::try_from(
            accept_frames.saturating_mul(usize::from(INTERNAL_CHANNELS)),
        )
        .map_err(|_| AppError::message("save recording", "frame count does not fit in a length"))?;
        {
            let writer = self.writer_mut()?;
            let mut samples = writer.get_i16_writer(num_samples);
            for sample in stereo.iter().take(num_samples as usize) {
                samples.write_sample(quantize_sample(*sample));
            }
            samples
                .flush()
                .map_err(|error| map_hound("save recording", error))?;
        }
        self.frames_since_repair = self
            .frames_since_repair
            .saturating_add(accept_frames as u64);
        if self.frames_since_repair >= REPAIR_EVERY_FRAMES || stop {
            self.repair()?;
            self.frames_since_repair = 0;
        }
        if stop {
            Ok(WavWrite::SafeStop {
                frames: accept_frames as u64,
            })
        } else {
            Ok(WavWrite::Wrote {
                frames: accept_frames as u64,
            })
        }
    }

    pub(crate) fn repair(&mut self) -> Result<(), AppError> {
        self.writer_mut()?
            .flush()
            .map_err(|error| map_hound("save recording", error))
    }

    pub(crate) fn finalize(mut self) -> Result<PathBuf, AppError> {
        if let Some(writer) = self.writer.take() {
            writer
                .finalize()
                .map_err(|error| map_hound("save recording", error))?;
        }
        std::fs::rename(&self.partial, &self.final_path)
            .map_err(|source| AppError::io("save recording", source))?;
        self.finalized = true;
        Ok(self.final_path.clone())
    }

    pub(crate) fn abandon(mut self) -> Result<(), AppError> {
        self.finalized = true;
        if let Some(writer) = self.writer.take() {
            let _ = writer.finalize();
        }
        if self.partial.exists() {
            std::fs::remove_file(&self.partial)
                .map_err(|source| AppError::io("save recording", source))?;
        }
        Ok(())
    }

    fn data_bytes_written(&self) -> u64 {
        self.writer
            .as_ref()
            .map(|writer| u64::from(writer.len()).saturating_mul(BYTES_PER_SAMPLE))
            .unwrap_or(0)
    }

    fn writer_mut(&mut self) -> Result<&mut HoundWriter, AppError> {
        self.writer
            .as_mut()
            .ok_or_else(|| AppError::message("save recording", "the wav file is closed"))
    }
}

impl Drop for WavWriter {
    fn drop(&mut self) {
        if self.finalized {
            return;
        }
        if let Some(writer) = self.writer.take() {
            let _ = writer.finalize();
        }
    }
}

fn allocate_paths(directory: &Path, when: CivilTime) -> Result<(PathBuf, PathBuf), AppError> {
    let stem = format!(
        "Skrivli_{:04}-{:02}-{:02}_{:02}-{:02}-{:02}",
        when.year, when.month, when.day, when.hour, when.minute, when.second
    );
    for suffix in 0..10_000 {
        let name = if suffix == 0 {
            format!("{stem}.wav")
        } else {
            format!("{stem}-{}.wav", suffix + 1)
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
pub(crate) fn read_wav_lengths(path: &Path) -> Result<(u32, u32, u64), AppError> {
    let mut file = File::open(path).map_err(|source| AppError::io("save recording", source))?;
    let mut header = [0_u8; 44];
    file.read_exact(&mut header)
        .map_err(|source| AppError::io("save recording", source))?;
    let riff = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
    let data = u32::from_le_bytes([header[40], header[41], header[42], header[43]]);
    let length = file
        .metadata()
        .map_err(|source| AppError::io("save recording", source))?
        .len();
    Ok((riff, data, length))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::{
        data_bytes_fit, quantize_sample, read_wav_lengths, WavWrite, WavWriter,
        MAX_SAFE_DATA_BYTES, RIFF_OVERHEAD_BYTES,
    };
    use crate::domain::audio::CivilTime;

    fn temp_dir(label: &str) -> std::path::PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("skrivli-wav-{label}-{}-{id}", std::process::id()));
        fs::create_dir_all(&dir).expect("temp");
        dir
    }

    fn stamp() -> CivilTime {
        CivilTime::new(2026, 11, 4, 13, 1, 2).expect("time")
    }

    #[test]
    fn riff_limit_rejects_the_next_byte_without_a_huge_file() {
        // Arrange
        let limit = MAX_SAFE_DATA_BYTES;

        // Act
        let fits = data_bytes_fit(limit - 4, 4);
        let overflows = data_bytes_fit(limit, 4);
        let wraps = data_bytes_fit(u64::MAX, 1);

        // Assert
        assert_eq!(limit, u32::MAX as u64 - RIFF_OVERHEAD_BYTES);
        assert!(fits);
        assert!(!overflows);
        assert!(!wraps);
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
    fn finalize_writes_matching_header_and_data_lengths() {
        // Arrange
        let dir = temp_dir("final");
        let mut writer = WavWriter::create(&dir, stamp()).expect("create");
        let samples = vec![0.5_f32, -0.5, 0.0, 0.0];

        // Act
        let wrote = writer.write_samples(&samples).expect("write");
        let path = writer.finalize().expect("finalize");
        let (riff, data, length) = read_wav_lengths(&path).expect("read");

        // Assert
        assert_eq!(wrote, WavWrite::Wrote { frames: 2 });
        assert_eq!(data, 8);
        assert_eq!(riff, 36 + 8);
        assert_eq!(length, 44 + 8);
        assert!(!dir.join("Skrivli_2026-11-04_13-01-02.wav.partial").exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn repair_updates_a_partial_before_finalize() {
        // Arrange
        let dir = temp_dir("repair");
        let mut writer = WavWriter::create(&dir, stamp()).expect("create");
        let chunk = vec![0.0_f32; 960];

        // Act
        for _ in 0..200 {
            writer.write_samples(&chunk).expect("write");
        }
        writer.repair().expect("repair");
        let (riff, data, length) = read_wav_lengths(writer.partial_path()).expect("partial");

        // Assert
        assert_eq!(data as u64, writer.data_bytes());
        assert_eq!(riff as u64, writer.data_bytes() + 36);
        assert_eq!(length, 44 + writer.data_bytes());
        writer.abandon().expect("abandon");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn collision_adds_a_numeric_suffix() {
        // Arrange
        let dir = temp_dir("name");
        let first = WavWriter::create(&dir, stamp()).expect("first");
        let first_name = first
            .final_path()
            .file_name()
            .expect("name")
            .to_string_lossy()
            .into_owned();

        // Act
        let second = WavWriter::create(&dir, stamp()).expect("second");
        let second_name = second
            .final_path()
            .file_name()
            .expect("name")
            .to_string_lossy()
            .into_owned();

        // Assert
        assert_eq!(first_name, "Skrivli_2026-11-04_13-01-02.wav");
        assert_eq!(second_name, "Skrivli_2026-11-04_13-01-02-2.wav");
        first.abandon().expect("abandon");
        second.abandon().expect("abandon");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn writer_output_opens_as_skrivli_spec_in_hound() {
        // Arrange
        let dir = temp_dir("hound-hdr");
        let writer = WavWriter::create(&dir, stamp()).expect("create");
        let path = writer.partial_path().to_path_buf();

        // Act
        let reader = hound::WavReader::open(&path).expect("open");

        // Assert
        assert_eq!(reader.spec(), super::wav_spec());
        writer.abandon().expect("abandon");
        let _ = fs::remove_dir_all(dir);
    }
}
