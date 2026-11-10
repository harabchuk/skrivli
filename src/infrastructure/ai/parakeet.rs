//! parakeet.cpp adapter. This is the only module that holds engine pointers.
//!
//! The engine is the flat C API of the pinned parakeet.cpp submodule, declared by hand in
//! [`ffi`]. Each piece of audio goes in as one offline clip
//! (`transcribe_pcm_batch_json`), and the words come back with piece-relative times, which
//! [`Segmenter`] groups into sentence segments.

use std::ffi::{c_char, c_int, CStr, CString};
use std::path::Path;
use std::ptr::NonNull;

use serde::Deserialize;

use crate::common::error::AppError;
use crate::services::ports::{InferencePort, InferenceSegment};

/// ABI version of `parakeet_capi.h` this adapter was written against.
const EXPECTED_ABI_VERSION: c_int = 10;
/// Sample rate the engine is given.
const SAMPLE_RATE: c_int = 16_000;
/// Pieces shorter than 100 ms hold no word and are not worth an engine call.
const MIN_PIECE_SAMPLES: usize = 1_600;
/// `decoder` argument for "the model's own decoder" (RNN-T for Nemotron, TDT for TDT v3).
const DECODER_DEFAULT: c_int = 0;
/// Characters that end a sentence and so close a segment.
const SENTENCE_END: [char; 6] = ['.', '?', '!', '。', '？', '！'];
/// A segment is closed after this many words even without a sentence end.
const MAX_WORDS_PER_SEGMENT: usize = 60;

/// Hand-written declarations for the parakeet.cpp C API (`include/parakeet_capi.h`).
mod ffi {
    use std::ffi::{c_char, c_int};

    #[repr(C)]
    pub(super) struct ParakeetCtx {
        _private: [u8; 0],
    }

    extern "C" {
        pub(super) fn parakeet_capi_abi_version() -> c_int;
        pub(super) fn parakeet_capi_load(gguf_path: *const c_char) -> *mut ParakeetCtx;
        pub(super) fn parakeet_capi_load_error() -> *const c_char;
        pub(super) fn parakeet_capi_free(ctx: *mut ParakeetCtx);
        pub(super) fn parakeet_capi_last_error(ctx: *mut ParakeetCtx) -> *const c_char;
        pub(super) fn parakeet_capi_transcribe_pcm_batch_json(
            ctx: *mut ParakeetCtx,
            samples_concat: *const f32,
            n_samples: *const c_int,
            n_clips: c_int,
            sample_rate: c_int,
            decoder: c_int,
        ) -> *mut c_char;
        pub(super) fn parakeet_capi_free_string(text: *mut c_char);
    }
}

/// One decoded word with piece-relative times.
#[derive(Clone, Debug, PartialEq)]
struct Word {
    text: String,
    start_ms: u64,
    end_ms: u64,
}

/// One clip's result, parsed from the engine's JSON document.
#[derive(Debug, Default, Deserialize)]
struct ClipDoc {
    #[serde(default)]
    text: String,
    #[serde(default)]
    words: Vec<WordDoc>,
}

#[derive(Debug, Deserialize)]
struct WordDoc {
    w: String,
    start: f64,
    end: f64,
}

/// Groups words into segments that end at a sentence end or at the word cap.
#[derive(Debug, Default)]
struct Segmenter {
    pending: Vec<Word>,
}

impl Segmenter {
    /// Group one clip. `piece_ms` is the length of the clip, used when the engine reports
    /// text without word times. Closes the segment in progress at the end.
    fn accept(&mut self, clip: ClipDoc, piece_ms: u64) -> Vec<InferenceSegment> {
        let mut done = Vec::new();
        if clip.words.is_empty() {
            // No word times: the text covers the whole piece.
            let word = Word {
                text: strip_language_tags(&clip.text),
                start_ms: 0,
                end_ms: piece_ms,
            };
            self.push(word, &mut done);
        }
        for doc in clip.words {
            let word = Word {
                text: strip_language_tags(&doc.w),
                start_ms: seconds_to_ms(doc.start),
                end_ms: seconds_to_ms(doc.end),
            };
            self.push(word, &mut done);
        }
        self.close(&mut done);
        done
    }

    fn push(&mut self, word: Word, done: &mut Vec<InferenceSegment>) {
        if word.text.is_empty() {
            return;
        }
        let ends_sentence = word.text.ends_with(SENTENCE_END);
        self.pending.push(word);
        if ends_sentence || self.pending.len() >= MAX_WORDS_PER_SEGMENT {
            self.close(done);
        }
    }

