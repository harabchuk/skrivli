use std::fs::OpenOptions;
use std::io::Write;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::{ReplaceFileW, REPLACEFILE_IGNORE_MERGE_ERRORS};

use crate::common::error::AppError;

pub(crate) const SCHEMA_VERSION: u32 = 3;

fn default_mic_gain() -> f32 {
    1.0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ThemePreference {
    System,
    Light,
    Dark,
}

/// Logical-pixel window placement. Zero or negative sizes are rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StoredWindowBounds {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

impl StoredWindowBounds {
    pub(crate) fn new(x: i32, y: i32, width: i32, height: i32) -> Option<Self> {
        if width <= 0 || height <= 0 {
            return None;
        }
        Some(Self {
            x,
            y,
            width,
            height,
        })
    }

    pub(crate) fn x(self) -> i32 {
        self.x
    }

    pub(crate) fn y(self) -> i32 {
        self.y
    }

    pub(crate) fn width(self) -> i32 {
        self.width
    }

    pub(crate) fn height(self) -> i32 {
        self.height
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "mode", content = "id", rename_all = "snake_case")]
pub(crate) enum StoredEndpoint {
    #[default]
    Default,
    Explicit(String),
}

impl StoredEndpoint {
    pub(crate) fn explicit(id: impl Into<String>) -> Self {
        let id = id.into();
        if id.trim().is_empty() {
            Self::Default
        } else {
            Self::Explicit(id)
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Settings {
    schema_version: u32,
    recordings_directory: PathBuf,
    close_to_tray: bool,
    start_hidden: bool,
    theme: ThemePreference,
    #[serde(default)]
    window_bounds: Option<StoredWindowBounds>,
    #[serde(default)]
    tray_explanation_seen: bool,
    #[serde(default)]
    microphone: StoredEndpoint,
    #[serde(default)]
    system_audio: StoredEndpoint,
    #[serde(default = "default_mic_gain")]
    mic_gain: f32,
    #[serde(default)]
    agc_enabled: bool,
}

impl Settings {
    pub(crate) fn defaults(recordings_directory: PathBuf) -> Result<Self, AppError> {
        if recordings_directory.as_os_str().is_empty() {
            return Err(AppError::message(
                "settings",
                "recordings directory is empty",
            ));
        }
        Ok(Self {
            schema_version: SCHEMA_VERSION,
            recordings_directory,
            close_to_tray: true,
            start_hidden: false,
            theme: ThemePreference::System,
            window_bounds: None,
            tray_explanation_seen: false,
            microphone: StoredEndpoint::Default,
            system_audio: StoredEndpoint::Default,
            mic_gain: default_mic_gain(),
            agc_enabled: false,
        })
    }

    pub(crate) fn recordings_directory(&self) -> &Path {
        &self.recordings_directory
    }

    pub(crate) fn close_to_tray(&self) -> bool {
        self.close_to_tray
    }

    pub(crate) fn start_hidden(&self) -> bool {
        self.start_hidden
    }

    pub(crate) fn theme(&self) -> ThemePreference {
        self.theme
    }

    pub(crate) fn window_bounds(&self) -> Option<StoredWindowBounds> {
        self.window_bounds
    }

    pub(crate) fn tray_explanation_seen(&self) -> bool {
        self.tray_explanation_seen
    }

    pub(crate) fn set_close_to_tray(&mut self, close_to_tray: bool) {
        self.close_to_tray = close_to_tray;
    }

    pub(crate) fn set_tray_explanation_seen(&mut self, seen: bool) {
        self.tray_explanation_seen = seen;
    }

    pub(crate) fn set_window_bounds(&mut self, bounds: Option<StoredWindowBounds>) {
        self.window_bounds = bounds;
    }

    pub(crate) fn microphone(&self) -> &StoredEndpoint {
        &self.microphone
    }

    pub(crate) fn system_audio(&self) -> &StoredEndpoint {
        &self.system_audio
    }

    pub(crate) fn set_microphone(&mut self, microphone: StoredEndpoint) {
        self.microphone = microphone;
    }

    pub(crate) fn set_system_audio(&mut self, system_audio: StoredEndpoint) {
        self.system_audio = system_audio;
    }

    pub(crate) fn mic_gain(&self) -> f32 {
        self.mic_gain
    }

    pub(crate) fn set_mic_gain(&mut self, gain: f32) {
        self.mic_gain = crate::domain::gain::clamp_slider_gain(gain);
    }

    pub(crate) fn agc_enabled(&self) -> bool {
        self.agc_enabled
    }

    pub(crate) fn set_agc_enabled(&mut self, enabled: bool) {
        self.agc_enabled = enabled;
    }

    pub(crate) fn set_recordings_directory(&mut self, directory: PathBuf) -> Result<(), AppError> {
        if directory.as_os_str().is_empty() {
            return Err(AppError::message(
                "recordings folder",
                "recordings directory is empty",
            ));
        }
        self.recordings_directory = directory;
        Ok(())
    }

    /// Missing, malformed, partial, or newer-than-supported files yield `defaults`.
    pub(crate) fn load(path: &Path, defaults: Self) -> Self {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(_) => return defaults,
        };
        let value: serde_json::Value = match serde_json::from_slice(&bytes) {
            Ok(value) => value,
            Err(_) => return defaults,
        };
        let Some(version) = value.get("schema_version").and_then(|v| v.as_u64()) else {
            return defaults;
        };
        // v1 files gain endpoint defaults. A newer file is left untouched.
        if version == 0 || version > u64::from(SCHEMA_VERSION) {
            return defaults;
        }
        match serde_json::from_value::<Settings>(value) {
            Ok(settings) => normalize(settings).unwrap_or(defaults),
            _ => defaults,
        }
    }

    pub(crate) fn save(&self, path: &Path) -> Result<(), AppError> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .map_err(|source| AppError::io("save settings", source))?;
            }
        }
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|error| AppError::message("save settings", error.to_string()))?;
        let temp = temp_sibling(path);
        {
            let mut file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&temp)
                .map_err(|source| AppError::io("save settings", source))?;
            file.write_all(&bytes)
                .map_err(|source| AppError::io("save settings", source))?;
            file.sync_all()
                .map_err(|source| AppError::io("save settings", source))?;
        }
        replace_file(&temp, path)
    }
}

