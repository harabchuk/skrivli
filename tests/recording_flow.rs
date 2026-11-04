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

fn wav_files(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).expect("read") {
        let path = entry.expect("entry").path();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if name.ends_with(".wav") && !name.ends_with(".partial") {
            files.push(path);
        }
    }
    files
}

#[test]
fn start_stop_writes_header_lengths_and_stop_is_idempotent() {
    // Arrange
    let dir = temp_dir("save");
    let harness = RecordingHarness::new(&dir);
    harness.push_frames(480, 0.25);

    // Act
    harness.start_default();
    let recording = harness
        .wait_phase(RecordingHarnessPhase::Recording)
        .expect("recording");
    harness.stop();
    let idle = harness
        .wait_phase(RecordingHarnessPhase::Idle)
        .expect("idle");
    harness.stop();
    std::thread::sleep(Duration::from_millis(50));
    let files = wav_files(&dir);

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
    let bytes = fs::read(&files[0]).expect("wav");
    let data = u32::from_le_bytes([bytes[40], bytes[41], bytes[42], bytes[43]]);
    let riff = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    assert_eq!(data, 480 * 4);
    assert_eq!(riff, 36 + data);
    assert_eq!(bytes.len() as u32, 44 + data);
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
fn startup_failure_leaves_no_worker_and_no_wav() {
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
    assert!(wav_files(&dir).is_empty());
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
    assert!(wav_files(&dir).is_empty());
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
    let wav = wav_files(&mixed_dir);
    assert_eq!(wav.len(), 1);
    let bytes = fs::read(&wav[0]).expect("wav");
    let sample = i16::from_le_bytes([bytes[44], bytes[45]]);
    assert!(sample > 8_000 && sample < 14_000);
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
