//! Status-bar notices and failure reasons in the chosen language.
//!
//! Services and `AppError` hand the UI fixed English sentences, and several places compare
//! them to decide what to do. They stay as they are. The views translate them only when they
//! show them. A sentence that is not in the table (for example, one an OS call produced) is
//! shown as it is.

use gpui_kit::SharedString;
use rust_i18n::t;

use crate::services::recording::{
    NOTICE_DISK, NOTICE_FALLBACK, NOTICE_FOLDER, NOTICE_FORMAT, NOTICE_LIMIT, NOTICE_LOW_DISK,
    NOTICE_MIC, NOTICE_MIC_MISSING, NOTICE_NO_SOURCES, NOTICE_PRIVACY, NOTICE_SAVE, NOTICE_SAVED,
    NOTICE_STALL, NOTICE_START, NOTICE_SYSTEM_FALLBACK, NOTICE_SYSTEM_MISSING, NOTICE_WORKER,
};
use crate::services::transcription::{NOTICE_CANCELLED, NOTICE_DONE, NOTICE_FAILED, NOTICE_MODEL};

use super::state::NOTICE_FAILED_KEPT;

pub(crate) const NOTICE_RECORDING_NOT_FOUND: &str = "Could not find that recording.";
pub(crate) const NOTICE_STILL_RUNNING: &str = "Skrivli is still running in the notification area.";

/// English sentence and its catalog key under `app.notice.`.
const KNOWN: &[(&str, &str)] = &[
    (NOTICE_SAVED, "recording_saved"),
    (NOTICE_PRIVACY, "microphone_privacy"),
    (NOTICE_MIC, "microphone_unavailable"),
    (NOTICE_NO_SOURCES, "no_sources"),
    (NOTICE_FORMAT, "microphone_format"),
    (NOTICE_START, "start_failed"),
    (NOTICE_FOLDER, "folder_not_writable"),
    (NOTICE_DISK, "disk_full"),
    (NOTICE_LOW_DISK, "disk_low"),
    (NOTICE_SAVE, "save_failed"),
    (NOTICE_LIMIT, "limit_reached"),
    (NOTICE_FALLBACK, "microphone_fallback"),
    (NOTICE_SYSTEM_FALLBACK, "output_fallback"),
    (NOTICE_SYSTEM_MISSING, "system_missing"),
    (NOTICE_MIC_MISSING, "microphone_missing"),
    (NOTICE_WORKER, "worker_failed"),
    (NOTICE_STALL, "capture_stalled"),
    (NOTICE_DONE, "transcript_ready"),
    (NOTICE_FAILED, "transcription_failed"),
    (NOTICE_CANCELLED, "transcription_cancelled"),
    (NOTICE_MODEL, "model_missing_or_unsupported"),
    (NOTICE_FAILED_KEPT, "transcription_failed_kept"),
    (NOTICE_RECORDING_NOT_FOUND, "recording_not_found"),
    (NOTICE_STILL_RUNNING, "still_running"),
    (
        "The recording changed before the transcript was saved.",
        "recording_changed",
    ),
    (
        "Some related files were not renamed.",
        "related_files_not_renamed",
    ),
    ("Could not save settings.", "save_settings"),
    ("Could not start logging.", "start_logging"),
    ("Could not prepare the Skrivli folders.", "prepare_folders"),
    (
        "Could not check whether Skrivli is already running.",
        "single_instance",
    ),
    ("Could not find the user profile folders.", "app_paths"),
    ("Could not open the Skrivli window.", "open_window"),
    ("Could not open the recordings folder.", "open_folder"),
    ("Could not show the Skrivli icon.", "tray"),
    (
        "Could not ask the running Skrivli window to open.",
        "activation",
    ),
    ("Could not update the recordings library.", "library"),
    ("Could not play the recording.", "playback"),
    ("Could not prepare the transcript.", "prepare_transcript"),
    ("The transcript file is missing.", "transcript_missing"),
    (
        "The transcript file could not be read.",
        "transcript_unreadable",
    ),
    (
        "Could not choose a transcription model file.",
        "choose_model",
    ),
    (
        "The transcription model file is missing.",
        "model_file_missing",
    ),
    (
        "The transcription model file could not be read.",
        "model_file_unreadable",
    ),
    (
        "The transcription model file is not supported.",
        "model_file_unsupported",
    ),
    (
        "The transcription model list could not be read.",
        "model_list_unreadable",
    ),
    (
        "Could not download the transcription model.",
        "download_failed",
    ),
    ("The model download was cancelled.", "download_cancelled"),
    (
        "The downloaded transcription model could not be verified.",
        "verify_failed",
    ),
    ("Could not load the transcription model.", "load_failed"),
    ("Could not run speech recognition.", "speech_recognition"),
    (
        "Transcription was cancelled.",
        "transcription_was_cancelled",
    ),
    ("Something went wrong.", "generic"),
];

fn key_for(message: &str) -> Option<String> {
    KNOWN
        .iter()
        .find(|(english, _)| *english == message)
        .map(|(_, id)| format!("app.notice.{id}"))
}

/// `message` in the active language, or as it is when it is not a known sentence.
pub(crate) fn localize(message: &str) -> SharedString {
    match key_for(message) {
        Some(key) => SharedString::from(t!(key).into_owned()),
        None => SharedString::from(message.to_string()),
    }
}

/// Same as [`localize`] for a fixed language.
#[cfg(test)]
fn localize_in(message: &str, locale: &str) -> String {
    match key_for(message) {
        Some(key) => t!(key, locale = locale).into_owned(),
        None => message.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{key_for, localize, localize_in, KNOWN};

    #[test]
    fn english_catalog_matches_every_known_sentence() {
        // Arrange
        let sentences: Vec<_> = KNOWN.iter().map(|(english, _)| *english).collect();

        // Act
        let shown: Vec<_> = sentences
            .iter()
            .map(|sentence| localize_in(sentence, "en"))
            .collect();

        // Assert
        assert_eq!(shown, sentences);
    }

    #[test]
    fn swedish_catalog_translates_every_known_sentence() {
        // Arrange
        let sentences: Vec<_> = KNOWN.iter().map(|(english, _)| *english).collect();

        // Act / Assert
        for sentence in sentences {
            let shown = localize_in(sentence, "sv");
            assert_ne!(shown, sentence, "{sentence}");
            assert!(!shown.starts_with("sv.app."), "{sentence}");
        }
    }

    #[test]
    fn unknown_sentences_are_shown_as_they_are() {
        // Arrange
        let message = "Windows said no.";

        // Act
        let shown = localize(message);

        // Assert
        assert!(key_for(message).is_none());
        assert_eq!(shown.as_ref(), message);
    }

    #[test]
    fn every_known_key_is_unique_per_sentence() {
        // Arrange
        let mut ids: Vec<_> = KNOWN.iter().map(|(_, id)| *id).collect();

        // Act
        let total = ids.len();
        ids.sort_unstable();
        ids.dedup();

        // Assert
        assert_eq!(ids.len(), total);
    }
}
