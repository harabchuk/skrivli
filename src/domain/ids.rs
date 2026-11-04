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
    #[allow(dead_code)]
    pub(crate) fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    #[allow(dead_code)]
    pub(crate) fn raw(self) -> u64 {
        self.0
    }
}