    fn close(&mut self, done: &mut Vec<InferenceSegment>) {
        let (Some(first), Some(last)) = (self.pending.first(), self.pending.last()) else {
            return;
        };
        let start_ms = first.start_ms;
        let end_ms = last.end_ms.max(start_ms);
        let text = self
            .pending
            .iter()
            .map(|word| word.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        self.pending.clear();
        done.push(InferenceSegment {
            start_ms,
            end_ms,
            text,
        });
    }
}

/// Remove language tags such as `<en-US>` that auto language ID appends to sentences,
/// and normalise whitespace.
fn strip_language_tags(text: &str) -> String {
    let mut kept = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('<') {
        kept.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('>') {
            Some(close) if is_tag_body(&after[..close]) => rest = &after[close + 1..],
            _ => {
                kept.push('<');
                rest = after;
            }
        }
    }
    kept.push_str(rest);
    kept.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn is_tag_body(body: &str) -> bool {
    (2..=12).contains(&body.len())
        && body
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn seconds_to_ms(seconds: f64) -> u64 {
    (seconds.max(0.0) * 1_000.0).round() as u64
}

/// Milliseconds of audio in `samples` 16 kHz samples.
fn samples_to_ms(samples: usize) -> u64 {
    (samples as u64).saturating_mul(1_000) / SAMPLE_RATE as u64
}

/// Parse the engine's batch document (an array with one object per clip) for one clip.
fn segments_from_document(
    document: &str,
    piece_ms: u64,
) -> Result<Vec<InferenceSegment>, AppError> {
    let mut clips: Vec<ClipDoc> = serde_json::from_str(document)
        .map_err(|error| AppError::message("inference", format!("bad engine output: {error}")))?;
    if clips.is_empty() {
        return Err(AppError::message("inference", "engine returned no clip"));
    }
    Ok(Segmenter::default().accept(clips.swap_remove(0), piece_ms))
}

/// Owns a loaded parakeet.cpp model context.
pub(crate) struct ParakeetInference {
    context: Option<NonNull<ffi::ParakeetCtx>>,
}

// SAFETY: parakeet.cpp documents that one context may be used from several threads (calls
// queue behind one compute backend, and the last-error buffer is written under a lock),
// so moving the owning handle to the transcription worker is sound. The handle is not
// `Sync`: only the owner calls into the engine.
unsafe impl Send for ParakeetInference {}

impl ParakeetInference {
    pub(crate) fn new() -> Self {
        Self { context: None }
    }
}

impl Default for ParakeetInference {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ParakeetInference {
    fn drop(&mut self) {
        self.unload();
    }
}

impl InferencePort for ParakeetInference {
    fn load(&mut self, model_path: &Path) -> Result<(), AppError> {
        self.unload();
        // SAFETY: plain call with no arguments.
        let abi = unsafe { ffi::parakeet_capi_abi_version() };
        if abi != EXPECTED_ABI_VERSION {
            return Err(AppError::message(
                "load model",
                format!("parakeet.cpp ABI {abi}, expected {EXPECTED_ABI_VERSION}"),
            ));
        }
        let path = model_path
            .to_str()
            .ok_or_else(|| AppError::message("load model", "model path is not valid UTF-8"))?;
        let path = CString::new(path)
            .map_err(|_| AppError::message("load model", "model path contains a NUL byte"))?;
        // SAFETY: `path` is a valid NUL-terminated string for the duration of the call.
        let raw = unsafe { ffi::parakeet_capi_load(path.as_ptr()) };
        let Some(context) = NonNull::new(raw) else {
            // SAFETY: the engine returns a thread-local, NUL-terminated message.
            let reason = unsafe { copy_c_str(ffi::parakeet_capi_load_error()) };
            return Err(AppError::message("load model", reason));
        };
        self.context = Some(context);
        Ok(())
    }

    fn unload(&mut self) {
        if let Some(context) = self.context.take() {
            // SAFETY: `context` came from `parakeet_capi_load` and is freed exactly once.
            unsafe { ffi::parakeet_capi_free(context.as_ptr()) };
        }
    }

    fn transcribe(&mut self, samples_16k_mono: &[f32]) -> Result<Vec<InferenceSegment>, AppError> {
        if samples_16k_mono.len() < MIN_PIECE_SAMPLES {
            return Ok(Vec::new());
        }
        let context = self
            .context
            .ok_or_else(|| AppError::message("inference", "model not loaded"))?;
        let count = c_int::try_from(samples_16k_mono.len())
            .map_err(|_| AppError::message("inference", "piece is too large"))?;
        // SAFETY: `context` is a live context owned by `self`; `samples_16k_mono` holds
        // exactly `count` floats and one clip is announced, so the sum of lengths matches;
        // `count` outlives the call.
        let document = unsafe {
            ffi::parakeet_capi_transcribe_pcm_batch_json(
                context.as_ptr(),
                samples_16k_mono.as_ptr(),
                &count,
                1,
                SAMPLE_RATE,
                DECODER_DEFAULT,
            )
        };
        if document.is_null() {
            return Err(AppError::message("inference", last_error(context)));
        }
        // SAFETY: a non-null result is a NUL-terminated string we own until freed below.
        let text = unsafe { copy_c_str(document) };
        // SAFETY: `document` came from the engine and is freed exactly once.
        unsafe { ffi::parakeet_capi_free_string(document) };
        segments_from_document(&text, samples_to_ms(samples_16k_mono.len()))
    }
}

/// The engine's last error for `context`, copied.
fn last_error(context: NonNull<ffi::ParakeetCtx>) -> String {
    // SAFETY: `context` is live; the returned string is owned by it and copied at once.
    unsafe { copy_c_str(ffi::parakeet_capi_last_error(context.as_ptr())) }
}

/// Copy a C string owned by the engine into Rust.
///
/// # Safety
/// `text` must be null or point to a NUL-terminated string that stays valid for the call.
unsafe fn copy_c_str(text: *const c_char) -> String {
    if text.is_null() {
        return String::new();
    }
    // SAFETY: guaranteed by the caller.
    unsafe { CStr::from_ptr(text) }
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::{
        samples_to_ms, segments_from_document, strip_language_tags, ClipDoc, ParakeetInference,
        Segmenter, WordDoc,
    };
    use crate::services::ports::InferencePort;

    fn clip(words: &[(&str, f64, f64)]) -> ClipDoc {
        ClipDoc {
            text: words
                .iter()
                .map(|(text, _, _)| *text)
                .collect::<Vec<_>>()
                .join(" "),
            words: words
                .iter()
                .map(|(w, start, end)| WordDoc {
                    w: (*w).to_string(),
                    start: *start,
                    end: *end,
                })
                .collect(),
        }
    }

    #[test]
    fn strips_language_tags() {
        // Arrange
        let text = "Hello there. <en-US>";

        // Act
        let cleaned = strip_language_tags(text);

        // Assert
        assert_eq!(cleaned, "Hello there.");
    }

    #[test]
    fn keeps_angle_brackets_that_are_not_language_tags() {
        // Arrange
        let text = "if a < b and c > d";

        // Act
        let cleaned = strip_language_tags(text);

        // Assert
        assert_eq!(cleaned, "if a < b and c > d");
    }

    #[test]
    fn sixteen_thousand_samples_are_one_second() {
        // Arrange
        let samples = 16_000;

        // Act
        let ms = samples_to_ms(samples);

        // Assert
        assert_eq!(ms, 1_000);
    }

    #[test]
    fn closes_segment_at_sentence_end_and_flushes_the_rest() {
        // Arrange
        let mut segmenter = Segmenter::default();
        let words = [
            ("Hello", 0.48, 0.64),
            ("there.", 0.64, 1.0),
            ("Next", 1.2, 1.4),
        ];

        // Act
        let segments = segmenter.accept(clip(&words), 2_000);

        // Assert
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].text, "Hello there.");
        assert_eq!((segments[0].start_ms, segments[0].end_ms), (480, 1_000));
        assert_eq!(segments[1].text, "Next");
        assert_eq!((segments[1].start_ms, segments[1].end_ms), (1_200, 1_400));
    }

    #[test]
    fn language_tag_words_are_dropped() {
        // Arrange
        let mut segmenter = Segmenter::default();
        let words = [("Hi.", 0.0, 0.2), ("<en-US>", 0.2, 0.2)];

        // Act
        let segments = segmenter.accept(clip(&words), 500);

        // Assert
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].text, "Hi.");
    }

