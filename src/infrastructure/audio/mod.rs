//! Audio adapters. Capture uses flexaudio; COM helpers stay for device watch.

pub(crate) mod capture;
pub(crate) mod devices;
pub(crate) mod flex_capture;
pub(crate) mod format;
pub(crate) mod levels;
pub(crate) mod mixer;
pub(crate) mod queue;
pub(crate) mod resampler;
pub(crate) mod system;
