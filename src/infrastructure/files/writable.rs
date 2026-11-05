//! Probe that a recordings folder exists and accepts a short write.

use std::path::Path;

use crate::common::error::AppError;

pub(crate) fn ensure_writable(directory: &Path) -> Result<(), AppError> {
    std::fs::create_dir_all(directory)
        .map_err(|source| AppError::io("recordings folder", source))?;
    let probe = directory.join(".skrivli-write-probe");
    std::fs::write(&probe, b"ok").map_err(|source| AppError::io("recordings folder", source))?;
    let _ = std::fs::remove_file(&probe);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::ensure_writable;

    #[test]
    fn writable_directory_accepts_the_probe() {
        // Arrange
        let dir = std::env::temp_dir().join(format!(
            "skrivli-writable-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");

        // Act
        let result = ensure_writable(&dir);

        // Assert
        assert!(result.is_ok());
        let _ = std::fs::remove_dir_all(dir);
    }
}
