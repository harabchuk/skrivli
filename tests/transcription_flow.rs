//! Transcript write + TranscriptionHost flow. No speech engine or microphone.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use skrivli::testing::{
    format_transcript_segments, run_transcription_flow, transcript_path_for, write_transcript_text,
    FakeInferenceEngine, TranscriptionFlowConfig, TranscriptionFlowPhase,
};

fn temp_dir(label: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "skrivli-txflow-{label}-{}-{id}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp");
    dir
}

fn write_dummy_flac(path: &Path, payload: &[u8]) {
    let mut bytes = b"fLaC".to_vec();
    bytes.extend_from_slice(payload);
    fs::write(path, bytes).expect("flac");
}

fn write_pcm_flac(path: &Path, frames: usize) {
    skrivli::testing::write_silent_flac(path, frames).expect("flac");
}

fn seed_sidecar(audio: &Path, id: u64) {
    skrivli::testing::write_recording_sidecar(audio, id).expect("sidecar");
}

fn short_config() -> TranscriptionFlowConfig {
    TranscriptionFlowConfig { read_frames: 1_000 }
}

#[test]
fn successful_format_and_write_leaves_flac_unchanged() {
    // Arrange
    let dir = temp_dir("ok");
    let audio = dir.join("meeting.flac");
    let payload = vec![1_u8, 2, 3, 4, 5, 6, 7, 8];
    write_dummy_flac(&audio, &payload);
    let before = fs::read(&audio).expect("before");
    let text = format_transcript_segments(&[(3_661_000, 3_662_000, "hello café")]);

    // Act
    let path = write_transcript_text(&audio, &text).expect("write");
    let after = fs::read(&audio).expect("after");
    let written = fs::read_to_string(&path).expect("txt");

    // Assert
    assert_eq!(before, after);
    assert_eq!(path, transcript_path_for(&audio).expect("path"));
    assert!(written.starts_with("[01:01:01] hello café"));
    assert!(!dir.join("meeting.txt.tmp").exists());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn failed_transcript_write_leaves_previous_txt_and_flac() {
    // Arrange
    let dir = temp_dir("fail");
    let audio = dir.join("clip.flac");
    let payload = vec![9_u8, 8, 7, 6];
    write_dummy_flac(&audio, &payload);
    let audio_before = fs::read(&audio).expect("audio before");
    let txt = dir.join("clip.txt");
    fs::write(&txt, "previous transcript\n").expect("prev");
    // Block the temp sibling so the atomic write cannot open it.
    let blocker = dir.join("clip.txt.tmp");
    fs::create_dir_all(&blocker).expect("blocker");

    // Act
    let result = write_transcript_text(&audio, "replacement\n");

    // Assert
    assert!(result.is_err());
    assert_eq!(fs::read(&audio).expect("audio after"), audio_before);
    assert_eq!(
        fs::read_to_string(&txt).expect("txt after"),
        "previous transcript\n"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn host_success_writes_transcript_updates_sidecar() {
    // Arrange
    let dir = temp_dir("host-ok");
    let audio = dir.join("meeting.flac");
    write_pcm_flac(&audio, 48_000);
    let before = fs::read(&audio).expect("before");
    seed_sidecar(&audio, 11);
    let model = dir.join("ggml-base.bin");
    fs::write(&model, b"ggml").expect("model");
    let engine = FakeInferenceEngine::success("hello café");

    // Act
    let result = run_transcription_flow(
        engine,
        short_config(),
        11,
        &audio,
        &model,
        "base-q5_1",
        None,
    );

    // Assert
    assert_eq!(result.phase, TranscriptionFlowPhase::Done);
    assert_eq!(fs::read(&audio).expect("after"), before);
    let text = fs::read_to_string(dir.join("meeting.txt")).expect("txt");
    assert!(text.contains("hello café"));
    assert_eq!(result.sidecar_status.as_deref(), Some("ready"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn host_engine_failure_marks_failed() {
    // Arrange
    let dir = temp_dir("host-fail");
    let audio = dir.join("clip.flac");
    write_pcm_flac(&audio, 8_000);
    let before = fs::read(&audio).expect("before");
    seed_sidecar(&audio, 12);
    let model = dir.join("ggml-base.bin");
    fs::write(&model, b"ggml").expect("model");

    // Act
    let result = run_transcription_flow(
        FakeInferenceEngine::fail_chunk(),
        short_config(),
        12,
        &audio,
        &model,
        "base-q5_1",
        None,
    );

    // Assert
    assert_eq!(result.phase, TranscriptionFlowPhase::Failed);
    assert_eq!(fs::read(&audio).expect("after"), before);
    assert!(!dir.join("clip.txt").exists());
    assert_eq!(result.sidecar_status.as_deref(), Some("failed"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn host_cancel_stops_running_job() {
    // Arrange
    let dir = temp_dir("host-cancel");
    let audio = dir.join("clip.flac");
    write_pcm_flac(&audio, 48_000);
    let before = fs::read(&audio).expect("before");
    seed_sidecar(&audio, 13);
    let model = dir.join("ggml-base.bin");
    fs::write(&model, b"ggml").expect("model");

    // Act
    let result = run_transcription_flow(
        FakeInferenceEngine::slow(800),
        short_config(),
        13,
        &audio,
        &model,
        "base-q5_1",
        Some(Duration::from_millis(50)),
    );

    // Assert
    assert_eq!(result.phase, TranscriptionFlowPhase::Cancelled);
    assert_eq!(fs::read(&audio).expect("after"), before);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn host_cancel_racing_completion_is_stable() {
    // Arrange
    let dir = temp_dir("host-race");
    let audio = dir.join("clip.flac");
    write_pcm_flac(&audio, 4_000);
    seed_sidecar(&audio, 14);
    let model = dir.join("ggml-base.bin");
    fs::write(&model, b"ggml").expect("model");

    // Act
    let result = run_transcription_flow(
        FakeInferenceEngine::race_cancel(),
        short_config(),
        14,
        &audio,
        &model,
        "base-q5_1",
        Some(Duration::from_millis(10)),
    );

    // Assert
    assert!(matches!(
        result.phase,
        TranscriptionFlowPhase::Done
            | TranscriptionFlowPhase::Cancelled
            | TranscriptionFlowPhase::Failed
    ));
    assert!(audio.is_file());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn host_invalid_model_fails() {
    // Arrange
    let dir = temp_dir("host-model");
    let audio = dir.join("clip.flac");
    write_pcm_flac(&audio, 2_000);
    let before = fs::read(&audio).expect("before");
    seed_sidecar(&audio, 15);
    let model = dir.join("invalid.bin");
    fs::write(&model, b"nope").expect("model");

    // Act
    let result = run_transcription_flow(
        FakeInferenceEngine::success("x"),
        short_config(),
        15,
        &audio,
        &model,
        "bad",
        None,
    );

    // Assert
    assert_eq!(result.phase, TranscriptionFlowPhase::Failed);
    assert_eq!(fs::read(&audio).expect("after"), before);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn host_streams_the_file_in_read_sized_feeds() {
    // Arrange
    let dir = temp_dir("host-chunk");
    let audio = dir.join("long.flac");
    write_pcm_flac(&audio, 24_000);
    seed_sidecar(&audio, 16);
    let model = dir.join("ggml-base.bin");
    fs::write(&model, b"ggml").expect("model");
    let calls = Arc::new(Mutex::new(Vec::new()));
    let engine = FakeInferenceEngine::success_with_calls("chunk", Arc::clone(&calls));

    // Act
    let result = run_transcription_flow(
        engine,
        short_config(),
        16,
        &audio,
        &model,
        "base-q5_1",
        None,
    );
    let sizes = calls.lock().expect("lock").clone();

    // Assert
    assert_eq!(result.phase, TranscriptionFlowPhase::Done);
    assert!(sizes.len() >= 2, "expected multiple feeds, got {sizes:?}");
    // 1,000 frames at 48 kHz are about 333 samples at 16 kHz.
    let max = sizes.iter().copied().max().unwrap_or(0);
    assert!(max <= 400, "feed too large: max={max} sizes={sizes:?}");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn host_fifo_two_queued_recordings() {
    // Arrange
    let dir = temp_dir("host-fifo");
    let audio_a = dir.join("a.flac");
    let audio_b = dir.join("b.flac");
    write_pcm_flac(&audio_a, 4_000);
    write_pcm_flac(&audio_b, 4_000);
    seed_sidecar(&audio_a, 21);
    seed_sidecar(&audio_b, 22);
    let model = dir.join("ggml-base.bin");
    fs::write(&model, b"ggml").expect("model");
    let order = Arc::new(Mutex::new(Vec::new()));

    // Act
    let seen = skrivli::testing::run_transcription_fifo(
        FakeInferenceEngine::slow(80),
        short_config(),
        &[(21, audio_a.as_path()), (22, audio_b.as_path())],
        &model,
        "base-q5_1",
        Arc::clone(&order),
    );

    // Assert
    assert_eq!(seen, vec![21, 22]);
    assert!(dir.join("a.txt").is_file());
    assert!(dir.join("b.txt").is_file());
    let _ = fs::remove_dir_all(dir);
}

fn wait_for_file(path: &Path) {
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < deadline {
        if path.is_file() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("missing {}", path.display());
}

fn file_names(paths: &[PathBuf]) -> Vec<String> {
    paths
        .iter()
        .filter_map(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .collect()
}

#[test]
fn queued_rename_cancels_and_writes_no_transcript() {
    use skrivli::testing::{IdentitySession, InferenceHold};

    // Arrange
    let dir = temp_dir("queued-rename");
    let blocker = dir.join("blocker.flac");
    let clip = dir.join("clip.flac");
    write_pcm_flac(&blocker, 4_000);
    write_pcm_flac(&clip, 4_000);
    seed_sidecar(&blocker, 1);
    seed_sidecar(&clip, 2);
    let session = IdentitySession::start(&dir, InferenceHold::BeforeOpen);
    session.wait_for_library_id(1);
    session.wait_for_library_id(2);
    session.enqueue(1, &blocker);
    session.wait_until_held();
    session.enqueue(2, &clip);

    // Act
    session.rename(2, "renamed");
    wait_for_file(&dir.join("renamed.flac"));
    session.release();
    session.shutdown();

    // Assert
    assert!(!dir.join("clip.txt").exists());
    assert!(!dir.join("renamed.txt").exists());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn queued_delete_cancels_and_writes_no_transcript() {
    use skrivli::testing::{IdentitySession, InferenceHold};

    // Arrange
    let dir = temp_dir("queued-delete");
    let blocker = dir.join("blocker.flac");
    let clip = dir.join("clip.flac");
    write_pcm_flac(&blocker, 4_000);
    write_pcm_flac(&clip, 4_000);
    seed_sidecar(&blocker, 1);
    seed_sidecar(&clip, 2);
    let session = IdentitySession::start(&dir, InferenceHold::BeforeOpen);
    session.wait_for_library_id(2);
    session.enqueue(1, &blocker);
    session.wait_until_held();
    session.enqueue(2, &clip);

    // Act
    session.delete(2);
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < deadline && session.discarded().is_empty() {
        std::thread::sleep(Duration::from_millis(20));
    }
    session.release();
    session.shutdown();

    // Assert
    assert!(!session_paths_include_txt(&dir));
    assert!(!dir.join("clip.txt").exists());
    let _ = fs::remove_dir_all(dir);
}

fn session_paths_include_txt(dir: &Path) -> bool {
    dir.join("clip.txt").exists() || dir.join("renamed.txt").exists()
}

#[test]
fn running_delete_cancels_before_write_and_keeps_flac() {
    use skrivli::testing::{IdentitySession, InferenceHold};

    // Arrange
    let dir = temp_dir("running-delete");
    let audio = dir.join("clip.flac");
    write_pcm_flac(&audio, 4_000);
    seed_sidecar(&audio, 5);
    let before = fs::read(&audio).expect("before");
    let session = IdentitySession::start(&dir, InferenceHold::DuringFeed);
    session.wait_for_library_id(5);
    session.enqueue(5, &audio);
    session.wait_until_held();

    // Act
    session.delete(5);
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < deadline && session.discarded().is_empty() {
        std::thread::sleep(Duration::from_millis(20));
    }
    session.release();
    session.shutdown();

    // Assert
    assert_eq!(fs::read(&audio).expect("after"), before);
    assert!(!dir.join("clip.txt").exists());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn rename_before_write_retargets_transcript_beside_new_audio() {
    use skrivli::testing::{IdentitySession, InferenceHold};

    // Arrange
    let dir = temp_dir("retarget");
    let audio = dir.join("meeting.flac");
    write_pcm_flac(&audio, 4_000);
    seed_sidecar(&audio, 6);
    let before = fs::read(&audio).expect("before");
    let session = IdentitySession::start(&dir, InferenceHold::BeforeOpen);
    session.wait_for_library_id(6);
    session.enqueue(6, &audio);
    session.wait_until_held();

    // Act
    session.rename(6, "notes");
    // The group rename moves audio first, then the sidecar the write checks identity against.
    wait_for_file(&dir.join("notes.flac"));
    wait_for_file(&dir.join("notes.json"));
    session.release();
    session.wait_terminal();
    let phase = session.phase();
    session.shutdown();

    // Assert
    assert_eq!(phase, skrivli::testing::TranscriptionFlowPhase::Done);
    assert_eq!(fs::read(dir.join("notes.flac")).expect("moved"), before);
    let text = fs::read_to_string(dir.join("notes.txt")).expect("txt");
    assert!(text.contains("noted"));
    assert!(!dir.join("meeting.txt").exists());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn rename_of_a_different_id_does_not_cancel() {
    use skrivli::testing::{IdentitySession, InferenceHold};

    // Arrange
    let dir = temp_dir("other-rename");
    let keep = dir.join("keep.flac");
    let other = dir.join("other.flac");
    write_pcm_flac(&keep, 4_000);
    write_pcm_flac(&other, 4_000);
    seed_sidecar(&keep, 7);
    seed_sidecar(&other, 8);
    let session = IdentitySession::start(&dir, InferenceHold::BeforeOpen);
    session.wait_for_library_id(7);
    session.wait_for_library_id(8);
    session.enqueue(7, &keep);
    session.wait_until_held();

    // Act
    session.rename(8, "moved");
    wait_for_file(&dir.join("moved.flac"));
    session.release();
    session.wait_terminal();
    let phase = session.phase();
    session.shutdown();

    // Assert
    assert_eq!(phase, skrivli::testing::TranscriptionFlowPhase::Done);
    assert!(dir.join("keep.txt").is_file());
    assert!(!dir.join("moved.txt").exists());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn delete_ready_recording_trashes_audio_sidecar_and_transcript() {
    use skrivli::testing::{write_transcript_text, IdentitySession, InferenceHold};

    // Arrange
    let dir = temp_dir("ready-delete");
    let audio = dir.join("clip.flac");
    write_pcm_flac(&audio, 4_000);
    seed_sidecar(&audio, 9);
    write_transcript_text(&audio, "already promoted\n").expect("txt");
    let session = IdentitySession::start(&dir, InferenceHold::BeforeOpen);
    session.wait_for_library_id(9);

    // Act
    session.delete(9);
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let mut names = Vec::new();
    while std::time::Instant::now() < deadline {
        names = file_names(&session.discarded());
        if names.iter().any(|name| name == "clip.txt") {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    session.shutdown();

    // Assert
    assert!(names.iter().any(|name| name == "clip.flac"), "{names:?}");
    assert!(names.iter().any(|name| name == "clip.json"), "{names:?}");
    assert!(names.iter().any(|name| name == "clip.txt"), "{names:?}");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn late_completion_after_delete_does_not_write_transcript() {
    use skrivli::testing::{IdentitySession, InferenceHold};

    // Arrange
    let dir = temp_dir("late-delete");
    let audio = dir.join("clip.flac");
    write_pcm_flac(&audio, 4_000);
    seed_sidecar(&audio, 10);
    let before = fs::read(&audio).expect("before");
    let session = IdentitySession::start(&dir, InferenceHold::DuringFeed);
    session.wait_for_library_id(10);
    session.enqueue(10, &audio);
    session.wait_until_held();

    // Act
    session.delete(10);
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < deadline && session.discarded().is_empty() {
        std::thread::sleep(Duration::from_millis(20));
    }
    session.release();
    session.shutdown();

    // Assert
    assert_eq!(fs::read(&audio).expect("after"), before);
    assert!(!dir.join("clip.txt").exists());
    let _ = fs::remove_dir_all(dir);
}
