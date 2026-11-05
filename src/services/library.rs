//! Library use-case host: scan, watch, rename, delete on a worker thread.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::domain::ids::RecordingId;
use crate::domain::recording::preserve_selection;
use crate::infrastructure::storage::library::{
    associated_paths, rename_recording, scan_library, LibraryCatalog, LibraryEntry, RenameResult,
};
use crate::services::ports::{DirectoryWatch, Trash, WatchEvent, DEFAULT_WATCH_DEBOUNCE};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LibrarySnapshot {
    pub(crate) catalog: LibraryCatalog,
    pub(crate) selected: Option<RecordingId>,
    pub(crate) notice: Option<&'static str>,
}

enum LibraryCommand {
    Refresh,
    SetDirectory(PathBuf),
    Select(Option<RecordingId>),
    Rename { id: RecordingId, new_stem: String },
    Delete { id: RecordingId },
    Shutdown,
}

#[derive(Clone)]
pub(crate) struct LibraryHost {
    tx: Sender<LibraryCommand>,
}

impl LibraryHost {
    pub(crate) fn spawn(
        directory: PathBuf,
        trash: Arc<dyn Trash>,
        watch: Arc<dyn DirectoryWatch>,
        on_change: Box<dyn Fn(LibrarySnapshot) + Send>,
        stop: Arc<AtomicBool>,
    ) -> (Self, JoinHandle<()>) {
        let (tx, rx) = mpsc::channel();
        let command_tx = tx.clone();
        let handle = thread::spawn(move || {
            library_loop(directory, trash, watch, rx, on_change, stop);
        });
        let _ = command_tx.send(LibraryCommand::Refresh);
        (Self { tx }, handle)
    }

    pub(crate) fn refresh(&self) {
        let _ = self.tx.send(LibraryCommand::Refresh);
    }

    pub(crate) fn set_directory(&self, directory: PathBuf) {
        let _ = self.tx.send(LibraryCommand::SetDirectory(directory));
    }

    pub(crate) fn select(&self, id: Option<RecordingId>) {
        let _ = self.tx.send(LibraryCommand::Select(id));
    }

    pub(crate) fn rename(&self, id: RecordingId, new_stem: String) {
        let _ = self.tx.send(LibraryCommand::Rename { id, new_stem });
    }

    pub(crate) fn delete(&self, id: RecordingId) {
        let _ = self.tx.send(LibraryCommand::Delete { id });
    }

    pub(crate) fn shutdown(&self) {
        let _ = self.tx.send(LibraryCommand::Shutdown);
    }
}