fn normalize(mut settings: Settings) -> Option<Settings> {
    if settings.schema_version == 0 || settings.schema_version > SCHEMA_VERSION {
        return None;
    }
    if settings.recordings_directory.as_os_str().is_empty() {
        return None;
    }
    if settings
        .window_bounds
        .is_some_and(|bounds| bounds.width <= 0 || bounds.height <= 0)
    {
        return None;
    }
    settings.microphone = clean_endpoint(settings.microphone);
    settings.system_audio = clean_endpoint(settings.system_audio);
    settings.mic_gain = crate::domain::gain::clamp_slider_gain(settings.mic_gain);
    settings.schema_version = SCHEMA_VERSION;
    Some(settings)
}

fn clean_endpoint(endpoint: StoredEndpoint) -> StoredEndpoint {
    match endpoint {
        StoredEndpoint::Explicit(id) => StoredEndpoint::explicit(id),
        StoredEndpoint::Default => StoredEndpoint::Default,
    }
}

fn temp_sibling(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_else(|| std::ffi::OsString::from("settings.json"));
    name.push(".tmp");
    path.with_file_name(name)
}

/// `std::fs::rename` fails on Windows when the destination already exists.
/// `ReplaceFileW` replaces that destination and removes the temp name.
fn replace_file(replacement: &Path, replaced: &Path) -> Result<(), AppError> {
    if !replaced.exists() {
        return std::fs::rename(replacement, replaced)
            .map_err(|source| AppError::io("save settings", source));
    }
    let replacement_wide = wide_path(replacement);
    let replaced_wide = wide_path(replaced);
    // SAFETY: both buffers are NUL-terminated and point at files in the same
    // directory. The temp file is closed. ReplaceFileW moves it onto `replaced`.
    let result = unsafe {
        ReplaceFileW(
            PCWSTR(replaced_wide.as_ptr()),
            PCWSTR(replacement_wide.as_ptr()),
            PCWSTR::null(),
            REPLACEFILE_IGNORE_MERGE_ERRORS,
            None,
            None,
        )
    };
    result.map_err(|error| AppError::message("save settings", error.to_string()))
}

