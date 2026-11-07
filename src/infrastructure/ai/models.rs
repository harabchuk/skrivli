//! Discover, validate, and download local whisper.cpp GGML model files.
//!
//! Downloads run only from an explicit caller (never bootstrap). A failed or
//! cancelled download leaves no accepted model file.

#![allow(dead_code)] // Consumed by M6.1 UI / M6.3 worker once wired end-to-end.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::common::error::AppError;
use crate::infrastructure::platform;

/// Checked-in default variant id (quantized multilingual base).
pub(crate) const DEFAULT_VARIANT_ID: &str = "base-q5_1";

const MANIFEST_JSON: &str = include_str!("../../../resources/models/manifest.json");
/// whisper.cpp reads a little-endian `u32` 0x67676d6c, so the bytes on disk are `lmgg`.
const GGML_MAGIC: [u8; 4] = 0x6767_6d6c_u32.to_le_bytes();
const DOWNLOAD_CHUNK: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct ModelManifest {
    schema_version: u32,
    source_base: String,
    license: String,
    models: Vec<ManifestEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct ManifestEntry {
    id: String,
    filename: String,
    bytes: u64,
    sha256: String,
    #[serde(default)]
    default: bool,
}

impl ManifestEntry {
    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    pub(crate) fn filename(&self) -> &str {
        &self.filename
    }

    pub(crate) fn bytes(&self) -> u64 {
        self.bytes
    }

    pub(crate) fn sha256(&self) -> &str {
        &self.sha256
    }

    pub(crate) fn is_default(&self) -> bool {
        self.default
    }

    pub(crate) fn download_url(&self, source_base: &str) -> String {
        let base = source_base.trim_end_matches('/');
        format!("{base}/{}", self.filename)
    }
}

impl ModelManifest {
    pub(crate) fn load_bundled() -> Result<Self, AppError> {
        Self::parse(MANIFEST_JSON)
    }

    pub(crate) fn parse(json: &str) -> Result<Self, AppError> {
        serde_json::from_str(json)
            .map_err(|error| AppError::message("model manifest", error.to_string()))
    }

    pub(crate) fn license(&self) -> &str {
        &self.license
    }

    pub(crate) fn source_base(&self) -> &str {
        &self.source_base
    }

    pub(crate) fn entries(&self) -> &[ManifestEntry] {
        &self.models
    }

    pub(crate) fn default_entry(&self) -> Option<&ManifestEntry> {
        self.models.iter().find(|entry| entry.default).or_else(|| {
            self.models
                .iter()
                .find(|entry| entry.id == DEFAULT_VARIANT_ID)
        })
    }

    pub(crate) fn by_id(&self, id: &str) -> Option<&ManifestEntry> {
        self.models.iter().find(|entry| entry.id == id)
    }

    pub(crate) fn by_filename(&self, filename: &str) -> Option<&ManifestEntry> {
        self.models.iter().find(|entry| entry.filename == filename)
    }
}

/// A `ggml-*.bin` found under the models directory or a user-picked path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiscoveredModel {
    pub(crate) path: PathBuf,
    pub(crate) filename: String,
    pub(crate) variant_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DownloadProgress {
    pub(crate) bytes_downloaded: u64,
    pub(crate) total_bytes: Option<u64>,
}

/// List `ggml-*.bin` files under `models_dir` and optionally one extra path
/// (a file or a directory the user chose).
pub(crate) fn discover(
    models_dir: &Path,
    extra: Option<&Path>,
    manifest: &ModelManifest,
) -> Vec<DiscoveredModel> {
    let mut found = Vec::new();
    push_directory(models_dir, manifest, &mut found);
    if let Some(path) = extra {
        if path.is_dir() {
            push_directory(path, manifest, &mut found);
        } else if is_ggml_filename(path) {
            push_file(path, manifest, &mut found);
        }
    }
    found.sort_by(|a, b| a.filename.cmp(&b.filename));
    found.dedup_by(|a, b| a.path == b.path);
    found
}

/// Confirm the path exists, is readable, and looks like a GGML model the
/// pinned engine can attempt to load. Missing / unreadable / unsupported are
/// distinct [`AppError`] contexts.
pub(crate) fn validate_model_file(path: &Path) -> Result<(), AppError> {
    if !path.is_file() {
        return Err(AppError::message(
            "model missing",
            format!("not a file: {}", path.display()),
        ));
    }
    let mut file = File::open(path).map_err(|source| AppError::io("model unreadable", source))?;
    let mut magic = [0_u8; 4];
    file.read_exact(&mut magic)
        .map_err(|source| AppError::io("model unreadable", source))?;
    if !is_ggml_magic(magic) {
        return Err(AppError::message(
            "model unsupported",
            format!("unexpected magic on {}", path.display()),
        ));
    }
    Ok(())
}

/// Copy a user-picked GGML file into `models_dir` after [`validate_model_file`].
/// A file that is already in that directory is accepted in place.
pub(crate) fn install_picked_file(source: &Path, models_dir: &Path) -> Result<PathBuf, AppError> {
    validate_model_file(source)?;
    if !is_ggml_filename(source) {
        return Err(AppError::message(
            "model unsupported",
            format!("name {}", source.display()),
        ));
    }
    fs::create_dir_all(models_dir).map_err(|err| AppError::io("download model", err))?;
    let Some(name) = source.file_name() else {
        return Err(AppError::message(
            "model unsupported",
            "picked path has no file name",
        ));
    };
    let destination = models_dir.join(name);
    if source == destination {
        return Ok(destination);
    }
    let temp = temp_sibling(&destination);
    fs::copy(source, &temp).map_err(|err| AppError::io("model unreadable", err))?;
    match platform::replace_file(&temp, &destination, "download model") {
        Ok(()) => Ok(destination),
        Err(error) => {
            let _ = fs::remove_file(&temp);
            Err(error)
        }
    }
}

/// Resolve the on-disk path for a manifest variant under `models_dir`.
pub(crate) fn path_for_variant(models_dir: &Path, entry: &ManifestEntry) -> PathBuf {
    models_dir.join(&entry.filename)
}

/// Download a manifest variant into `models_dir` after an explicit user action.
/// Writes a temp sibling, reports progress, honors cancel, verifies SHA-256,
/// and promotes with [`platform::replace_file`] only on match.
pub(crate) fn download_variant(
    entry: &ManifestEntry,
    source_base: &str,
    models_dir: &Path,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(DownloadProgress),
) -> Result<PathBuf, AppError> {
    fs::create_dir_all(models_dir).map_err(|source| AppError::io("download model", source))?;
    let destination = path_for_variant(models_dir, entry);
    let url = entry.download_url(source_base);
    let response = ureq::get(&url)
        .call()
        .map_err(|error| AppError::message("download model", error.to_string()))?;
    let total = response
        .header("Content-Length")
        .and_then(|value| value.parse::<u64>().ok())
        .or(Some(entry.bytes));
    let mut reader = response.into_reader();
    install_from_reader(
        &mut reader,
        &destination,
        entry.bytes,
        entry.sha256(),
        total,
        cancel,
        on_progress,
    )?;
    Ok(destination)
}

/// Testable install path: stream bytes to a temp sibling, hash, promote.
pub(crate) fn install_from_reader(
    reader: &mut dyn Read,
    destination: &Path,
    expected_bytes: u64,
    expected_sha256_hex: &str,
    total_hint: Option<u64>,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(DownloadProgress),
) -> Result<(), AppError> {
    if let Some(parent) = destination.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|source| AppError::io("download model", source))?;
        }
    }
    let temp = temp_sibling(destination);
    let outcome = write_temp_hashed(
        reader,
        &temp,
        expected_bytes,
        expected_sha256_hex,
        total_hint,
        cancel,
        on_progress,
    );
    match outcome {
        Ok(()) => {
            platform::replace_file(&temp, destination, "download model")?;
            Ok(())
        }
        Err(error) => {
            let _ = fs::remove_file(&temp);
            Err(error)
        }
    }
}

