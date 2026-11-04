//! Session timeline from QPC ticks to 48 kHz frames.
//!
//! Conversion is integer division from a fixed origin `t0`. Repeating it does
//! not accumulate a floating-point error. Device latency is a fixed tick
//! offset subtracted before that division.
//!
//! Packet placement:
//! - A sudden hole of at least 1 ms (the 5 ms case included) becomes silence.
//! - A sudden overlap is discarded from the front of the late packet.
//! - Gradual clock error is absorbed until it passes [`DRIFT_THRESHOLD_FRAMES`]
//!   (20 ms). Past that, frames are dropped or duplicated. Adaptive resampling
//!   is not done here (D-06).

use super::audio::SessionOrigin;

/// One mixed block: 10 ms at 48 kHz.
pub(crate) const MIX_BLOCK_FRAMES: u64 = 480;
/// Sparse correction starts after about 20 ms.
pub(crate) const DRIFT_THRESHOLD_FRAMES: u64 = 960;
/// A block this far behind the session clock is emitted with silence.
pub(crate) const LATENCY_LIMIT_FRAMES: u64 = 7_200;
/// Sudden holes and overlaps at least this large are timeline repairs.
/// One millisecond. A 5 ms gap is silence. Sub-millisecond slip stays in the
/// drift accumulator.
pub(crate) const HOLE_FRAMES: u64 = 48;
/// Largest silence insert for one hole. Matches the microphone writer's cap.
pub(crate) const MAX_GAP_FRAMES: u64 = 48_000 * 5;

/// `latency_hns` is a WASAPI stream latency in 100 ns units.
pub(crate) fn latency_reference_to_ticks(latency_hns: i64, ticks_per_second: u64) -> u64 {
    if latency_hns <= 0 || ticks_per_second == 0 {
        return 0;
    }
    let ticks =
        u128::from(latency_hns as u64).saturating_mul(u128::from(ticks_per_second)) / 10_000_000;
    u64::try_from(ticks).unwrap_or(u64::MAX)
}

/// Frame index of a packet after subtracting device latency.
/// A timestamp that latency pulls before `t0` clamps to frame 0 when the raw
/// timestamp is still inside the session.
pub(crate) fn map_packet_frame(
    origin: SessionOrigin,
    timestamp: u64,
    latency_ticks: u64,
) -> Option<u64> {
    let adjusted = timestamp.saturating_sub(latency_ticks);
    if let Some(frame) = origin.sample_index(adjusted) {
        return Some(frame);
    }
    origin.sample_index(timestamp).map(|_| 0)
}

