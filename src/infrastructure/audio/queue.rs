//! Bounded handoff from a capture thread to the mixer.
//!
//! One queue per source (microphone and loopback). Both use this module.
//! Overflow policy (D-04): reject the new packet. `try_send` on a full
//! [`std::sync::mpsc::sync_channel`] does not remove a queued packet and does
//! not wait. The capture loop counts the rejection (`overrun`) and continues.
//! A rejected packet leaves a timestamp gap; the mixer fills that gap with
//! silence instead of stalling the device thread. Disk I/O never runs here.
//!
//! Capacity is [`CAPTURE_QUEUE_CAPACITY`] blocks, not a custom lock-free
//! ring. At a 10 ms device period that is 640 ms of queued audio, which is
//! past the mixer's 150 ms lateness limit. A larger engine period shortens
//! how many milliseconds fit in the same 64 blocks. The cap is the block
//! count either way.

use std::sync::mpsc::{SyncSender, TrySendError};

use super::format::to_internal;
use crate::domain::audio::AudioFormat;

/// About 64 packets. At a 10 ms device period this is a few hundred
/// milliseconds of audio, not an unbounded buffer.
pub(crate) const CAPTURE_QUEUE_CAPACITY: usize = 64;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AudioBlock {
    pub(crate) samples: Vec<f32>,
    /// First output frame on the session timeline. `None` appends.
    pub(crate) timeline_frame: Option<u64>,
    pub(crate) discontinuity: bool,
}

/// `true` when the packet was queued. `false` when it was rejected or the
/// writer is gone.
pub(crate) fn offer_block(tx: &SyncSender<AudioBlock>, block: AudioBlock) -> bool {
    match tx.try_send(block) {
        Ok(()) => true,
        Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => false,
    }
}

pub(crate) fn block_from_packet(
    bytes: &[u8],
    format: AudioFormat,
    timeline_frame: Option<u64>,
    silent: bool,
    discontinuity: bool,
) -> Result<AudioBlock, super::format::ConvertError> {
    let samples = if silent {
        super::format::silence_internal(
            format.frames_from_bytes(bytes.len() as u64).unwrap_or(0),
            format,
        )?
    } else {
        to_internal(bytes, format)?
    };
    let _ = bytes;
    Ok(AudioBlock {
        samples,
        timeline_frame,
        discontinuity,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::sync_channel;

    use super::{offer_block, AudioBlock};

    fn block(mark: f32) -> AudioBlock {
        AudioBlock {
            samples: vec![mark, mark],
            timeline_frame: None,
            discontinuity: false,
        }
    }

    #[test]
    fn a_full_queue_rejects_the_new_packet() {
        // Arrange
        let (tx, rx) = sync_channel(1);
        assert!(offer_block(&tx, block(1.0)));

        // Act
        let accepted = offer_block(&tx, block(2.0));
        let queued = rx.try_recv().expect("first");

        // Assert
        assert!(!accepted);
        assert_eq!(queued.samples, vec![1.0, 1.0]);
    }
}
