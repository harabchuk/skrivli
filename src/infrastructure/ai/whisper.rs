//! whisper.cpp adapter. This is the only module that holds engine pointers.

use std::ffi::c_void;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use whisper_rs::{
    FullParams, SamplingStrategy, SegmentCallbackData, WhisperContext, WhisperContextParameters,
};

use crate::common::error::AppError;
use crate::services::ports::{InferencePort, InferenceSegment};

/// Engine abort check. `user_data` must point to a live `AtomicBool`.
unsafe extern "C" fn abort_requested(user_data: *mut c_void) -> bool {
    // SAFETY: the caller installs a pointer to an `AtomicBool` that outlives the decode.
    unsafe { &*(user_data as *const AtomicBool) }.load(Ordering::Acquire)
}

/// whisper.cpp reports segment times in 10 ms units.
fn engine_time_to_ms(engine_time: i64) -> u64 {
    (engine_time.max(0) as u64).saturating_mul(10)
}

/// Owns a whisper.cpp context on the calling (worker) thread.
pub(crate) struct WhisperInference {
    context: Option<WhisperContext>,
}

impl WhisperInference {
    pub(crate) fn new() -> Self {
        Self { context: None }
    }
}

impl Default for WhisperInference {
    fn default() -> Self {
        Self::new()
    }
}

impl InferencePort for WhisperInference {
    fn load(&mut self, model_path: &Path) -> Result<(), AppError> {
        self.unload();
        let path = model_path
            .to_str()
            .ok_or_else(|| AppError::message("load model", "model path is not valid UTF-8"))?;
        // Engine chatter stays on its own streams; never forward into tracing.
        let context = WhisperContext::new_with_params(path, WhisperContextParameters::default())
            .map_err(|error| AppError::message("load model", error.to_string()))?;
        self.context = Some(context);
        Ok(())
    }

    fn unload(&mut self) {
        self.context = None;
    }

    fn transcribe_chunk(
        &mut self,
        samples_f32_16k_mono: &[f32],
        cancel: &Arc<AtomicBool>,
        on_progress: Arc<dyn Fn(u8) + Send + Sync>,
    ) -> Result<Vec<InferenceSegment>, AppError> {
        let context = self
            .context
            .as_ref()
            .ok_or_else(|| AppError::message("inference", "model not loaded"))?;
        if cancel.load(Ordering::Acquire) {
            return Err(AppError::message(
                "transcription cancelled",
                "cancel before start",
            ));
        }

        let mut state = context
            .create_state()
            .map_err(|error| AppError::message("inference", error.to_string()))?;

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_print_progress(false);
        params.set_print_special(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);

        // whisper-rs 0.16 `set_abort_callback_safe` casts its boxed closure to the wrong
        // type, so the engine reads garbage and aborts at the end of encoding (error -6).
        // SAFETY: `cancel` outlives `state.full` below, the only call that uses this
        // pointer, and `abort_requested` only reads the atomic behind it.
        unsafe {
            params.set_abort_callback(Some(abort_requested));
            params.set_abort_callback_user_data(Arc::as_ptr(cancel) as *mut c_void);
        }

        let progress = Arc::clone(&on_progress);
        params.set_progress_callback_safe(move |percent: i32| {
            let clamped = percent.clamp(0, 100) as u8;
            progress(clamped);
        });

        let segments = Arc::new(Mutex::new(Vec::<InferenceSegment>::new()));
        let segments_cb = Arc::clone(&segments);
        params.set_segment_callback_safe(move |data: SegmentCallbackData| {
            if let Ok(mut guard) = segments_cb.lock() {
                guard.push(InferenceSegment {
                    start_ms: engine_time_to_ms(data.start_timestamp),
                    end_ms: engine_time_to_ms(data.end_timestamp),
                    text: data.text,
                });
            }
        });

        let result = state.full(params, samples_f32_16k_mono);

        if cancel.load(Ordering::Acquire) {
            return Err(AppError::message(
                "transcription cancelled",
                "abort callback",
            ));
        }

        result.map_err(|error| AppError::message("inference", error.to_string()))?;

        let out = segments
            .lock()
            .map_err(|_| AppError::message("inference", "segment lock poisoned"))?
            .clone();
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::{abort_requested, engine_time_to_ms, WhisperInference};
    use crate::services::ports::InferencePort;
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    #[test]
    fn abort_check_follows_the_cancel_flag() {
        // Arrange
        let cancel = Arc::new(AtomicBool::new(false));
        let user_data = Arc::as_ptr(&cancel) as *mut c_void;

        // Act
        // SAFETY: `cancel` is alive for both calls.
        let before = unsafe { abort_requested(user_data) };
        cancel.store(true, Ordering::Release);
        let after = unsafe { abort_requested(user_data) };

        // Assert
        assert!(!before);
        assert!(after);
    }

    #[test]
    fn engine_time_converts_to_milliseconds() {
        // Arrange
        let engine_time = 150;

        // Act
        let ms = engine_time_to_ms(engine_time);

        // Assert
        assert_eq!(ms, 1_500);
        assert_eq!(engine_time_to_ms(-1), 0);
    }

    #[test]
    fn new_adapter_starts_unloaded() {
        // Arrange
        let mut engine = WhisperInference::new();
        let cancel = Arc::new(AtomicBool::new(false));

        // Act
        let error = engine
            .transcribe_chunk(&[0.0; 16], &cancel, Arc::new(|_| {}))
            .expect_err("unloaded");

        // Assert
        assert_eq!(error.user_message(), "Could not run speech recognition.");
        engine.unload();
    }

    /// `SKRIVLI_TEST_MODEL` names a local ggml model file.
    #[test]
    #[ignore = "needs a real whisper model file"]
    fn real_model_decodes_a_chunk_without_aborting() {
        // Arrange
        let model = std::env::var("SKRIVLI_TEST_MODEL").expect("SKRIVLI_TEST_MODEL");
        let mut engine = WhisperInference::new();
        engine.load(std::path::Path::new(&model)).expect("load");
        let cancel = Arc::new(AtomicBool::new(false));
        let samples: Vec<f32> = (0..16_000 * 3)
            .map(|n| (n as f32 * 0.05).sin() * 0.2)
            .collect();

        // Act
        let result = engine.transcribe_chunk(&samples, &cancel, Arc::new(|_| {}));

        // Assert
        assert!(result.is_ok(), "{:?}", result.err());
    }
}