fn write_temp_hashed(
    reader: &mut dyn Read,
    temp: &Path,
    expected_bytes: u64,
    expected_sha256_hex: &str,
    total_hint: Option<u64>,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(DownloadProgress),
) -> Result<(), AppError> {
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(temp)
        .map_err(|source| AppError::io("download model", source))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; DOWNLOAD_CHUNK];
    let mut downloaded = 0_u64;
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err(AppError::message("download cancelled", "cancel flag set"));
        }
        let read = reader
            .read(&mut buffer)
            .map_err(|source| AppError::io("download model", source))?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read])
            .map_err(|source| AppError::io("download model", source))?;
        hasher.update(&buffer[..read]);
        downloaded = downloaded.saturating_add(read as u64);
        on_progress(DownloadProgress {
            bytes_downloaded: downloaded,
            total_bytes: total_hint.or(Some(expected_bytes)),
        });
    }
    file.sync_all()
        .map_err(|source| AppError::io("download model", source))?;
    drop(file);

    if downloaded != expected_bytes {
        return Err(AppError::message(
            "verify model",
            format!("size {downloaded} != expected {expected_bytes}"),
        ));
    }
    let digest = hex::encode(hasher.finalize());
    if !digest.eq_ignore_ascii_case(expected_sha256_hex) {
        return Err(AppError::message(
            "verify model",
            format!("sha256 mismatch (got {digest})"),
        ));
    }
    Ok(())
}

