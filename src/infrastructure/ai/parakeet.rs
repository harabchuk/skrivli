//! parakeet.cpp adapter. This is the only module that holds engine pointers.
//!
//! The engine is the flat C API of the pinned parakeet.cpp submodule, declared by hand in
//! [`ffi`]. Audio goes in as a stream (`stream_feed_json`), and finished words come back
//! with stream-relative times, which [`Segmenter`] groups into sentence segments.

use std::ffi::{c_char, c_int, CStr, CString};
use std::marker::PhantomData;
use std::path::Path;
use std::ptr::NonNull;

use serde::Deserialize;

use crate::common::error::AppError;
use crate::services::ports::{InferencePort, InferenceSegment, SpeechStream};

/// ABI version of `parakeet_capi.h` this adapter was written against.
const EXPECTED_ABI_VERSION: c_int = 10;
/// Sample rate the stream expects.
const STREAM_SAMPLE_RATE: u64 = 16_000;
/// The engine drops and repeats words when it is fed blocks shorter than about one
/// encoder chunk (measured with 0.25 s to 1 s blocks), so audio is gathered to 2 s first.
const MIN_FEED_SAMPLES: usize = 32_000;
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

    #[repr(C)]
    pub(super) struct ParakeetStream {
        _private: [u8; 0],
    }

    extern "C" {
        pub(super) fn parakeet_capi_abi_version() -> c_int;
        pub(super) fn parakeet_capi_load(gguf_path: *const c_char) -> *mut ParakeetCtx;
        pub(super) fn parakeet_capi_load_error() -> *const c_char;
        pub(super) fn parakeet_capi_free(ctx: *mut ParakeetCtx);
        pub(super) fn parakeet_capi_last_error(ctx: *mut ParakeetCtx) -> *const c_char;
        pub(super) fn parakeet_capi_stream_begin(ctx: *mut ParakeetCtx) -> *mut ParakeetStream;
        pub(super) fn parakeet_capi_stream_feed_json(
            stream: *mut ParakeetStream,
            pcm: *const f32,
            n_samples: c_int,
        ) -> *mut c_char;
        pub(super) fn parakeet_capi_stream_finalize_json(
            stream: *mut ParakeetStream,
        ) -> *mut c_char;
        pub(super) fn parakeet_capi_stream_free(stream: *mut ParakeetStream);
        pub(super) fn parakeet_capi_free_string(text: *mut c_char);
    }
}

/// One decoded word with stream-relative times.
#[derive(Clone, Debug, PartialEq)]
struct Word {
    text: String,
    start_ms: u64,
    end_ms: u64,
}

/// What one feed or finalize call returned, parsed from the engine's JSON document.
#[derive(Debug, Default, Deserialize)]
struct StreamChunk {
    #[serde(default)]
    text: String,
    #[serde(default)]
    eou: i64,
    #[serde(default)]
    words: Vec<WordDoc>,
}

#[derive(Debug, Deserialize)]
struct WordDoc {
    w: String,
    start: f64,
    end: f64,
}

/// Groups words into segments that end at a sentence end or an end-of-utterance event.
#[derive(Debug, Default)]
struct Segmenter {
    pending: Vec<Word>,
    /// End of the last closed segment, used when the engine reports text without words.
    last_boundary_ms: u64,
}

/// Gathers small feeds into blocks the engine decodes cleanly.
#[derive(Debug, Default)]
struct FeedBuffer {
    samples: Vec<f32>,
}

impl FeedBuffer {
    /// Add samples. Returns a block to send once enough audio has gathered.
    fn push(&mut self, samples: &[f32]) -> Option<Vec<f32>> {
        self.samples.extend_from_slice(samples);
        (self.samples.len() >= MIN_FEED_SAMPLES).then(|| std::mem::take(&mut self.samples))
    }

    /// The audio still waiting, if any.
    fn take_rest(&mut self) -> Option<Vec<f32>> {
        (!self.samples.is_empty()).then(|| std::mem::take(&mut self.samples))
    }
}

impl Segmenter {
    /// Take one engine document. `stream_ms` is the audio fed so far.
    fn accept(&mut self, chunk: StreamChunk, stream_ms: u64) -> Vec<InferenceSegment> {
        let mut done = Vec::new();
        if chunk.words.is_empty() {
            // No word times: the text covers the audio since the last closed segment.
            let word = Word {
                text: strip_language_tags(&chunk.text),
                start_ms: self.last_boundary_ms.min(stream_ms),
                end_ms: stream_ms,
            };
            self.push(word, &mut done);
        }
        for doc in chunk.words {
            let word = Word {
                text: strip_language_tags(&doc.w),
                start_ms: seconds_to_ms(doc.start),
                end_ms: seconds_to_ms(doc.end),
            };
            self.push(word, &mut done);
        }
        if chunk.eou != 0 {
            self.close(&mut done);
        }
        done
    }