pub(crate) fn block_is_late(block_start: u64, now_frame: u64) -> bool {
    now_frame.saturating_sub(block_start) > LATENCY_LIMIT_FRAMES
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PacketPlan {
    pub(crate) silence_frames: u64,
    pub(crate) skip_frames: u64,
    pub(crate) take_frames: u64,
    pub(crate) duplicate_frames: u64,
    pub(crate) gap: bool,
    pub(crate) drift_drop: u64,
    pub(crate) drift_duplicate: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AlignState {
    pub(crate) cursor: u64,
    /// Uncorrected drift in frames. Positive means the device clock is slow.
    pub(crate) last_error: i64,
}

impl AlignState {
    pub(crate) fn new() -> Self {
        Self {
            cursor: 0,
            last_error: 0,
        }
    }
}

impl Default for AlignState {
    fn default() -> Self {
        Self::new()
    }
}

/// Place one packet on the session timeline.
///
/// `start` is the QPC-derived frame of the first sample. `None` appends.
pub(crate) fn plan_packet(
    state: &mut AlignState,
    start: Option<u64>,
    frames: u64,
    discontinuity: bool,
) -> PacketPlan {
    if frames == 0 {
        return PacketPlan {
            silence_frames: 0,
            skip_frames: 0,
            take_frames: 0,
            duplicate_frames: 0,
            gap: false,
            drift_drop: 0,
            drift_duplicate: 0,
        };
    }
    let Some(start) = start else {
        state.cursor = state.cursor.saturating_add(frames);
        return PacketPlan {
            silence_frames: 0,
            skip_frames: 0,
            take_frames: frames,
            duplicate_frames: 0,
            gap: false,
            drift_drop: 0,
            drift_duplicate: 0,
        };
    };

    let error = start as i64 - state.cursor as i64;
    let jump = error.saturating_sub(state.last_error);
    let hole = i64::try_from(HOLE_FRAMES).unwrap_or(i64::MAX);
    let mut silence = 0_u64;
    let mut skip = 0_u64;
    let mut sudden_overlap = false;

    if (discontinuity && jump > 0) || (jump >= hole && error > 0) {
        silence = u64::try_from(jump).unwrap_or(u64::MAX).min(MAX_GAP_FRAMES);
    } else if jump <= -hole && error < 0 {
        let late = u64::try_from(-jump).unwrap_or(u64::MAX);
        skip = late.min(frames);
        sudden_overlap = true;
    }

    let residual = if silence > 0 {
        error.saturating_sub(i64::try_from(silence).unwrap_or(i64::MAX))
    } else if sudden_overlap {
        state.last_error
    } else {
        error
    };

    let threshold = i64::try_from(DRIFT_THRESHOLD_FRAMES).unwrap_or(i64::MAX);
    let mut duplicate = 0_u64;
    let mut drift_drop = 0_u64;
    let mut drift_duplicate = 0_u64;
    if residual > threshold {
        let extra = u64::try_from(residual - threshold).unwrap_or(u64::MAX);
        duplicate = extra;
        drift_duplicate = extra;
        state.last_error = threshold;
    } else if residual < -threshold {
        let extra = u64::try_from((-residual) - threshold).unwrap_or(u64::MAX);
        let room = frames.saturating_sub(skip);
        let drop = extra.min(room);
        skip = skip.saturating_add(drop);
        drift_drop = drop;
        let applied = i64::try_from(drop).unwrap_or(i64::MAX);
        state.last_error = residual.saturating_add(applied);
    } else {
        state.last_error = residual;
    }

    let take = frames.saturating_sub(skip);
    state.cursor = state
        .cursor
        .saturating_add(silence)
        .saturating_add(take)
        .saturating_add(duplicate);
    PacketPlan {
        silence_frames: silence,
        skip_frames: skip,
        take_frames: take,
        duplicate_frames: duplicate,
        gap: silence > 0,
        drift_drop,
        drift_duplicate,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        block_is_late, latency_reference_to_ticks, map_packet_frame, plan_packet, AlignState,
        DRIFT_THRESHOLD_FRAMES, HOLE_FRAMES, LATENCY_LIMIT_FRAMES, MIX_BLOCK_FRAMES,
    };
    use crate::domain::audio::SessionOrigin;

    #[test]
    fn sample_index_matches_the_closed_form_across_an_hour() {
        // Arrange
        let origin = SessionOrigin::new(0, 10_000_000, 48_000).expect("origin");

        // Act
        let mut previous = 0_u64;
        for second in 0..3_600_u64 {
            let ticks = second * 10_000_000;
            let frame = origin.sample_index(ticks).expect("frame");
            let closed = second * 48_000;

            // Assert
            assert_eq!(frame, closed);
            assert!(frame >= previous);
            previous = frame;
        }
        assert_eq!(MIX_BLOCK_FRAMES, 480);
    }

    #[test]
    fn latency_shifts_the_packet_without_float_division() {
        // Arrange
        let origin = SessionOrigin::new(1_000, 10_000_000, 48_000).expect("origin");
        let ten_ms = latency_reference_to_ticks(100_000, 10_000_000);

        // Act
        let raw = map_packet_frame(origin, 1_000 + 10_000_000, 0).expect("raw");
        let late = map_packet_frame(origin, 1_000 + ten_ms, ten_ms);
        let clamped = map_packet_frame(origin, 1_000 + ten_ms / 2, ten_ms);

        // Assert
        assert_eq!(ten_ms, 100_000);
        assert_eq!(raw, 48_000);
        assert_eq!(late, Some(0));
        assert_eq!(clamped, Some(0));
        assert_eq!(latency_reference_to_ticks(-1, 10_000_000), 0);
    }

    #[test]
    fn a_five_millisecond_hole_becomes_silence_and_overlap_is_dropped() {
        // Arrange
        let mut aligned = AlignState::new();
        let mut late = AlignState::new();

        // Act
        let first = plan_packet(&mut aligned, Some(0), 480, false);
        let gap = plan_packet(&mut aligned, Some(480 + 240), 480, false);
        let _ = plan_packet(&mut late, Some(0), 480, false);
        let overlap = plan_packet(&mut late, Some(280), 480, false);

        // Assert
        assert_eq!(first.take_frames, 480);
        assert!(gap.gap);
        assert_eq!(gap.silence_frames, 240);
        assert_eq!(gap.drift_duplicate, 0);
        assert_eq!(overlap.skip_frames, 200);
        assert_eq!(overlap.drift_drop, 0);
        assert_eq!(overlap.take_frames, 280);
        let _ = HOLE_FRAMES;
    }

    #[test]
    fn discontinuity_opens_a_short_gap_and_lateness_is_150ms() {
        // Arrange
        let mut state = AlignState::new();

        // Act
        let _ = plan_packet(&mut state, Some(0), 480, false);
        let marked = plan_packet(&mut state, Some(490), 480, true);

        // Assert
        assert!(marked.gap);
        assert_eq!(marked.silence_frames, 10);
        assert!(!block_is_late(0, LATENCY_LIMIT_FRAMES));
        assert!(block_is_late(0, LATENCY_LIMIT_FRAMES + 1));
    }

    #[test]
    fn fast_and_slow_clocks_correct_only_past_20ms() {
        // Arrange
        let frames = 4_800_u64;
        let packets = 2_200_u64;
        let mut fast = AlignState::new();
        let mut slow = AlignState::new();
        let mut fast_drops = 0_u64;
        let mut slow_dups = 0_u64;

        // Act
        for index in 0..packets {
            let fast_start = index * frames * 10_000 / 10_001;
            let slow_start = index * frames * 10_001 / 10_000;
            let fast_plan = plan_packet(&mut fast, Some(fast_start), frames, false);
            let slow_plan = plan_packet(&mut slow, Some(slow_start), frames, false);
            fast_drops = fast_drops.saturating_add(fast_plan.drift_drop);
            slow_dups = slow_dups.saturating_add(slow_plan.drift_duplicate);
            assert!(
                fast_plan.drift_drop <= 8,
                "fast correction should stay sparse, dropped {}",
                fast_plan.drift_drop
            );
            assert!(
                slow_plan.drift_duplicate <= 8,
                "slow correction should stay sparse, duplicated {}",
                slow_plan.drift_duplicate
            );
        }

        // Assert
        let produced = packets * frames;
        assert!(fast_drops > 0);
        assert!(slow_dups > 0);
        let fast_qpc = produced * 10_000 / 10_001;
        let fast_error = fast.cursor as i64 - fast_qpc as i64;
        assert!(fast_error.abs() <= DRIFT_THRESHOLD_FRAMES as i64 + frames as i64);
        let slow_qpc = produced * 10_001 / 10_000;
        let slow_error = slow.cursor as i64 - slow_qpc as i64;
        assert!(slow_error.abs() <= DRIFT_THRESHOLD_FRAMES as i64 + frames as i64);
        assert!(fast.last_error.abs() <= DRIFT_THRESHOLD_FRAMES as i64 + 8);
        assert!(slow.last_error.abs() <= DRIFT_THRESHOLD_FRAMES as i64 + 8);
    }
}
