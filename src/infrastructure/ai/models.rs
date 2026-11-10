//! Discover, validate, and download local speech model files (GGUF).
//!
//! Downloads run only from an explicit caller (never bootstrap). A failed or
//! cancelled download leaves no accepted model file.

#![allow(dead_code)] // Consumed by M6.1 UI / M6.3 worker once wired end-to-end.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::common::error::AppError;
use crate::infrastructure::platform;

/// Checked-in default variant id (Nemotron streaming, Q8_0).
pub(crate) const DEFAULT_VARIANT_ID: &str = "nemotron-3.5-streaming-0.6b-q8_0";

const MANIFEST_JSON: &str = include_str!("../../../resources/models/manifest.json");
/// GGUF files start with the little-endian `u32` 0x46554747, which is the bytes `GGUF`.
const GGUF_MAGIC: [u8; 4] = 0x4655_4747_u32.to_le_bytes();
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
    /// Full download address, for a model hosted outside `source_base`.
    #[serde(default)]
    url: Option<String>,
    /// Overrides the manifest-wide license.
    #[serde(default)]
    license: Option<String>,
    name: String,
    description: String,
    /// General comparisons from 1 to 3, not measured on this PC.
    speed: u8,
    accuracy: u8,
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

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn description(&self) -> &str {
        &self.description
    }

    /// (speed, accuracy), each out of 3.
    pub(crate) fn ratings(&self) -> (u8, u8) {
        (self.speed, self.accuracy)
    }

    pub(crate) fn license_override(&self) -> Option<&str> {
        self.license.as_deref()
    }

    pub(crate) fn download_url(&self, source_base: &str) -> String {
        if let Some(url) = &self.url {
            return url.clone();
        }
        let base = source_base.trim_end_matches('/');
        format!("{base}/{}", self.filename)
    }
}

impl ModelManifest {
    pub(crate) fn load_bundled() -> Result<Self, AppError> {
        Self::parse(MANIFEST_JSON)
    }

    pub(crate) fn parse(json: &str) -> Result<Self, AppError> {
        let manifest: Self = serde_json::from_str(json)
            .map_err(|error| AppError::message("model manifest", error.to_string()))?;
        manifest.validate()?;
        Ok(manifest)
    }

    fn validate(&self) -> Result<(), AppError> {
        let mut ids = HashSet::new();
        let mut filenames = HashSet::new();
        for entry in &self.models {
            if !ids.insert(entry.id.as_str()) || !filenames.insert(entry.filename.as_str()) {
                return Err(AppError::message(
                    "model manifest",
                    format!("duplicate id or filename at {}", entry.id),
                ));
            }
            if !(1..=3).contains(&entry.speed) || !(1..=3).contains(&entry.accuracy) {
                return Err(AppError::message(
                    "model manifest",
                    format!("rating out of range at {}", entry.id),
                ));
            }
        }
        Ok(())
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

/// A `*.gguf` file found under the models directory or a user-picked path.
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

/// List `*.gguf` files under `models_dir` and optionally one extra path
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
        } else if is_gguf_filename(path) {
            push_file(path, manifest, &mut found);
        }
    }
    found.sort_by(|a, b| a.filename.cmp(&b.filename));
    found.dedup_by(|a, b| a.path == b.path);
    found
}

/// Confirm the path exists, is readable, and looks like a GGUF model the
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
    if !is_gguf_magic(magic) {
        return Err(AppError::message(
            "model unsupported",
            format!("unexpected magic on {}", path.display()),
        ));
    }
    Ok(())
}