fn library_loop(
    mut directory: PathBuf,
    trash: Arc<dyn Trash>,
    watch: Arc<dyn DirectoryWatch>,
    rx: mpsc::Receiver<LibraryCommand>,
    on_change: Box<dyn Fn(LibrarySnapshot) + Send>,
    stop: Arc<AtomicBool>,
) {
    let mut selected = None;
    let mut catalog = LibraryCatalog::empty();
    let mut active = start_watch(watch.as_ref(), directory.clone());

    publish(&catalog, selected, None, &on_change);

    loop {
        if stop.load(Ordering::Acquire) {
            break;
        }

        while let Ok(event) = active.rx.try_recv() {
            if matches!(event, WatchEvent::Changed | WatchEvent::Poll) {
                rescan(&directory, &mut catalog, &mut selected, None, &on_change);
            }
        }

        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(LibraryCommand::Shutdown) => break,
            Ok(LibraryCommand::Refresh) => {
                rescan(&directory, &mut catalog, &mut selected, None, &on_change);
            }
            Ok(LibraryCommand::SetDirectory(next)) => {
                directory = next;
                active.stop();
                active = start_watch(watch.as_ref(), directory.clone());
                rescan(&directory, &mut catalog, &mut selected, None, &on_change);
            }
            Ok(LibraryCommand::Select(id)) => {
                selected = id;
                publish(&catalog, selected, None, &on_change);
            }
            Ok(LibraryCommand::Rename { id, new_stem }) => {
                let notice = match rename_recording(&catalog, id, &new_stem) {
                    Ok(result) => rename_notice(&result),
                    Err(error) => {
                        tracing::error!(error = %error, "rename failed");
                        Some(error.user_message())
                    }
                };
                rescan(&directory, &mut catalog, &mut selected, notice, &on_change);
                selected = preserve_ids(Some(id), &catalog);
                if notice.is_some() {
                    publish(&catalog, selected, notice, &on_change);
                }
            }
            Ok(LibraryCommand::Delete { id }) => {
                let notice = match catalog.find(id) {
                    Some(entry) => delete_entry(entry, trash.as_ref()),
                    None => Some("Could not update the recordings library."),
                };
                let keep = selected.filter(|current| *current != id);
                selected = keep;
                rescan(&directory, &mut catalog, &mut selected, notice, &on_change);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    active.stop();
}

struct ActiveWatch {
    stop: Arc<AtomicBool>,
    rx: mpsc::Receiver<WatchEvent>,
    handle: JoinHandle<()>,
}

impl ActiveWatch {
    fn stop(self) {
        self.stop.store(true, Ordering::Release);
        let _ = self.handle.join();
    }
}

fn start_watch(watch: &dyn DirectoryWatch, directory: PathBuf) -> ActiveWatch {
    let stop = Arc::new(AtomicBool::new(false));
    let (rx, handle) = watch.spawn(directory, Arc::clone(&stop), DEFAULT_WATCH_DEBOUNCE);
    ActiveWatch { stop, rx, handle }
}

fn rescan(
    directory: &std::path::Path,
    catalog: &mut LibraryCatalog,
    selected: &mut Option<RecordingId>,
    notice: Option<&'static str>,
    on_change: &dyn Fn(LibrarySnapshot),
) {
    *catalog = scan_library(directory);
    *selected = preserve_ids(*selected, catalog);
    publish(catalog, *selected, notice.or(catalog.error), on_change);
}

fn preserve_ids(previous: Option<RecordingId>, catalog: &LibraryCatalog) -> Option<RecordingId> {
    let infos: Vec<_> = catalog
        .entries
        .iter()
        .map(|entry| entry.info.clone())
        .collect();
    preserve_selection(previous, &infos)
}

fn delete_entry(entry: &LibraryEntry, trash: &dyn Trash) -> Option<&'static str> {
    let paths = associated_paths(entry);
    match trash.discard(&paths) {
        Ok(()) => None,
        Err(error) => {
            tracing::error!(error = %error, "delete failed");
            Some(error.user_message())
        }
    }
}

fn rename_notice(result: &RenameResult) -> Option<&'static str> {
    if result.partial_failure() {
        Some("Some related files were not renamed.")
    } else {
        None
    }
}

