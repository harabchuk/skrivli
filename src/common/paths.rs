use std::path::PathBuf;

const APP_DIR: &str = "Skrivli";

/// Folders derived from roaming, local, and home bases. Nothing is created here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AppPaths {
    roaming_app_data: PathBuf,
    local_app_data: PathBuf,
    home: PathBuf,
}

impl AppPaths {
    pub(crate) fn from_bases(
        roaming_app_data: impl Into<PathBuf>,
        local_app_data: impl Into<PathBuf>,
        home: impl Into<PathBuf>,
    ) -> Self {
        Self {
            roaming_app_data: roaming_app_data.into(),
            local_app_data: local_app_data.into(),
            home: home.into(),
        }
    }

    pub(crate) fn settings_file(&self) -> PathBuf {
        self.roaming_app_data.join(APP_DIR).join("settings.json")
    }

    pub(crate) fn logs_dir(&self) -> PathBuf {
        self.local_app_data.join(APP_DIR).join("logs")
    }

    pub(crate) fn models_dir(&self) -> PathBuf {
        self.local_app_data.join(APP_DIR).join("models")
    }

    pub(crate) fn recordings_dir(&self) -> PathBuf {
        self.home.join("Documents").join(APP_DIR)
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::AppPaths;

    #[test]
    fn layouts_folders_from_injected_bases() {
        // Arrange
        let paths = AppPaths::from_bases(r"C:\Roaming", r"D:\Local", r"E:\Home");

        // Act
        let settings = paths.settings_file();
        let logs = paths.logs_dir();
        let models = paths.models_dir();
        let recordings = paths.recordings_dir();

        // Assert
        assert_eq!(settings, Path::new(r"C:\Roaming\Skrivli\settings.json"));
        assert_eq!(logs, Path::new(r"D:\Local\Skrivli\logs"));
        assert_eq!(models, Path::new(r"D:\Local\Skrivli\models"));
        assert_eq!(recordings, Path::new(r"E:\Home\Documents\Skrivli"));
    }
}