/// Copy a user-picked GGUF file into `models_dir` after [`validate_model_file`].
/// A file that is already in that directory is accepted in place.
pub(crate) fn install_picked_file(source: &Path, models_dir: &Path) -> Result<PathBuf, AppError> {
    validate_model_file(source)?;
    if !is_gguf_filename(source) {
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
        if path.is_file() && is_gguf_filename(&path) {
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

fn is_gguf_filename(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            let lower = name.to_ascii_lowercase();
            lower.ends_with(".gguf")
        })
}

fn is_gguf_magic(magic: [u8; 4]) -> bool {
    magic == GGUF_MAGIC
}

fn temp_sibling(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_else(|| std::ffi::OsString::from("model.gguf"));
    name.push(".tmp");
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::{
        discover, install_from_reader, validate_model_file, DownloadProgress, ModelManifest,
        DEFAULT_VARIANT_ID, GGUF_MAGIC,
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
        assert_eq!(manifest.license(), "OpenMDW-1.1");
        assert!(manifest
            .source_base()
            .starts_with("https://huggingface.co/mudler/parakeet-cpp-gguf/resolve/"));
        assert_eq!(default.id(), DEFAULT_VARIANT_ID);
        assert_eq!(
            default.filename(),
            "nemotron-3.5-asr-streaming-0.6b-q8_0.gguf"
        );
        assert_eq!(default.bytes(), 983_696_512);
        assert_eq!(
            default.sha256(),
            "ba2f13eccd4a5245be728f77e6149bd6a4fdcdd133ff2e08ac6005bcef7a99f1"
        );
        assert_eq!(manifest.entries().len(), 1);
    }

    #[test]
    fn bundled_download_address_is_pinned_to_a_commit() {
        // Arrange
        let manifest = bundled();
        let entry = manifest.default_entry().expect("default");

        // Act
        let url = entry.download_url(manifest.source_base());

        // Assert
        assert_eq!(
            url,
            "https://huggingface.co/mudler/parakeet-cpp-gguf/resolve/\
             07a08a3a405d20c5434b9a1442f6e6cf571f57ad/\
             nemotron-3.5-asr-streaming-0.6b-q8_0.gguf"
        );
    }

    #[test]
    fn entry_with_its_own_url_ignores_the_source_base() {
        // Arrange
        let json = r#"{"schema_version":3,"source_base":"https://example.invalid/base/",
            "license":"MIT","models":[
              {"id":"a","filename":"a.gguf","bytes":1,"sha256":"00",
               "url":"https://elsewhere.invalid/x/a.gguf",
               "name":"A","description":"a","speed":1,"accuracy":1},
              {"id":"b","filename":"b.gguf","bytes":1,"sha256":"00",
               "name":"B","description":"b","speed":1,"accuracy":1}
            ]}"#;
        let manifest = ModelManifest::parse(json).expect("manifest");

        // Act
        let own = manifest
            .by_id("a")
            .expect("a")
            .download_url(manifest.source_base());
        let shared = manifest
            .by_id("b")
            .expect("b")
            .download_url(manifest.source_base());

        // Assert
        assert_eq!(own, "https://elsewhere.invalid/x/a.gguf");
        assert_eq!(shared, "https://example.invalid/base/b.gguf");
    }

    fn manifest_json(second_filename: &str, speed: u8) -> String {
        format!(
            r#"{{"schema_version":3,"source_base":"https://example.invalid/","license":"MIT",
            "models":[
              {{"id":"a","filename":"a.gguf","bytes":1,"sha256":"00",
                "name":"A","description":"a","speed":1,"accuracy":1}},
              {{"id":"b","filename":"{second_filename}","bytes":1,"sha256":"00",
                "name":"B","description":"b","speed":{speed},"accuracy":1}}
            ]}}"#
        )
    }

    #[test]
    fn manifest_accepts_valid_entries() {
        // Arrange
        let json = manifest_json("b.gguf", 3);

        // Act
        let parsed = ModelManifest::parse(&json);

        // Assert
        assert!(parsed.is_ok());
    }

    #[test]
    fn manifest_rejects_two_models_sharing_a_local_file() {
        // Arrange
        let json = manifest_json("a.gguf", 1);

        // Act
        let parsed = ModelManifest::parse(&json);

        // Assert
        assert!(parsed.is_err());
    }

    #[test]
    fn manifest_rejects_a_rating_outside_one_to_three() {
        // Arrange
        let json = manifest_json("b.gguf", 4);

        // Act
        let parsed = ModelManifest::parse(&json);

        // Assert
        assert!(parsed.is_err());
    }

    #[test]
    fn discovers_only_gguf_files() {
        // Arrange
        let dir = temp_dir("discover");
        let known = dir.join("nemotron-3.5-asr-streaming-0.6b-q8_0.gguf");
        let custom = dir.join("custom.GGUF");
        std::fs::write(&known, b"GGUF-data").expect("write known");
        std::fs::write(&custom, b"GGUF-data").expect("write custom");
        std::fs::write(dir.join("ggml-base-q5_1.bin"), b"lmgg-data").expect("write old");
        std::fs::write(dir.join("readme.txt"), b"no").expect("write txt");
        let manifest = bundled();

        // Act
        let found = discover(&dir, None, &manifest);

        // Assert
        assert_eq!(found.len(), 2);
        let mapped = found
            .iter()
            .find(|item| item.filename == "nemotron-3.5-asr-streaming-0.6b-q8_0.gguf")
            .expect("known");
        assert_eq!(mapped.variant_id.as_deref(), Some(DEFAULT_VARIANT_ID));
        let other = found
            .iter()
            .find(|item| item.filename == "custom.GGUF")
            .expect("custom");
        assert!(other.variant_id.is_none());
        assert!(dir.join("ggml-base-q5_1.bin").is_file());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn validate_accepts_gguf_header_bytes() {
        // Arrange: the first bytes of a GGUF file (magic, then version 3).
        let dir = temp_dir("validate-real");
        let path = dir.join("model.gguf");
        std::fs::write(&path, [b'G', b'G', b'U', b'F', 3, 0, 0, 0]).expect("write");

        // Act
        let result = validate_model_file(&path);

        // Assert
        assert!(result.is_ok());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn rejects_ggml_magic_as_unsupported() {
        // Arrange: a whisper.cpp file header (u32 0x67676d6c, little-endian).
        let dir = temp_dir("validate-ggml");
        let path = dir.join("model.gguf");
        std::fs::write(&path, [0x6c, 0x6d, 0x67, 0x67, 0x99, 0xca, 0x00, 0x00]).expect("write");

        // Act
        let error = validate_model_file(&path).expect_err("unsupported");

        // Assert
        assert_eq!(
            error.user_message(),
            "The transcription model file is not supported."
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn validate_distinguishes_missing_unreadable_unsupported() {
        // Arrange
        let dir = temp_dir("validate");
        let missing = dir.join("missing.gguf");
        let good = dir.join("good.gguf");
        let bad = dir.join("bad.gguf");
        let mut good_bytes = Vec::from(GGUF_MAGIC);
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
            "The transcription model file is missing."
        );
        assert_eq!(
            unsupported.user_message(),
            "The transcription model file is not supported."
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn install_promotes_only_when_hash_and_size_match() {
        // Arrange
        let dir = temp_dir("install-ok");
        let dest = dir.join("model-a.gguf");
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
        assert!(!dir.join("model-a.gguf.tmp").exists());
        assert!(!progress.lock().expect("lock").is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn failed_hash_leaves_no_accepted_model() {
        // Arrange
        let dir = temp_dir("install-bad");
        let dest = dir.join("model-a.gguf");
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
            "The downloaded transcription model could not be verified."
        );
        assert!(!dest.exists());
        assert!(!dir.join("model-a.gguf.tmp").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn cancel_leaves_no_accepted_model() {
        // Arrange
        let dir = temp_dir("install-cancel");
        let dest = dir.join("model-b.gguf");
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
        assert!(!dir.join("model-b.gguf.tmp").exists());
        let _ = std::fs::remove_dir_all(dir);
    }
}