fn publish(
    catalog: &LibraryCatalog,
    selected: Option<RecordingId>,
    notice: Option<&'static str>,
    on_change: &dyn Fn(LibrarySnapshot),
) {
    on_change(LibrarySnapshot {
        catalog: catalog.clone(),
        selected,
        notice,
    });
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::mpsc::{self, Receiver};
    use std::sync::{Arc, Mutex};
    use std::thread::JoinHandle;
    use std::time::Duration;

    use super::LibraryHost;
    use crate::common::error::AppError;
    use crate::infrastructure::platform;
    use crate::infrastructure::storage::sidecar::{write_sidecar, Sidecar};
    use crate::services::ports::{DirectoryWatch, Trash, WatchEvent};

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("skrivli-libhost-{tag}-{id}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("dir");
        dir
    }

    struct FakeTrash {
        discarded: Mutex<Vec<PathBuf>>,
    }

    impl Trash for FakeTrash {
        fn discard(&self, paths: &[PathBuf]) -> Result<(), AppError> {
            self.discarded
                .lock()
                .expect("lock")
                .extend_from_slice(paths);
            Ok(())
        }
    }

    struct SilentWatch;

    impl DirectoryWatch for SilentWatch {
        fn spawn(
            &self,
            _directory: PathBuf,
            _stop: Arc<AtomicBool>,
            _debounce: Duration,
        ) -> (Receiver<WatchEvent>, JoinHandle<()>) {
            let (tx, rx) = mpsc::channel();
            drop(tx);
            let handle = std::thread::spawn(|| {});
            (rx, handle)
        }
    }

    #[test]
    fn host_refresh_lists_saved_recording() {
        // Arrange
        let dir = temp_dir("host");
        let wav = dir.join("clip.wav");
        fs::write(&wav, b"RIFF").expect("wav");
        write_sidecar(
            &wav,
            &Sidecar {
                id: 99,
                started_at_unix_ms: 10,
                ended_at_unix_ms: 20,
                duration_ms: 10,
                microphone_id: "m".into(),
                system_audio_id: "s".into(),
                problems: Vec::new(),
            },
        )
        .expect("side");
        let snapshots = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&snapshots);
        let stop = Arc::new(AtomicBool::new(false));
        let host_ports = platform::host();
        let (host, handle) = LibraryHost::spawn(
            dir.clone(),
            Arc::clone(&host_ports.trash),
            Arc::clone(&host_ports.watch),
            Box::new(move |snapshot| {
                sink.lock().expect("lock").push(snapshot);
            }),
            Arc::clone(&stop),
        );

        // Act
        host.refresh();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let mut found = false;
        while std::time::Instant::now() < deadline {
            let guard = snapshots.lock().expect("lock");
            if guard.iter().any(|snap| {
                snap.catalog
                    .entries
                    .iter()
                    .any(|entry| entry.info.id().raw() == 99)
            }) {
                found = true;
                break;
            }
            drop(guard);
            std::thread::sleep(Duration::from_millis(20));
        }
        host.shutdown();
        stop.store(true, Ordering::Release);
        let _ = handle.join();

        // Assert
        assert!(found, "expected catalog entry id 99");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn delete_asks_trash_and_leaves_files_on_disk() {
        // Arrange
        let dir = temp_dir("delete");
        let wav = dir.join("clip.wav");
        fs::write(&wav, b"RIFF").expect("wav");
        write_sidecar(
            &wav,
            &Sidecar {
                id: 42,
                started_at_unix_ms: 10,
                ended_at_unix_ms: 20,
                duration_ms: 10,
                microphone_id: "m".into(),
                system_audio_id: "s".into(),
                problems: Vec::new(),
            },
        )
        .expect("side");
        let trash = Arc::new(FakeTrash {
            discarded: Mutex::new(Vec::new()),
        });
        let discarded = Arc::clone(&trash);
        let snapshots = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&snapshots);
        let stop = Arc::new(AtomicBool::new(false));
        let (host, handle) = LibraryHost::spawn(
            dir.clone(),
            trash,
            Arc::new(SilentWatch),
            Box::new(move |snapshot| {
                sink.lock().expect("lock").push(snapshot);
            }),
            Arc::clone(&stop),
        );

        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let mut id = None;
        while std::time::Instant::now() < deadline {
            let guard = snapshots.lock().expect("lock");
            if let Some(entry) = guard
                .iter()
                .rev()
                .find_map(|snap| snap.catalog.entries.first())
            {
                id = Some(entry.info.id());
                break;
            }
            drop(guard);
            std::thread::sleep(Duration::from_millis(20));
        }
        let id = id.expect("catalog entry");

        // Act
        host.delete(id);
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            if !discarded.discarded.lock().expect("lock").is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        host.shutdown();
        stop.store(true, Ordering::Release);
        let _ = handle.join();

        // Assert
        let paths = discarded.discarded.lock().expect("lock");
        assert!(
            paths.iter().any(|path| path == &wav),
            "trash should receive the wav path"
        );
        assert!(wav.exists(), "fake trash must leave files on disk");
        let _ = fs::remove_dir_all(dir);
    }
}