    /// Close the segment in progress at the end of the stream.
    fn flush(&mut self) -> Vec<InferenceSegment> {
        let mut done = Vec::new();
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
        self.last_boundary_ms = end_ms;
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
fn samples_to_ms(samples: u64) -> u64 {
    samples.saturating_mul(1_000) / STREAM_SAMPLE_RATE
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
            // SAFETY: `context` came from `parakeet_capi_load` and is freed exactly once;
            // streams borrow the engine, so none is alive here.
            unsafe { ffi::parakeet_capi_free(context.as_ptr()) };
        }
    }

    fn begin_stream(&mut self) -> Result<Box<dyn SpeechStream + '_>, AppError> {
        let context = self
            .context
            .ok_or_else(|| AppError::message("inference", "model not loaded"))?;
        // SAFETY: `context` is a live context owned by `self`.
        let raw = unsafe { ffi::parakeet_capi_stream_begin(context.as_ptr()) };
        let raw =
            NonNull::new(raw).ok_or_else(|| AppError::message("inference", last_error(context)))?;
        Ok(Box::new(ParakeetStream {
            context,
            raw,
            fed_samples: 0,
            buffer: FeedBuffer::default(),
            segmenter: Segmenter::default(),
            _engine: PhantomData,
        }))
    }
}

/// One streaming session. It borrows the engine, so the model outlives it.
struct ParakeetStream<'a> {
    context: NonNull<ffi::ParakeetCtx>,
    raw: NonNull<ffi::ParakeetStream>,
    fed_samples: u64,
    buffer: FeedBuffer,
    segmenter: Segmenter,
    _engine: PhantomData<&'a mut ParakeetInference>,
}

impl ParakeetStream<'_> {
    /// Parse an engine JSON document and free it. `document` is null on engine failure.
    fn read_document(&self, document: *mut c_char) -> Result<StreamChunk, AppError> {
        if document.is_null() {
            return Err(AppError::message("inference", last_error(self.context)));
        }
        // SAFETY: a non-null result is a NUL-terminated string we own until freed below.
        let text = unsafe { copy_c_str(document) };
        // SAFETY: `document` came from the engine and is freed exactly once.
        unsafe { ffi::parakeet_capi_free_string(document) };
        serde_json::from_str(&text)
            .map_err(|error| AppError::message("inference", format!("bad engine output: {error}")))
    }

    fn stream_ms(&self) -> u64 {
        samples_to_ms(self.fed_samples)
    }
}

impl ParakeetStream<'_> {
    /// Hand one block to the engine and group the words it finished.
    fn send(&mut self, samples: &[f32]) -> Result<Vec<InferenceSegment>, AppError> {
        let count = c_int::try_from(samples.len())
            .map_err(|_| AppError::message("inference", "feed block is too large"))?;
        // SAFETY: `raw` is a live stream, and `samples` is valid for `count` floats.
        let document = unsafe {
            ffi::parakeet_capi_stream_feed_json(self.raw.as_ptr(), samples.as_ptr(), count)
        };
        let chunk = self.read_document(document)?;
        self.fed_samples = self.fed_samples.saturating_add(samples.len() as u64);
        let stream_ms = self.stream_ms();
        Ok(self.segmenter.accept(chunk, stream_ms))
    }
}

impl SpeechStream for ParakeetStream<'_> {
    fn feed(&mut self, samples: &[f32]) -> Result<Vec<InferenceSegment>, AppError> {
        match self.buffer.push(samples) {
            Some(block) => self.send(&block),
            None => Ok(Vec::new()),
        }
    }

    fn finish(mut self: Box<Self>) -> Result<Vec<InferenceSegment>, AppError> {
        let mut segments = match self.buffer.take_rest() {
            Some(rest) => self.send(&rest)?,
            None => Vec::new(),
        };
        // SAFETY: `raw` is a live stream.
        let document = unsafe { ffi::parakeet_capi_stream_finalize_json(self.raw.as_ptr()) };
        let chunk = self.read_document(document)?;
        let stream_ms = self.stream_ms();
        segments.extend(self.segmenter.accept(chunk, stream_ms));
        segments.extend(self.segmenter.flush());
        Ok(segments)
    }
}