fn wide_path(path: &Path) -> Vec<u16> {
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::{Settings, StoredEndpoint, StoredWindowBounds, ThemePreference, SCHEMA_VERSION};

    fn temp_dir(label: &str) -> std::path::PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "skrivli-settings-{label}-{}-{id}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn sample(dir: &std::path::Path) -> Settings {
        Settings::defaults(dir.join("Recordings")).expect("defaults")
    }

    #[test]
    fn round_trip_preserves_fields() {
        // Arrange
        let dir = temp_dir("round");
        let path = dir.join("settings.json");
        let mut settings = sample(&dir);
        settings.start_hidden = true;
        settings.close_to_tray = false;
        settings.theme = ThemePreference::Dark;
        settings.recordings_directory = dir.join("Meetings");

        // Act
        settings.save(&path).expect("save");
        let loaded = Settings::load(&path, sample(&dir));

        // Assert
        assert_eq!(loaded, settings);
        assert!(!dir.join("settings.json.tmp").exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn round_trip_preserves_bounds_and_tray_explanation() {
        // Arrange
        let dir = temp_dir("bounds");
        let path = dir.join("settings.json");
        let mut settings = sample(&dir);
        let bounds = StoredWindowBounds::new(-40, 12, 640, 480).expect("bounds");
        settings.set_window_bounds(Some(bounds));
        settings.set_tray_explanation_seen(true);
        settings.set_close_to_tray(false);

        // Act
        settings.save(&path).expect("save");
        let loaded = Settings::load(&path, sample(&dir));

        // Assert
        assert_eq!(loaded, settings);
        assert_eq!(loaded.window_bounds(), Some(bounds));
        assert!(loaded.tray_explanation_seen());
        assert!(!loaded.close_to_tray());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn malformed_json_falls_back_to_defaults() {
        // Arrange
        let dir = temp_dir("malformed");
        let path = dir.join("settings.json");
        fs::write(&path, b"{not json").expect("write");
        let defaults = sample(&dir);

        // Act
        let loaded = Settings::load(&path, defaults.clone());

        // Assert
        assert_eq!(loaded, defaults);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn corrupt_file_still_loads_defaults() {
        // Arrange
        let dir = temp_dir("corrupt");
        let path = dir.join("settings.json");
        let garbage = [0xff_u8, 0xfe, 0x00, b'{'];
        fs::write(&path, garbage).expect("write");
        let defaults = sample(&dir);

        // Act
        let first = Settings::load(&path, defaults.clone());
        let second = Settings::load(&path, defaults.clone());

        // Assert
        assert_eq!(first, defaults);
        assert_eq!(second, defaults);
        assert_eq!(fs::read(&path).expect("still there"), garbage);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn future_schema_version_falls_back_without_crashing() {
        // Arrange
        let dir = temp_dir("future");
        let path = dir.join("settings.json");
        fs::write(
            &path,
            r#"{"schema_version":99,"recordings_directory":"C:\\secret","close_to_tray":false,"start_hidden":true,"theme":"dark"}"#,
        )
        .expect("write");
        let defaults = sample(&dir);

        // Act
        let loaded = Settings::load(&path, defaults.clone());

        // Assert
        assert_eq!(loaded, defaults);
        assert_ne!(
            loaded.recordings_directory(),
            std::path::Path::new(r"C:\secret")
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn current_version_migrates_as_identity() {
        // Arrange
        let dir = temp_dir("migrate");
        let path = dir.join("settings.json");
        let meetings = dir.join("Meetings");
        let body = format!(
            r#"{{"schema_version":1,"recordings_directory":"{}","close_to_tray":false,"start_hidden":true,"theme":"light","extra":1}}"#,
            meetings.display().to_string().replace('\\', "\\\\")
        );
        fs::write(&path, body).expect("write");
        let defaults = sample(&dir);

        // Act
        let loaded = Settings::load(&path, defaults);

        // Assert
        assert_eq!(loaded.schema_version, SCHEMA_VERSION);
        assert_eq!(loaded.microphone(), &StoredEndpoint::Default);
        assert_eq!(loaded.system_audio(), &StoredEndpoint::Default);
        assert!((loaded.mic_gain() - 1.0).abs() < 0.001);
        assert!(!loaded.agc_enabled());
        let mut updated = loaded.clone();
        updated.set_system_audio(StoredEndpoint::explicit("{render}"));
        assert!(matches!(
            updated.system_audio(),
            StoredEndpoint::Explicit(_)
        ));
        assert_eq!(loaded.recordings_directory, meetings);
        assert!(!loaded.close_to_tray);
        assert!(loaded.start_hidden);
        assert_eq!(loaded.theme, ThemePreference::Light);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn atomic_save_replaces_existing_file() {
        // Arrange
        let dir = temp_dir("replace");
        let path = dir.join("settings.json");
        let original = sample(&dir);
        original.save(&path).expect("first save");
        let mut updated = original.clone();
        updated.start_hidden = true;

        // Act
        updated.save(&path).expect("replace");
        let loaded = Settings::load(&path, sample(&dir));

        // Assert
        assert!(loaded.start_hidden);
        assert_eq!(loaded.recordings_directory, original.recordings_directory);
        let raw = fs::read(&path).expect("readable");
        assert!(serde_json::from_slice::<Settings>(&raw).is_ok());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn truncated_object_falls_back_to_defaults() {
        // Arrange
        let dir = temp_dir("partial");
        let path = dir.join("settings.json");
        fs::write(&path, br#"{"schema_version":1,"recordings_directory":"#).expect("write");
        let defaults = sample(&dir);

        // Act
        let loaded = Settings::load(&path, defaults.clone());

        // Assert
        assert_eq!(loaded, defaults);
        let _ = fs::remove_dir_all(dir);
    }
}
