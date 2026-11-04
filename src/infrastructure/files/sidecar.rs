//! Sidecar write path used by the recording service.
//!
//! Load/validate live in `infrastructure::storage::sidecar`. This module
//! re-exports the write surface so existing recording call sites stay stable.

pub(crate) use crate::infrastructure::storage::sidecar::{write_sidecar, Sidecar};
