//! whisper.cpp adapter. This is the only module that holds engine pointers.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use whisper_rs::{
    FullParams, SamplingStrategy, SegmentCallbackData, WhisperContext, WhisperContextParameters,
};

use crate::common::error::AppError;
use crate::services::ports::{InferencePort, InferenceSegment};

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
            .map_err(|error| AppError::message("model unsupported", error.to_string()))?;
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

        let abort = Arc::clone(cancel);
        params.set_abort_callback_safe(move || abort.load(Ordering::Acquire));

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
                    start_ms: data.start_timestamp.max(0) as u64,
                    end_ms: data.end_timestamp.max(0) as u64,
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
    use super::WhisperInference;
    use crate::services::ports::InferencePort;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

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
}
