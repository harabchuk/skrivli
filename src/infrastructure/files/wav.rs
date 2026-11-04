//! Streaming PCM16 stereo 48 kHz RIFF writer.
//!
//! Bytes land in `Name.wav.partial`. Length fields are rewritten about every
//! two seconds and again on finalize, then the file is renamed to `.wav`.
//! A standard RIFF size is a `u32`. This writer stops before `36 + data`
//! would exceed that field. It does not switch to RF64.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
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
    file: Option<File>,
    partial: PathBuf,
    final_path: PathBuf,
    data_bytes: u64,
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
        let mut writer = Self {
            file: Some(file),
            partial,
            final_path,
            data_bytes: 0,
            frames_since_repair: 0,
            finalized: false,
        };
        writer.write_header()?;
        writer
            .file_mut()?
            .sync_all()
            .map_err(|source| AppError::io("save recording", source))?;
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
        self.data_bytes
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
        let (accept_frames, stop) = if data_bytes_fit(self.data_bytes, wanted) {
            (frames, false)
        } else {
            let room = MAX_SAFE_DATA_BYTES.saturating_sub(self.data_bytes);
            ((room / BYTES_PER_FRAME) as usize, true)
        };
        if accept_frames == 0 {
            self.repair()?;
            return Ok(WavWrite::SafeStop { frames: 0 });
        }
        let mut pcm = Vec::with_capacity(accept_frames.saturating_mul(4));
        for sample in stereo.iter().take(accept_frames.saturating_mul(2)) {
            pcm.extend_from_slice(&quantize_sample(*sample).to_le_bytes());
        }
        let file = self.file_mut()?;
        file.seek(SeekFrom::End(0))
            .map_err(|source| AppError::io("save recording", source))?;
        file.write_all(&pcm)
            .map_err(|source| AppError::io("save recording", source))?;
        self.data_bytes = self.data_bytes.saturating_add(pcm.len() as u64);
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
        let data_bytes = u32::try_from(self.data_bytes)
            .map_err(|_| AppError::message("wav limit", "data length exceeds the RIFF field"))?;
        let riff_size = data_bytes.saturating_add(RIFF_OVERHEAD_BYTES as u32);
        let file = self.file_mut()?;
        file.seek(SeekFrom::Start(4))
            .map_err(|source| AppError::io("save recording", source))?;
        file.write_all(&riff_size.to_le_bytes())
            .map_err(|source| AppError::io("save recording", source))?;
        file.seek(SeekFrom::Start(40))
            .map_err(|source| AppError::io("save recording", source))?;
        file.write_all(&data_bytes.to_le_bytes())
            .map_err(|source| AppError::io("save recording", source))?;
        file.sync_all()
            .map_err(|source| AppError::io("save recording", source))?;
        Ok(())
    }

    pub(crate) fn finalize(mut self) -> Result<PathBuf, AppError> {
        self.repair()?;
        self.file.take();
        std::fs::rename(&self.partial, &self.final_path)
            .map_err(|source| AppError::io("save recording", source))?;
        self.finalized = true;
        Ok(self.final_path.clone())
    }

    pub(crate) fn abandon(mut self) -> Result<(), AppError> {
        self.finalized = true;
        self.file.take();
        if self.partial.exists() {
            std::fs::remove_file(&self.partial)
                .map_err(|source| AppError::io("save recording", source))?;
        }
        Ok(())
    }

    fn write_header(&mut self) -> Result<(), AppError> {
        let header = pcm_header(0);
        self.file_mut()?
            .write_all(&header)
            .map_err(|source| AppError::io("save recording", source))
    }

    fn file_mut(&mut self) -> Result<&mut File, AppError> {
        self.file
            .as_mut()
            .ok_or_else(|| AppError::message("save recording", "the wav file is closed"))
    }
}

impl Drop for WavWriter {
    fn drop(&mut self) {
        if self.finalized || self.file.is_none() {
            return;
        }
        let _ = self.repair();
    }
}

pub(crate) fn pcm_header(data_bytes: u32) -> [u8; 44] {
    let mut header = [0_u8; 44];
    header[0..4].copy_from_slice(b"RIFF");
    let riff_size = data_bytes.saturating_add(RIFF_OVERHEAD_BYTES as u32);
    header[4..8].copy_from_slice(&riff_size.to_le_bytes());
    header[8..12].copy_from_slice(b"WAVE");
    header[12..16].copy_from_slice(b"fmt ");
    header[16..20].copy_from_slice(&16_u32.to_le_bytes());
    header[20..22].copy_from_slice(&1_u16.to_le_bytes());
    header[22..24].copy_from_slice(&INTERNAL_CHANNELS.to_le_bytes());
    header[24..28].copy_from_slice(&INTERNAL_SAMPLE_RATE.to_le_bytes());
    let byte_rate = INTERNAL_SAMPLE_RATE * u32::from(INTERNAL_CHANNELS) * 2;
    header[28..32].copy_from_slice(&byte_rate.to_le_bytes());
    header[32..34].copy_from_slice(&4_u16.to_le_bytes());
    header[34..36].copy_from_slice(&16_u16.to_le_bytes());
    header[36..40].copy_from_slice(b"data");
    header[40..44].copy_from_slice(&data_bytes.to_le_bytes());
    header
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RecoveryReport {
    pub(crate) recovered: usize,
    pub(crate) unrecoverable: usize,
}

/// Repair `*.wav.partial` files that still have a readable canonical header.
/// Unsafe files stay in place.
pub(crate) fn recover_partials(directory: &Path) -> RecoveryReport {
    let mut report = RecoveryReport {
        recovered: 0,
        unrecoverable: 0,
    };
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(_) => return report,
    };
    for entry in entries {
        let Ok(entry) = entry else {
            continue;
        };
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !name.ends_with(".wav.partial") {
            continue;
        }
        match recover_one(&path) {
            Ok(()) => report.recovered = report.recovered.saturating_add(1),
            Err(error) => {
                tracing::error!(error = %error, "a partial recording was left in place");
                report.unrecoverable = report.unrecoverable.saturating_add(1);
            }
        }
    }
    report
}

