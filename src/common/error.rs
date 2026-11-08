use std::fmt;

/// Diagnostic failure. [`AppError::user_message`] is safe to show in the UI.
#[derive(Debug)]
pub(crate) enum AppError {
    Io {
        context: &'static str,
        source: std::io::Error,
    },
    Message {
        context: &'static str,
        detail: String,
    },
}

impl AppError {
    pub(crate) fn io(context: &'static str, source: std::io::Error) -> Self {
        Self::Io { context, source }
    }

    pub(crate) fn message(context: &'static str, detail: impl Into<String>) -> Self {
        Self::Message {
            context,
            detail: detail.into(),
        }
    }

    /// Short text for the UI. It never includes paths, audio, or transcript text.
    pub(crate) fn user_message(&self) -> &'static str {
        let context = match self {
            Self::Io { context, .. } | Self::Message { context, .. } => *context,
        };
        match context {
            "save settings" => "Could not save settings.",
            "create logs directory" | "open log file" | "start logging" => {
                "Could not start logging."
            }
            "prepare folders" => "Could not prepare the Skrivli folders.",
            "single instance" => "Could not check whether Skrivli is already running.",
            "app paths" => "Could not find the user profile folders.",
            "open window" => "Could not open the Skrivli window.",
            "open recordings folder" => "Could not open the recordings folder.",
            "tray" => "Could not show the Skrivli icon.",
            "activation" => "Could not ask the running Skrivli window to open.",
            "recordings folder" => "The recordings folder is not writable.",
            "save recording" => "Could not save the recording.",
            "free space" => "There is not enough free disk space to record.",
            "microphone" => "The microphone is not available.",
            "microphone privacy" => "Microphone access is turned off in Windows privacy settings.",
            "recording limit" => "Recording stopped because it reached the 12 hour limit.",
            "library" | "library rename" | "library delete" | "library watch" => {
                "Could not update the recordings library."
            }
            "playback" => "Could not play the recording.",
            "transcription" | "save transcript" => "Could not prepare the transcript.",
            "transcript identity" => "The recording changed before the transcript was saved.",
            "transcript missing" => "The transcript file is missing.",
            "read transcript" => "The transcript file could not be read.",
            "choose model" => "Could not choose a transcription model file.",
            "model missing" => "The transcription model file is missing.",
            "model unreadable" => "The transcription model file could not be read.",
            "model unsupported" => "The transcription model file is not supported.",
            "model manifest" => "The transcription model list could not be read.",
            "download model" => "Could not download the transcription model.",
            "download cancelled" => "The model download was cancelled.",
            "verify model" => "The downloaded transcription model could not be verified.",
            "load model" => "Could not load the transcription model.",
            "inference" => "Could not run speech recognition.",
            "transcription cancelled" => "Transcription was cancelled.",
            _ => "Something went wrong.",
        }
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { context, source } => write!(f, "{context}: {source}"),
            Self::Message { context, detail } => write!(f, "{context}: {detail}"),
        }
    }
}

impl std::error::Error for AppError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Message { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::AppError;

    #[test]
    fn user_message_omits_diagnostic_detail() {
        // Arrange
        let error = AppError::io(
            "save settings",
            std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                r"C:\Users\me\Documents\Skrivli\take.wav",
            ),
        );

        // Act
        let shown = error.user_message();
        let diagnostic = error.to_string();

        // Assert
        assert_eq!(shown, "Could not save settings.");
        assert!(!shown.contains("take.wav"));
        assert!(diagnostic.contains("save settings"));
        assert!(diagnostic.contains("take.wav"));
        assert!(error.source().is_some());
    }

    #[test]
    fn message_diagnostic_keeps_detail_out_of_user_text() {
        // Arrange
        let error = AppError::message("app paths", "APPDATA is not set");

        // Act
        let shown = error.user_message();

        // Assert
        assert_eq!(shown, "Could not find the user profile folders.");
        assert!(error.to_string().contains("APPDATA is not set"));
        assert!(error.source().is_none());
    }
}