impl Drop for ParakeetStream<'_> {
    fn drop(&mut self) {
        // SAFETY: `raw` came from `parakeet_capi_stream_begin` and is freed exactly once.
        unsafe { ffi::parakeet_capi_stream_free(self.raw.as_ptr()) };
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
        samples_to_ms, strip_language_tags, FeedBuffer, ParakeetInference, Segmenter, StreamChunk,
        WordDoc,
    };
    use crate::services::ports::InferencePort;

    fn chunk(eou: bool, words: &[(&str, f64, f64)]) -> StreamChunk {
        StreamChunk {
            text: words
                .iter()
                .map(|(text, _, _)| *text)
                .collect::<Vec<_>>()
                .join(" "),
            eou: i64::from(eou),
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
    fn closes_segment_at_sentence_end() {
        // Arrange
        let mut segmenter = Segmenter::default();
        let words = [
            ("Hello", 0.48, 0.64),
            ("there.", 0.64, 1.0),
            ("Next", 1.2, 1.4),
        ];

        // Act
        let segments = segmenter.accept(chunk(false, &words), 2_000);

        // Assert
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].text, "Hello there.");
        assert_eq!((segments[0].start_ms, segments[0].end_ms), (480, 1_000));
        let rest = segmenter.flush();
        assert_eq!(rest[0].text, "Next");
        assert_eq!((rest[0].start_ms, rest[0].end_ms), (1_200, 1_400));
    }

    #[test]
    fn closes_segment_on_end_of_utterance() {
        // Arrange
        let mut segmenter = Segmenter::default();

        // Act
        let segments = segmenter.accept(chunk(true, &[("yes", 0.0, 0.3)]), 500);

        // Assert
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].text, "yes");
    }

    #[test]
    fn language_tag_words_are_dropped() {
        // Arrange
        let mut segmenter = Segmenter::default();
        let words = [("Hi.", 0.0, 0.2), ("<en-US>", 0.2, 0.2)];

        // Act
        let segments = segmenter.accept(chunk(false, &words), 500);
        let rest = segmenter.flush();

        // Assert
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].text, "Hi.");
        assert!(rest.is_empty());
    }

    #[test]
    fn text_without_words_spans_samples_since_last_boundary() {
        // Arrange
        let mut segmenter = Segmenter::default();
        let no_words = StreamChunk {
            text: "Hello.".into(),
            eou: 0,
            words: Vec::new(),
        };

        // Act
        let segments = segmenter.accept(no_words, 1_000);

        // Assert
        assert_eq!(segments.len(), 1);
        assert_eq!((segments[0].start_ms, segments[0].end_ms), (0, 1_000));
    }

    #[test]
    fn new_adapter_starts_unloaded() {
        // Arrange
        let mut engine = ParakeetInference::new();

        // Act
        let error = engine.begin_stream().err().expect("unloaded");

        // Assert
        assert_eq!(error.user_message(), "Could not run speech recognition.");
        engine.unload();
    }

    #[test]
    fn small_feeds_are_gathered_into_engine_sized_blocks() {
        // Arrange
        let mut buffer = FeedBuffer::default();
        let second = vec![0.0_f32; 16_000];

        // Act
        let first = buffer.push(&second);
        let gathered = buffer.push(&second);
        let rest = buffer.take_rest();
        buffer.push(&second);
        let tail = buffer.take_rest();

        // Assert
        assert!(first.is_none());
        assert_eq!(gathered.map(|block| block.len()), Some(32_000));
        assert!(rest.is_none());
        assert_eq!(tail.map(|block| block.len()), Some(16_000));
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
        assert!(engine.begin_stream().is_err());
    }

    #[test]
    fn long_run_without_sentence_end_is_split() {
        // Arrange
        let mut segmenter = Segmenter::default();
        let words: Vec<(&str, f64, f64)> = (0..130)
            .map(|n| ("word", n as f64, n as f64 + 0.5))
            .collect();

        // Act
        let segments = segmenter.accept(chunk(false, &words), 130_000);
        let rest = segmenter.flush();

        // Assert
        assert_eq!(segments.len(), 2);
        assert_eq!(rest.len(), 1);
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

    /// `SKRIVLI_TEST_MODEL` names a local Nemotron GGUF file.
    #[test]
    #[ignore = "needs the real model file"]
    fn real_model_streams_a_clip() {
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
        let mut stream = engine.begin_stream().expect("stream");
        let mut segments = Vec::new();
        for block in samples.chunks(16_000) {
            segments.extend(stream.feed(block).expect("feed"));
        }
        segments.extend(stream.finish().expect("finish"));
        println!(
            "{:.1} s of audio streamed in {:?}",
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
