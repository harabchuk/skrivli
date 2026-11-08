//! Fake-capture recording flow. No microphone or privacy grant is required.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use skrivli::testing::{RecordingHarness, RecordingHarnessPhase};

fn temp_dir(label: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir =
        std::env::temp_dir().join(format!("skrivli-flow-{label}-{}-{id}", std::process::id()));
    fs::create_dir_all(&dir).expect("temp");
    dir
}

fn flac_files(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).expect("read") {
        let path = entry.expect("entry").path();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if name.ends_with(".flac") && !name.ends_with(".partial") {
            files.push(path);
        }
    }
    files
}

fn assert_finalized_only(dir: &std::path::Path) {
    let names: Vec<String> = fs::read_dir(dir)
        .expect("read")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(flac_files(dir).len(), 1, "files: {names:?}");
    assert!(
        names.iter().all(|name| !name.contains("partial")),
        "files: {names:?}"
    );
}

#[test]
fn start_stop_writes_streaminfo_and_stop_is_idempotent() {
    // Arrange
    let dir = temp_dir("save");
    let harness = RecordingHarness::new(&dir);
    harness.push_frames(480, 0.25);

    // Act
    harness.start_default();
    let recording = harness
        .wait_phase(RecordingHarnessPhase::Recording)
        .expect("recording");
    // Let the host pull the queued block before stop closes both streams.
    std::thread::sleep(Duration::from_millis(50));
    harness.stop();
    let idle = harness
        .wait_phase(RecordingHarnessPhase::Idle)
        .expect("idle");
    harness.stop();
    std::thread::sleep(Duration::from_millis(50));
    let files = flac_files(&dir);

    // Assert
    assert!(recording.notice.is_none());
    assert_eq!(idle.notice, Some("Recording saved."));
    assert_eq!(harness.workers(), 0);
    assert_eq!(harness.capture_threads(), 0);
    assert_eq!(files.len(), 1);
    assert!(dir
        .read_dir()
        .expect("dir")
        .flatten()
        .all(|entry| !entry.file_name().to_string_lossy().ends_with(".partial")));
    let reader = flac_codec::decode::FlacSampleReader::open(&files[0]).expect("flac");
    assert_eq!(
        flac_codec::metadata::Metadata::total_samples(&reader),
        Some(480)
    );
    assert_eq!(flac_codec::metadata::Metadata::sample_rate(&reader), 48_000);
    assert_eq!(flac_codec::metadata::Metadata::channel_count(&reader), 2);
    assert!(
        dir.join(files[0].file_stem().expect("stem"))
            .with_extension("json")
            .is_file()
            || dir
                .read_dir()
                .expect("dir")
                .flatten()
                .any(|entry| { entry.file_name().to_string_lossy().ends_with(".json") })
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn stop_ends_a_live_capture_session() {
    // Arrange
    let dir = temp_dir("live-stop");
    let harness = RecordingHarness::new(&dir);
    harness.fail_system();
    harness.stream_mic(480, 0.2);

    // Act
    harness.start_default();
    harness
        .wait_phase(RecordingHarnessPhase::Recording)
        .expect("recording");
    harness.stop();
    let idle = harness
        .wait_phase(RecordingHarnessPhase::Idle)
        .expect("idle");

    // Assert
    assert!(idle
        .notice
        .unwrap_or("")
        .contains("System audio is unavailable"));
    assert_eq!(harness.workers(), 0);
    assert_eq!(harness.capture_threads(), 0);
    assert_eq!(flac_files(&dir).len(), 1);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn startup_failure_leaves_no_worker_and_no_flac() {
    // Arrange
    let dir = temp_dir("privacy");
    let harness = RecordingHarness::new(&dir);
    harness.fail_open(true);
    harness.fail_system();

    // Act
    harness.start_default();
    let failed = harness
        .wait_phase(RecordingHarnessPhase::Error)
        .expect("error");

    // Assert
    assert_eq!(
        failed.notice,
        Some("Microphone access is turned off in Windows privacy settings.")
    );
    assert_eq!(harness.workers(), 0);
    assert_eq!(harness.capture_threads(), 0);
    assert!(flac_files(&dir).is_empty());
    assert!(dir
        .read_dir()
        .expect("dir")
        .flatten()
        .all(|entry| !entry.file_name().to_string_lossy().contains("partial")));
    harness.stop();
    assert_eq!(harness.workers(), 0);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn idle_stop_does_not_create_a_recording() {
    // Arrange
    let dir = temp_dir("idle");
    let harness = RecordingHarness::new(&dir);

    // Act
    harness.stop();
    std::thread::sleep(Duration::from_millis(40));
    harness.stop();

    // Assert
    assert_eq!(harness.workers(), 0);
    assert!(flac_files(&dir).is_empty());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn mixed_sources_are_summed_and_a_dead_loopback_still_saves() {
    // Arrange
    let mixed_dir = temp_dir("mix");
    let mixed = RecordingHarness::new(&mixed_dir);
    mixed.push_frames(480, 0.2);
    mixed.push_system_frames(480, 0.2);
    let degraded = temp_dir("loop");
    let mic_only = RecordingHarness::new(&degraded);
    mic_only.fail_system();
    mic_only.push_frames(480, 0.25);
    mic_only.set_overruns(2);

    // Act
    mixed.start_default();
    mixed
        .wait_phase(RecordingHarnessPhase::Recording)
        .expect("recording");
    mixed.stop();
    mixed
        .wait_phase(RecordingHarnessPhase::Idle)
        .expect("saved");
    mic_only.start_default();
    mic_only
        .wait_phase(RecordingHarnessPhase::Recording)
        .expect("mic recording");
    mic_only.stop();
    let saved = mic_only
        .wait_phase(RecordingHarnessPhase::Idle)
        .expect("mic only");

    // Assert
    let flac = flac_files(&mixed_dir);
    assert_eq!(flac.len(), 1);
    let mut reader = flac_codec::decode::FlacSampleReader::open(&flac[0]).expect("flac");
    let mut decoded = [0_i32; 2];
    assert_eq!(reader.read(&mut decoded).expect("decode"), 2);
    assert!(decoded[0] > 8_000 && decoded[0] < 14_000);
    assert!(saved
        .notice
        .unwrap_or("")
        .contains("System audio is unavailable"));
    let sidecar = fs::read_dir(&degraded)
        .expect("dir")
        .flatten()
        .find(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        .expect("sidecar");
    let text = fs::read_to_string(sidecar.path()).expect("json");
    assert!(text.contains("overrun"));
    let _ = fs::remove_dir_all(mixed_dir);
    let _ = fs::remove_dir_all(degraded);
}

#[test]
fn mid_session_low_disk_stops_with_warning() {
    // Arrange
    let dir = temp_dir("low-disk");
    let harness = RecordingHarness::new(&dir);
    harness.fail_system();
    harness.stream_mic(480, 0.2);

    // Act
    harness.start_default();
    harness
        .wait_phase(RecordingHarnessPhase::Recording)
        .expect("recording");
    harness.set_free_bytes(1024);
    let stopped = harness
        .wait_notice_contains("free disk space is low")
        .expect("low disk");

    // Assert
    assert!(matches!(
        stopped.phase,
        RecordingHarnessPhase::Recording
            | RecordingHarnessPhase::Stopping
            | RecordingHarnessPhase::Error
            | RecordingHarnessPhase::Idle
    ));
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while harness.workers() > 0 && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(harness.workers(), 0);
    assert_finalized_only(&dir);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn mid_session_write_fault_preserves_usable_output() {
    // Arrange
    let dir = temp_dir("write-fault");
    let harness = RecordingHarness::new(&dir);
    harness.fail_system();
    harness.push_frames(480, 0.25);
    harness.push_frames(480, 0.25);
    harness.inject_write_fault();

    // Act — fault is armed before the first mix write after Recording.
    harness.start_default();
    let failed = harness
        .wait_notice_contains("Could not save the recording")
        .expect("save failure");

    // Assert
    assert!(failed.notice.unwrap_or("").contains("Could not save"));
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while harness.workers() > 0 && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(harness.workers(), 0);
    assert_finalized_only(&dir);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn folder_fault_finalizes_in_place_and_reports_folder() {
    // Arrange
    let dir = temp_dir("folder-fault");
    let harness = RecordingHarness::new(&dir);
    harness.fail_system();
    harness.stream_mic(480, 0.2);

    // Act
    harness.start_default();
    harness
        .wait_phase(RecordingHarnessPhase::Recording)
        .expect("recording");
    std::thread::sleep(Duration::from_millis(40));
    harness.inject_folder_fault();
    let failed = harness
        .wait_notice_contains("The recordings folder is not writable")
        .expect("folder notice");

    // Assert
    assert!(matches!(
        failed.phase,
        RecordingHarnessPhase::Recording
            | RecordingHarnessPhase::Stopping
            | RecordingHarnessPhase::Error
            | RecordingHarnessPhase::Idle
    ));
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while harness.workers() > 0 && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_finalized_only(&dir);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn mid_session_mic_loss_keeps_system_and_degrades() {
    // Arrange
    let dir = temp_dir("mic-loss");
    let harness = RecordingHarness::new(&dir);
    harness.stream_mic(480, 0.2);
    harness.stream_system(480, 0.2);

    // Act
    harness.start_default();
    harness
        .wait_phase(RecordingHarnessPhase::Recording)
        .expect("recording");
    harness.fail_next_mic_open();
    harness.fail_mic_pull();
    let degraded = harness
        .wait_notice_contains("microphone is unavailable")
        .expect("degraded");
    harness.stop();
    let idle = harness
        .wait_phase(RecordingHarnessPhase::Idle)
        .expect("idle");

    // Assert
    assert!(degraded.notice.unwrap_or("").contains("microphone"));
    assert!(idle.notice.is_some());
    assert_eq!(flac_files(&dir).len(), 1);
    let sidecar = fs::read_dir(&dir)
        .expect("dir")
        .flatten()
        .find(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        .expect("sidecar");
    let text = fs::read_to_string(sidecar.path()).expect("json");
    assert!(text.contains("gap") || text.contains("capture") || text.contains("source_change"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn capture_worker_panic_is_surfaced() {
    // Arrange
    let dir = temp_dir("panic");
    let harness = RecordingHarness::new(&dir);
    harness.fail_system();
    harness.stream_mic(480, 0.2);

    // Act
    harness.start_default();
    harness
        .wait_phase(RecordingHarnessPhase::Recording)
        .expect("recording");
    harness.panic_mic_pull();
    let noticed = harness
        .wait_notice_contains("capture worker failed")
        .expect("worker notice");
    harness.stop();
    let _ = harness.wait_phase(RecordingHarnessPhase::Idle);

    // Assert
    assert!(noticed.notice.unwrap_or("").contains("worker"));
    assert_eq!(harness.workers(), 0);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn silent_open_source_reports_stall() {
    // Arrange
    let dir = temp_dir("stall");
    let harness = RecordingHarness::new(&dir);
    harness.fail_system();
    harness.silent_mic();

    // Act
    harness.start_default();
    harness
        .wait_phase(RecordingHarnessPhase::Recording)
        .expect("recording");
    let stalled = harness
        .wait_notice_contains("stalled")
        .expect("stall notice");
    harness.stop();
    let _ = harness.wait_phase(RecordingHarnessPhase::Idle);

    // Assert
    assert!(stalled.notice.unwrap_or("").contains("stalled"));
    let _ = fs::remove_dir_all(dir);
}
