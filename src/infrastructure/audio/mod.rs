//! Audio adapters. Capture uses flexaudio; OS clocks and device wake live under platform.

pub(crate) mod devices;
pub(crate) mod flex_capture;
pub(crate) mod format;
pub(crate) mod levels;
pub(crate) mod mixer;
pub(crate) mod queue;
pub(crate) mod resampler;
