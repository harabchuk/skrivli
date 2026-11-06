//! Audio adapters. Capture uses flexaudio; OS clocks and device wake live under platform.

pub(crate) mod devices;
pub(crate) mod flex_capture;
pub(crate) mod format;
pub(crate) mod levels;
pub(crate) mod queue;
pub(crate) mod resampler;
#[cfg(test)]
mod sonora_spike;
pub(crate) mod sonora_steps;
