//! Session audio path after capture.
//!
//! [`Pipeline`] resamples each source, places blocks, downmixes the
//! microphone, and runs the mic, system, and mix chains. Production
//! microphone gain is `SonoraMicGain`. When system audio was requested,
//! `SonoraEchoCanceller` runs on the raw microphone before that gain.

pub(crate) mod aligner;
mod chain;
mod echo_stats;
mod pipeline;
mod steps;

pub(crate) use aligner::{Aligner, AudioSource};
pub(crate) use chain::Chain;
pub(crate) use echo_stats::{EchoStats, EchoStatsSlot};
pub(crate) use pipeline::{PassThroughPipelineFactory, Pipeline, PipelineSetup, PipelineStages};
pub(crate) use steps::{FixedGain, SoftLimiter, SumMixer};
// A later meter or stem is one `.tap(...)` at the chain site. The session
// factory does not install these yet, so the names stay unused.
#[allow(unused_imports)]
pub(crate) use steps::{AudioLevelStats, ChannelTap, StatsTap};
