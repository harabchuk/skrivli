//! File adapters for recordings.

pub(crate) mod player;
// Unused after phase 1 of the remove-rescue plan; deleted in phase 2.
#[allow(dead_code)]
pub(crate) mod rescue;
pub(crate) mod transcript_reader;
pub(crate) mod wav;
pub(crate) mod writable;