fn recover_one(partial: &Path) -> Result<(), AppError> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(partial)
        .map_err(|source| AppError::io("recover recording", source))?;
    let length = file
        .metadata()
        .map_err(|source| AppError::io("recover recording", source))?
        .len();
    if length < WAV_HEADER_BYTES {
        return Err(AppError::message(
            "recover recording",
            "partial header is incomplete",
        ));
    }
    let mut header = [0_u8; 44];
    file.seek(SeekFrom::Start(0))
        .map_err(|source| AppError::io("recover recording", source))?;
    file.read_exact(&mut header)
        .map_err(|source| AppError::io("recover recording", source))?;
    if !header_is_canonical(&header) {
        return Err(AppError::message(
            "recover recording",
            "partial header is not a Skrivli wav",
        ));
    }
    let data_bytes = (length - WAV_HEADER_BYTES) / BYTES_PER_FRAME * BYTES_PER_FRAME;
    if !data_bytes_fit(0, data_bytes) {
        return Err(AppError::message(
            "recover recording",
            "partial exceeds the RIFF size field",
        ));
    }
    let data_u32 = u32::try_from(data_bytes)
        .map_err(|_| AppError::message("recover recording", "data length does not fit"))?;
    let repaired = pcm_header(data_u32);
    file.seek(SeekFrom::Start(0))
        .map_err(|source| AppError::io("recover recording", source))?;
    file.write_all(&repaired)
        .map_err(|source| AppError::io("recover recording", source))?;
    file.set_len(WAV_HEADER_BYTES + data_bytes)
        .map_err(|source| AppError::io("recover recording", source))?;
    file.sync_all()
        .map_err(|source| AppError::io("recover recording", source))?;
    drop(file);
    let recovered = recovered_path(partial)?;
    std::fs::rename(partial, &recovered)
        .map_err(|source| AppError::io("recover recording", source))?;
    Ok(())
}

fn header_is_canonical(header: &[u8; 44]) -> bool {
    let expected = pcm_header(0);
    &header[0..4] == b"RIFF" && &header[8..12] == b"WAVE" && header[8..40] == expected[8..40]
}

fn recovered_path(partial: &Path) -> Result<PathBuf, AppError> {
    let name = partial
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| AppError::message("recover recording", "partial name is not unicode"))?;
    let stem = name
        .strip_suffix(".wav.partial")
        .ok_or_else(|| AppError::message("recover recording", "partial name is unexpected"))?;
    let directory = partial.parent().unwrap_or(Path::new("."));
    for suffix in 0..10_000 {
        let file_name = if suffix == 0 {
            format!("{stem}.recovered.wav")
        } else {
            format!("{stem}-{}.recovered.wav", suffix + 1)
        };
        let candidate = directory.join(file_name);
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(AppError::message(
        "recover recording",
        "could not choose a recovered name",
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
        data_bytes_fit, quantize_sample, read_wav_lengths, recover_partials, WavWrite, WavWriter,
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
    fn recoverable_partial_is_renamed_and_a_bad_header_is_left() {
        // Arrange
        let dir = temp_dir("recover");
        let partial = dir.join("Skrivli_2026-11-04_13-01-02.wav.partial");
        let mut bytes = super::pcm_header(0).to_vec();
        bytes.extend_from_slice(&[1, 0, 2, 0, 3, 0, 4, 0]);
        fs::write(&partial, &bytes).expect("partial");
        let garbage = dir.join("Skrivli_bad.wav.partial");
        fs::write(&garbage, b"not a wav header at all, still too short").expect("garbage");

        // Act
        let report = recover_partials(&dir);

        // Assert
        assert_eq!(report.recovered, 1);
        assert_eq!(report.unrecoverable, 1);
        assert!(!partial.exists());
        assert!(garbage.exists());
        let recovered = dir.join("Skrivli_2026-11-04_13-01-02.recovered.wav");
        let (riff, data, length) = read_wav_lengths(&recovered).expect("recovered");
        assert_eq!(data, 8);
        assert_eq!(riff, 44);
        assert_eq!(length, 52);
        let _ = fs::remove_dir_all(dir);
    }
}
