//! Recording and transcription identifiers.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct RecordingId(u64);

impl RecordingId {
    pub(crate) fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    pub(crate) fn raw(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct JobId(u64);

impl JobId {
    pub(crate) fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    #[cfg_attr(not(test), allow(dead_code))] // Compared in CancelTranscription; raw used by tests.
    pub(crate) fn raw(self) -> u64 {
        self.0
    }
}
