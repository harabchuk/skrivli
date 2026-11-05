//! Transcript write + TranscriptionHost flow. No whisper.cpp or microphone.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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

fn write_dummy_wav(path: &Path, payload: &[u8]) {
    let mut bytes = Vec::with_capacity(44 + payload.len());
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + payload.len() as u32).to_le_bytes());
    bytes.extend_from_slice(b"WAVE");
    bytes.extend_from_slice(b"fmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&2_u16.to_le_bytes());
    bytes.extend_from_slice(&48_000_u32.to_le_bytes());
    bytes.extend_from_slice(&(48_000_u32 * 4).to_le_bytes());
    bytes.extend_from_slice(&4_u16.to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    bytes.extend_from_slice(payload);
    fs::write(path, bytes).expect("wav");
}

fn write_pcm_wav(path: &Path, frames: usize) {
    let channels = 2_usize;
    let sample_rate = 48_000_u32;
    let data_bytes = (frames * channels * 2) as u32;
    let mut bytes = Vec::with_capacity(44 + data_bytes as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    bytes.extend_from_slice(b"WAVE");
    bytes.extend_from_slice(b"fmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&(channels as u16).to_le_bytes());
    bytes.extend_from_slice(&sample_rate.to_le_bytes());
    bytes.extend_from_slice(&(sample_rate * channels as u32 * 2).to_le_bytes());
    bytes.extend_from_slice(&((channels * 2) as u16).to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_bytes.to_le_bytes());
    bytes.extend(std::iter::repeat_n(0_u8, data_bytes as usize));
    fs::write(path, bytes).expect("wav");
}

fn seed_sidecar(wav: &Path, id: u64) {
    skrivli::testing::write_recording_sidecar(wav, id).expect("sidecar");
}

fn short_config() -> TranscriptionFlowConfig {
    TranscriptionFlowConfig {
        window_ms: 200,
        overlap_ms: 40,
        read_frames: 1_000,
    }
}

#[test]
fn successful_format_and_write_leaves_wav_unchanged() {
    // Arrange
    let dir = temp_dir("ok");
    let wav = dir.join("meeting.wav");
    let payload = vec![1_u8, 2, 3, 4, 5, 6, 7, 8];
    write_dummy_wav(&wav, &payload);
    let before = fs::read(&wav).expect("before");
    let text = format_transcript_segments(&[(3_661_000, 3_662_000, "hello café")]);

    // Act
    let path = write_transcript_text(&wav, &text).expect("write");
    let after = fs::read(&wav).expect("after");
    let written = fs::read_to_string(&path).expect("txt");

    // Assert
    assert_eq!(before, after);
    assert_eq!(path, transcript_path_for(&wav).expect("path"));
    assert!(written.starts_with("[01:01:01] hello café"));
    assert!(!dir.join("meeting.txt.tmp").exists());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn failed_transcript_write_leaves_previous_txt_and_wav() {
    // Arrange
    let dir = temp_dir("fail");
    let wav = dir.join("clip.wav");
    let payload = vec![9_u8, 8, 7, 6];
    write_dummy_wav(&wav, &payload);
    let wav_before = fs::read(&wav).expect("wav before");
    let txt = dir.join("clip.txt");
    fs::write(&txt, "previous transcript\n").expect("prev");
    // Block the temp sibling so the atomic write cannot open it.
    let blocker = dir.join("clip.txt.tmp");
    fs::create_dir_all(&blocker).expect("blocker");

    // Act
    let result = write_transcript_text(&wav, "replacement\n");

    // Assert
    assert!(result.is_err());
    assert_eq!(fs::read(&wav).expect("wav after"), wav_before);
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
    let wav = dir.join("meeting.wav");
    write_pcm_wav(&wav, 48_000);
    let before = fs::read(&wav).expect("before");
    seed_sidecar(&wav, 11);
    let model = dir.join("ggml-base.bin");
    fs::write(&model, b"ggml").expect("model");
    let engine = FakeInferenceEngine::success("hello café");

    // Act
    let result =
        run_transcription_flow(engine, short_config(), 11, &wav, &model, "base-q5_1", None);

    // Assert
    assert_eq!(result.phase, TranscriptionFlowPhase::Done);
    assert_eq!(fs::read(&wav).expect("after"), before);
    let text = fs::read_to_string(dir.join("meeting.txt")).expect("txt");
    assert!(text.contains("hello café"));
    assert_eq!(result.sidecar_status.as_deref(), Some("ready"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn host_engine_failure_marks_failed() {
    // Arrange
    let dir = temp_dir("host-fail");
    let wav = dir.join("clip.wav");
    write_pcm_wav(&wav, 8_000);
    let before = fs::read(&wav).expect("before");
    seed_sidecar(&wav, 12);
    let model = dir.join("ggml-base.bin");
    fs::write(&model, b"ggml").expect("model");

    // Act
    let result = run_transcription_flow(
        FakeInferenceEngine::fail_chunk(),
        short_config(),
        12,
        &wav,
        &model,
        "base-q5_1",
        None,
    );

    // Assert
    assert_eq!(result.phase, TranscriptionFlowPhase::Failed);
    assert_eq!(fs::read(&wav).expect("after"), before);
    assert!(!dir.join("clip.txt").exists());
    assert_eq!(result.sidecar_status.as_deref(), Some("failed"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn host_cancel_stops_running_job() {
    // Arrange
    let dir = temp_dir("host-cancel");
    let wav = dir.join("clip.wav");
    write_pcm_wav(&wav, 48_000);
    let before = fs::read(&wav).expect("before");
    seed_sidecar(&wav, 13);
    let model = dir.join("ggml-base.bin");
    fs::write(&model, b"ggml").expect("model");

    // Act
    let result = run_transcription_flow(
        FakeInferenceEngine::slow(800),
        short_config(),
        13,
        &wav,
        &model,
        "base-q5_1",
        Some(Duration::from_millis(50)),
    );

    // Assert
    assert_eq!(result.phase, TranscriptionFlowPhase::Cancelled);
    assert_eq!(fs::read(&wav).expect("after"), before);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn host_cancel_racing_completion_is_stable() {
    // Arrange
    let dir = temp_dir("host-race");
    let wav = dir.join("clip.wav");
    write_pcm_wav(&wav, 4_000);
    seed_sidecar(&wav, 14);
    let model = dir.join("ggml-base.bin");
    fs::write(&model, b"ggml").expect("model");
    let race = Arc::new(AtomicBool::new(true));

    // Act
    let result = run_transcription_flow(
        FakeInferenceEngine::race_cancel(Arc::clone(&race)),
        short_config(),
        14,
        &wav,
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
    assert!(wav.is_file());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn host_invalid_model_fails() {
    // Arrange
    let dir = temp_dir("host-model");
    let wav = dir.join("clip.wav");
    write_pcm_wav(&wav, 2_000);
    let before = fs::read(&wav).expect("before");
    seed_sidecar(&wav, 15);
    let model = dir.join("invalid.bin");
    fs::write(&model, b"nope").expect("model");

    // Act
    let result = run_transcription_flow(
        FakeInferenceEngine::success("x"),
        short_config(),
        15,
        &wav,
        &model,
        "bad",
        None,
    );

    // Assert
    assert_eq!(result.phase, TranscriptionFlowPhase::Failed);
    assert_eq!(fs::read(&wav).expect("after"), before);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn host_long_input_stays_chunked() {
    // Arrange
    let dir = temp_dir("host-chunk");
    let wav = dir.join("long.wav");
    write_pcm_wav(&wav, 24_000);
    seed_sidecar(&wav, 16);
    let model = dir.join("ggml-base.bin");
    fs::write(&model, b"ggml").expect("model");
    let calls = Arc::new(Mutex::new(Vec::new()));
    let engine = FakeInferenceEngine::success_with_calls("chunk", Arc::clone(&calls));

    // Act
    let result =
        run_transcription_flow(engine, short_config(), 16, &wav, &model, "base-q5_1", None);
    let sizes = calls.lock().expect("lock").clone();

    // Assert
    assert_eq!(result.phase, TranscriptionFlowPhase::Done);
    assert!(
        sizes.len() >= 2,
        "expected multiple windowed infer calls, got {sizes:?}"
    );
    let max = sizes.iter().copied().max().unwrap_or(0);
    let window_cap = ((0.200_f64 * 16_000.0).round() as usize) + 64;
    assert!(
        max <= window_cap,
        "infer chunk too large: max={max} cap={window_cap}"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn host_fifo_two_queued_recordings() {
    // Arrange
    let dir = temp_dir("host-fifo");
    let wav_a = dir.join("a.wav");
    let wav_b = dir.join("b.wav");
    write_pcm_wav(&wav_a, 4_000);
    write_pcm_wav(&wav_b, 4_000);
    seed_sidecar(&wav_a, 21);
    seed_sidecar(&wav_b, 22);
    let model = dir.join("ggml-base.bin");
    fs::write(&model, b"ggml").expect("model");
    let order = Arc::new(Mutex::new(Vec::new()));

    // Act
    let seen = skrivli::testing::run_transcription_fifo(
        FakeInferenceEngine::slow(80),
        short_config(),
        &[(21, wav_a.as_path()), (22, wav_b.as_path())],
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
