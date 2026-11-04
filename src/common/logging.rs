use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use tracing_subscriber::prelude::*;

use super::error::AppError;

const ACTIVE_LIMIT_BYTES: u64 = 1024 * 1024;
/// Rotated siblings kept beside the active file. Active + these two is about 1 MB × 3.
const ARCHIVE_COUNT: usize = 2;

/// Keeps the non-blocking tracing worker alive until shutdown.
pub(crate) struct LogGuard {
    _worker: tracing_appender::non_blocking::WorkerGuard,
}

pub(crate) fn init(logs_dir: &Path) -> Result<LogGuard, AppError> {
    std::fs::create_dir_all(logs_dir)
        .map_err(|source| AppError::io("create logs directory", source))?;
    let file = SizeRotatingFile::create(
        &logs_dir.join("skrivli.log"),
        ACTIVE_LIMIT_BYTES,
        ARCHIVE_COUNT,
    )
    .map_err(|source| AppError::io("open log file", source))?;
    let (writer, guard) = tracing_appender::non_blocking(file);
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let subscriber = tracing_subscriber::registry().with(filter).with(
        tracing_subscriber::fmt::layer()
            .with_writer(writer)
            .with_ansi(false),
    );
    tracing::subscriber::set_global_default(subscriber)
        .map_err(|error| AppError::message("start logging", error.to_string()))?;
    install_panic_hook();
    Ok(LogGuard { _worker: guard })
}

fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!("{}", panic_diagnostic(info));
        previous(info);
    }));
}

/// Location-only panic line. The payload is intentionally omitted.
pub(crate) fn panic_diagnostic(info: &std::panic::PanicHookInfo<'_>) -> String {
    match info.location() {
        Some(location) => format!(
            "panic at {}:{}:{}",
            location.file(),
            location.line(),
            location.column()
        ),
        None => "panic at unknown location".to_string(),
    }
}

/// Size-capped log file. `archive_count` is the number of rotated siblings (`.1` …).
struct SizeRotatingFile {
    path: PathBuf,
    file: File,
    written: u64,
    max_bytes: u64,
    archive_count: usize,
}

impl SizeRotatingFile {
    fn create(path: &Path, max_bytes: u64, archive_count: usize) -> io::Result<Self> {
        if max_bytes == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "max_bytes must be positive",
            ));
        }
        if path.file_name().is_none() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "log path needs a file name",
            ));
        }
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        let written = file.metadata()?.len();
        Ok(Self {
            path: path.to_path_buf(),
            file,
            written,
            max_bytes,
            archive_count,
        })
    }

    fn rotate(&mut self) -> io::Result<()> {
        self.file.flush()?;
        // Close the active file before renaming it. Windows denies the rename
        // while this process still has the log open.
        let sink = OpenOptions::new().write(true).open("NUL")?;
        drop(std::mem::replace(&mut self.file, sink));
        shift_archives(&self.path, self.archive_count)?;
        self.file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        self.written = 0;
        Ok(())
    }
}

impl Write for SizeRotatingFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.written > 0 && self.written.saturating_add(buf.len() as u64) > self.max_bytes {
            self.rotate()?;
        }
        let written = self.file.write(buf)?;
        self.written = self.written.saturating_add(written as u64);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

fn shift_archives(path: &Path, archive_count: usize) -> io::Result<()> {
    if archive_count == 0 {
        return remove_if_exists(path);
    }
    remove_if_exists(&archive_path(path, archive_count))?;
    for index in (1..archive_count).rev() {
        let from = archive_path(path, index);
        if from.exists() {
            let to = archive_path(path, index + 1);
            remove_if_exists(&to)?;
            std::fs::rename(from, to)?;
        }
    }
    std::fs::rename(path, archive_path(path, 1))
}

fn archive_path(path: &Path, index: usize) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_else(|| std::ffi::OsString::from("skrivli.log"));
    name.push(format!(".{index}"));
    path.with_file_name(name)
}

fn remove_if_exists(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Write;
    use std::sync::{Mutex, OnceLock};

    use super::{panic_diagnostic, SizeRotatingFile};

    fn hook_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn temp_dir(label: &str) -> std::path::PathBuf {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("skrivli-log-{label}-{}-{id}", std::process::id()));
        fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn panic_diagnostic_records_location_not_payload() {
        // Arrange
        let _guard = hook_lock().lock().expect("hook lock");
        let previous = std::panic::take_hook();
        let (tx, rx) = std::sync::mpsc::channel();
        std::panic::set_hook(Box::new(move |info| {
            let _ = tx.send(panic_diagnostic(info));
        }));

        // Act
        let _ = std::panic::catch_unwind(|| panic!("secret-transcript-text"));
        let line = rx.recv().expect("hook line");
        std::panic::set_hook(previous);

        // Assert
        assert!(line.starts_with("panic at "));
        assert!(line.contains(':'));
        assert!(!line.contains("secret-transcript-text"));
    }

    #[test]
    fn rotating_file_keeps_three_generations() {
        // Arrange
        let dir = temp_dir("rotate");
        let path = dir.join("skrivli.log");
        let mut log = SizeRotatingFile::create(&path, 8, 2).expect("create");

        // Act
        log.write_all(b"12345678").expect("first");
        log.write_all(b"ABCDEFGH").expect("second");
        log.write_all(b"zzzzzzzz").expect("third");
        log.flush().expect("flush");

        // Assert
        assert_eq!(fs::read(&path).expect("active"), b"zzzzzzzz");
        assert_eq!(
            fs::read(dir.join("skrivli.log.1")).expect("archive 1"),
            b"ABCDEFGH"
        );
        assert_eq!(
            fs::read(dir.join("skrivli.log.2")).expect("archive 2"),
            b"12345678"
        );
        assert!(!dir.join("skrivli.log.3").exists());
        let _ = fs::remove_dir_all(dir);
    }
}