fn push_directory(dir: &Path, manifest: &ModelManifest, out: &mut Vec<DiscoveredModel>) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() && is_ggml_filename(&path) {
            push_file(&path, manifest, out);
        }
    }
}

fn push_file(path: &Path, manifest: &ModelManifest, out: &mut Vec<DiscoveredModel>) {
    let Some(filename) = path.file_name().and_then(|name| name.to_str()) else {
        return;
    };
    let variant_id = manifest.by_filename(filename).map(|entry| entry.id.clone());
    out.push(DiscoveredModel {
        path: path.to_path_buf(),
        filename: filename.to_string(),
        variant_id,
    });
}

fn is_ggml_filename(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            let lower = name.to_ascii_lowercase();
            lower.starts_with("ggml-") && lower.ends_with(".bin")
        })
}

fn is_ggml_magic(magic: [u8; 4]) -> bool {
    magic == GGML_MAGIC
}

fn temp_sibling(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_else(|| std::ffi::OsString::from("model.bin"));
    name.push(".tmp");
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::{
        discover, install_from_reader, validate_model_file, DownloadProgress, ModelManifest,
        DEFAULT_VARIANT_ID, GGML_MAGIC,
    };
    use sha2::{Digest, Sha256};
    use std::io::Cursor;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    fn temp_dir(label: &str) -> std::path::PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "skrivli-models-{label}-{}-{id}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn bundled() -> ModelManifest {
        ModelManifest::load_bundled().expect("manifest")
    }

    #[test]
    fn bundled_manifest_pins_verified_variants() {
        // Arrange / Act
        let manifest = bundled();
        let default = manifest.default_entry().expect("default");

        // Assert
        assert_eq!(manifest.license(), "MIT");
        assert!(manifest
            .source_base()
            .starts_with("https://huggingface.co/ggerganov/whisper.cpp/"));
        assert_eq!(default.id(), DEFAULT_VARIANT_ID);
        assert_eq!(default.bytes(), 59_707_625);
        assert_eq!(
            default.sha256(),
            "422f1ae452ade6f30a004d7e5c6a43195e4433bc370bf23fac9cc591f01a8898"
        );
        assert_eq!(manifest.entries().len(), 3);
        assert!(manifest.by_id("tiny-q5_1").is_some());
        assert!(manifest.by_id("small-q5_1").is_some());
    }

    #[test]
    fn discover_finds_ggml_files_and_maps_manifest_ids() {
        // Arrange
        let dir = temp_dir("discover");
        let base = dir.join("ggml-base-q5_1.bin");
        let other = dir.join("ggml-custom.bin");
        std::fs::write(&base, b"ggml-data").expect("write base");
        std::fs::write(&other, b"ggml-data").expect("write other");
        std::fs::write(dir.join("readme.txt"), b"no").expect("write txt");
        let manifest = bundled();

        // Act
        let found = discover(&dir, None, &manifest);

        // Assert
        assert_eq!(found.len(), 2);
        let mapped = found
            .iter()
            .find(|item| item.filename == "ggml-base-q5_1.bin")
            .expect("base");
        assert_eq!(mapped.variant_id.as_deref(), Some("base-q5_1"));
        let custom = found
            .iter()
            .find(|item| item.filename == "ggml-custom.bin")
            .expect("custom");
        assert!(custom.variant_id.is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn validate_accepts_real_whisper_header_bytes() {
        // Arrange: first bytes of a real ggml-base-q5_1.bin (u32 0x67676d6c, little-endian).
        let dir = temp_dir("validate-real");
        let path = dir.join("ggml-real.bin");
        std::fs::write(&path, [0x6c, 0x6d, 0x67, 0x67, 0x99, 0xca, 0x00, 0x00]).expect("write");

        // Act
        let result = validate_model_file(&path);

        // Assert
        assert!(result.is_ok());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn validate_rejects_text_ggml_prefix() {
        // Arrange
        let dir = temp_dir("validate-text");
        let path = dir.join("ggml-text.bin");
        std::fs::write(&path, b"ggml-not-a-model").expect("write");

        // Act
        let error = validate_model_file(&path).expect_err("unsupported");

        // Assert
        assert_eq!(
            error.user_message(),
            "The speech model file is not supported."
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn validate_distinguishes_missing_unreadable_unsupported() {
        // Arrange
        let dir = temp_dir("validate");
        let missing = dir.join("ggml-missing.bin");
        let good = dir.join("ggml-good.bin");
        let bad = dir.join("ggml-bad.bin");
        let mut good_bytes = Vec::from(GGML_MAGIC);
        good_bytes.extend_from_slice(b"rest");
        std::fs::write(&good, &good_bytes).expect("good");
        std::fs::write(&bad, b"XXXX").expect("bad");

        // Act
        let missing_err = validate_model_file(&missing).expect_err("missing");
        let unsupported = validate_model_file(&bad).expect_err("unsupported");
        validate_model_file(&good).expect("good");

        // Assert
        assert_eq!(
            missing_err.user_message(),
            "The speech model file is missing."
        );
        assert_eq!(
            unsupported.user_message(),
            "The speech model file is not supported."
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn install_promotes_only_when_hash_and_size_match() {
        // Arrange
        let dir = temp_dir("install-ok");
        let dest = dir.join("ggml-base-q5_1.bin");
        let payload = b"ggml-payload-for-hash";
        let digest = hex::encode(Sha256::digest(payload));
        let cancel = AtomicBool::new(false);
        let progress = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&progress);

        // Act
        install_from_reader(
            &mut Cursor::new(payload.as_slice()),
            &dest,
            payload.len() as u64,
            &digest,
            Some(payload.len() as u64),
            &cancel,
            &mut |update: DownloadProgress| {
                sink.lock().expect("lock").push(update.bytes_downloaded);
            },
        )
        .expect("install");

        // Assert
        assert_eq!(std::fs::read(&dest).expect("read"), payload);
        assert!(!dir.join("ggml-base-q5_1.bin.tmp").exists());
        assert!(!progress.lock().expect("lock").is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn failed_hash_leaves_no_accepted_model() {
        // Arrange
        let dir = temp_dir("install-bad");
        let dest = dir.join("ggml-base-q5_1.bin");
        let cancel = AtomicBool::new(false);

        // Act
        let error = install_from_reader(
            &mut Cursor::new(b"ggml-wrong"),
            &dest,
            10,
            "0000000000000000000000000000000000000000000000000000000000000000",
            Some(10),
            &cancel,
            &mut |_| {},
        )
        .expect_err("hash");

        // Assert
        assert_eq!(
            error.user_message(),
            "The downloaded speech model could not be verified."
        );
        assert!(!dest.exists());
        assert!(!dir.join("ggml-base-q5_1.bin.tmp").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn cancel_leaves_no_accepted_model() {
        // Arrange
        let dir = temp_dir("install-cancel");
        let dest = dir.join("ggml-tiny-q5_1.bin");
        let cancel = AtomicBool::new(true);

        // Act
        let error = install_from_reader(
            &mut Cursor::new(b"ggml-should-not-land"),
            &dest,
            20,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            Some(20),
            &cancel,
            &mut |_| {},
        )
        .expect_err("cancel");

        // Assert
        assert_eq!(error.user_message(), "The model download was cancelled.");
        assert!(!dest.exists());
        assert!(!dir.join("ggml-tiny-q5_1.bin.tmp").exists());
        let _ = std::fs::remove_dir_all(dir);
    }
}