    #[test]
    fn text_without_words_spans_the_piece() {
        // Arrange
        let mut segmenter = Segmenter::default();
        let no_words = ClipDoc {
            text: "Hello.".into(),
            words: Vec::new(),
        };

        // Act
        let segments = segmenter.accept(no_words, 1_000);

        // Assert
        assert_eq!(segments.len(), 1);
        assert_eq!((segments[0].start_ms, segments[0].end_ms), (0, 1_000));
    }

    #[test]
    fn long_run_without_sentence_end_is_split() {
        // Arrange
        let mut segmenter = Segmenter::default();
        let words: Vec<(&str, f64, f64)> = (0..130)
            .map(|n| ("word", n as f64, n as f64 + 0.5))
            .collect();

        // Act
        let segments = segmenter.accept(clip(&words), 130_000);

        // Assert
        assert_eq!(segments.len(), 3);
    }

    #[test]
    fn parses_the_batch_document_for_one_clip() {
        // Arrange
        let document = r#"[{"text":"Hej då.","words":[{"w":"Hej","start":0.24,"end":0.4,"conf":0.9},{"w":"då.","start":0.4,"end":0.72,"conf":0.8}],"tokens":[]}]"#;

        // Act
        let segments = segments_from_document(document, 1_000).expect("parse");

        // Assert
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].text, "Hej då.");
        assert_eq!((segments[0].start_ms, segments[0].end_ms), (240, 720));
    }

    #[test]
    fn malformed_or_empty_documents_are_inference_errors() {
        // Arrange
        let documents = ["not json", "[]"];

        // Act
        let errors: Vec<_> = documents
            .iter()
            .map(|document| segments_from_document(document, 1_000).expect_err("rejected"))
            .collect();

        // Assert
        for error in errors {
            assert_eq!(error.user_message(), "Could not run speech recognition.");
        }
    }

    #[test]
    fn new_adapter_starts_unloaded() {
        // Arrange
        let mut engine = ParakeetInference::new();

        // Act
        let error = engine.transcribe(&[0.0; 16_000]).expect_err("unloaded");

        // Assert
        assert_eq!(error.user_message(), "Could not run speech recognition.");
        engine.unload();
    }

    #[test]
    fn an_empty_piece_needs_no_engine() {
        // Arrange
        let mut engine = ParakeetInference::new();

        // Act
        let segments = engine.transcribe(&[]).expect("empty piece");

        // Assert
        assert!(segments.is_empty());
    }

    #[test]
    fn linked_engine_matches_the_expected_abi() {
        // Arrange
        let expected = super::EXPECTED_ABI_VERSION;

        // Act
        // SAFETY: plain call with no arguments.
        let abi = unsafe { super::ffi::parakeet_capi_abi_version() };

        // Assert
        assert_eq!(abi, expected);
    }

    #[test]
    fn loading_a_missing_file_is_a_model_error() {
        // Arrange
        let mut engine = ParakeetInference::new();

        // Act
        let error = engine
            .load(std::path::Path::new("definitely-missing.gguf"))
            .expect_err("missing");

        // Assert
        assert_eq!(
            error.user_message(),
            "Could not load the transcription model."
        );
        assert!(engine.transcribe(&[0.0; 16_000]).is_err());
    }

    /// Reads a 16 kHz mono 16-bit WAV with a 44-byte header (the parakeet.cpp fixtures).
    fn read_fixture_wav(path: &std::path::Path) -> Vec<f32> {
        let bytes = std::fs::read(path).expect("wav");
        bytes[44..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| f32::from(i16::from_le_bytes(*pair)) / 32_768.0)
            .collect()
    }

    /// `SKRIVLI_TEST_MODEL` names a local GGUF model file.
    #[test]
    #[ignore = "needs the real model file"]
    fn real_model_transcribes_a_clip() {
        // Arrange
        let model = std::env::var("SKRIVLI_TEST_MODEL").expect("SKRIVLI_TEST_MODEL");
        let clip = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("third_party/parakeet.cpp/tests/fixtures/speech.wav");
        let samples = read_fixture_wav(&clip);
        let mut engine = ParakeetInference::new();
        let started = std::time::Instant::now();
        engine.load(std::path::Path::new(&model)).expect("load");
        println!("load took {:?}", started.elapsed());

        // Act
        let started = std::time::Instant::now();
        let segments = engine.transcribe(&samples).expect("transcribe");
        println!(
            "{:.1} s of audio transcribed in {:?}",
            samples.len() as f64 / 16_000.0,
            started.elapsed()
        );

        // Assert
        assert!(!segments.is_empty());
        let text = segments
            .iter()
            .map(|segment| segment.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(!text.contains('<'), "language tag leaked: {text}");
        assert!(segments
            .windows(2)
            .all(|pair| pair[0].start_ms <= pair[1].start_ms));
        for segment in &segments {
            println!("{}..{} {}", segment.start_ms, segment.end_ms, segment.text);
        }
    }
}
