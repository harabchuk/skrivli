//! Cut planning: where to split a finished recording into pieces for offline
//! transcription.
//!
//! The planner is pure and incremental. It reads [`VoiceFrame`]s in time order
//! and reports cut positions in milliseconds. It keeps only the frames since
//! the current piece started, so memory stays bounded on long recordings.
//!
//! Rules, relative to the start of the current piece:
//! - Pauses before [`MIN_PIECE_MS`] are ignored.
//! - From [`PREFERRED_PIECE_MS`], the middle of the first pause of at least
//!   [`MIN_PAUSE_MS`] becomes a cut, once that pause has ended.
//! - If [`MAX_PIECE_MS`] is reached first, the cut goes in the middle of the
//!   longest pause after [`MIN_PIECE_MS`]. With no pause at all, it goes at the
//!   frame with the lowest speech score.

/// Pauses that start before this are ignored.
pub(crate) const MIN_PIECE_MS: u64 = 8_000;
/// From here, a pause of at least [`MIN_PAUSE_MS`] is a cut.
pub(crate) const PREFERRED_PIECE_MS: u64 = 20_000;
/// A piece never grows past this.
pub(crate) const MAX_PIECE_MS: u64 = 30_000;
/// Shortest pause that counts as a natural cut.
pub(crate) const MIN_PAUSE_MS: u64 = 300;

/// One analysed slice of 16 kHz mono audio. Produced by a pause detector.
///
/// `speech_score` runs from 0 (surely silence) to 1 (surely speech). Each
/// detector owns its thresholds and says through `silent` what counts as a
/// pause; the planner only compares scores when no pause exists.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct VoiceFrame {
    start_ms: u64,
    end_ms: u64,
    silent: bool,
    speech_score: f32,
}

impl VoiceFrame {
    /// `end_ms` is raised to `start_ms` if it is earlier. The score is clamped
    /// to 0..=1, and NaN becomes 0.
    pub(crate) fn new(start_ms: u64, end_ms: u64, silent: bool, speech_score: f32) -> Self {
        let speech_score = if speech_score.is_nan() {
            0.0
        } else {
            speech_score.clamp(0.0, 1.0)
        };
        Self {
            start_ms,
            end_ms: end_ms.max(start_ms),
            silent,
            speech_score,
        }
    }

    pub(crate) fn start_ms(&self) -> u64 {
        self.start_ms
    }

    pub(crate) fn end_ms(&self) -> u64 {
        self.end_ms
    }

    pub(crate) fn silent(&self) -> bool {
        self.silent
    }

    pub(crate) fn speech_score(&self) -> f32 {
        self.speech_score
    }
}

/// Incremental cut planner. See the module notes for the rules.
#[derive(Debug, Default)]
pub(crate) struct CutPlanner {
    piece_start_ms: u64,
    /// Frames since `piece_start_ms`, in time order.
    frames: Vec<VoiceFrame>,
    last_end_ms: u64,
}

impl CutPlanner {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Start of the piece being built. Only tests read it; the transcription service
    /// tracks its own sample offset.
    #[cfg(test)]
    pub(crate) fn piece_start_ms(&self) -> u64 {
        self.piece_start_ms
    }

    /// Feeds the next frame and returns the cuts it settles, in increasing order.
    /// Usually none, sometimes one, rarely more after a long stretch without a
    /// natural pause.
    pub(crate) fn push(&mut self, frame: VoiceFrame) -> Vec<u64> {
        self.last_end_ms = self.last_end_ms.max(frame.end_ms());
        self.frames.push(frame);
        let mut cuts = Vec::new();
        // A pause has ended when the newest frame is speech.
        let mut check_ended_pause = !frame.silent();
        while let Some(cut) = self.next_cut(check_ended_pause) {
            cuts.push(cut);
            self.cut_at(cut);
            check_ended_pause = self.frames.last().is_some_and(|f| !f.silent());
        }
        cuts
    }

    /// Ends the last piece. Returns its end in ms, or `None` when no audio
    /// follows the last cut. A recording shorter than [`MIN_PIECE_MS`] gives
    /// just this one piece.
    pub(crate) fn finish(&mut self) -> Option<u64> {
        let end = self.last_end_ms;
        self.frames.clear();
        if end > self.piece_start_ms {
            self.piece_start_ms = end;
            Some(end)
        } else {
            None
        }
    }

    fn cut_at(&mut self, cut: u64) {
        self.piece_start_ms = cut;
        self.frames.retain(|f| f.end_ms() > cut);
    }

    fn next_cut(&self, check_ended_pause: bool) -> Option<u64> {
        let last = self.frames.last()?;
        if check_ended_pause {
            let preferred_from = self.piece_start_ms + PREFERRED_PIECE_MS;
            if let Some(run) = self
                .pauses(preferred_from, last.start_ms())
                .into_iter()
                .find(|&(start, end)| end - start >= MIN_PAUSE_MS)
            {
                return Some(middle(run));
            }
        }
        let limit = self.piece_start_ms + MAX_PIECE_MS;
        if last.end_ms() < limit {
            return None;
        }
        Some(self.forced_cut(limit))
    }

    /// Runs of silent frames that start at or after `from` and end by `until`.
    fn pauses(&self, from: u64, until: u64) -> Vec<(u64, u64)> {
        let mut runs: Vec<(u64, u64)> = Vec::new();
        let mut open: Option<(u64, u64)> = None;
        for frame in self.frames.iter().filter(|f| f.start_ms() >= from) {
            if frame.end_ms() > until {
                break;
            }
            if frame.silent() {
                open = Some(match open {
                    Some((start, _)) => (start, frame.end_ms()),
                    None => (frame.start_ms(), frame.end_ms()),
                });
            } else if let Some(run) = open.take() {
                runs.push(run);
            }
        }
        runs.extend(open);
        runs
    }

    fn forced_cut(&self, limit: u64) -> u64 {
        let min_from = self.piece_start_ms + MIN_PIECE_MS;
        // Longest pause (any length); on a tie the earlier one wins.
        let longest = self.pauses(min_from, limit).into_iter().fold(
            None::<(u64, u64)>,
            |best, run| match best {
                Some(b) if b.1 - b.0 >= run.1 - run.0 => Some(b),
                _ => Some(run),
            },
        );
        if let Some(run) = longest {
            return middle(run);
        }
        // No silence: the quietest frame, the earlier one on a tie.
        let quietest = self
            .frames
            .iter()
            .filter(|f| f.start_ms() >= min_from && f.end_ms() <= limit)
            .fold(None::<&VoiceFrame>, |best, f| match best {
                Some(b) if b.speech_score() <= f.speech_score() => Some(b),
                _ => Some(f),
            });
        quietest.map_or(limit, |f| middle((f.start_ms(), f.end_ms())))
    }
}

fn middle((start, end): (u64, u64)) -> u64 {
    start + (end - start) / 2
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME_MS: u64 = 20;

    /// Frames of 20 ms from `from_ms` to `to_ms`.
    fn run(from_ms: u64, to_ms: u64, silent: bool, score: f32) -> Vec<VoiceFrame> {
        (from_ms..to_ms)
            .step_by(FRAME_MS as usize)
            .map(|t| VoiceFrame::new(t, t + FRAME_MS, silent, score))
            .collect()
    }

    fn speech(from_ms: u64, to_ms: u64) -> Vec<VoiceFrame> {
        run(from_ms, to_ms, false, 0.9)
    }

    fn pause(from_ms: u64, to_ms: u64) -> Vec<VoiceFrame> {
        run(from_ms, to_ms, true, 0.0)
    }

    fn plan(frames: impl IntoIterator<Item = VoiceFrame>) -> (Vec<u64>, Option<u64>) {
        let mut planner = CutPlanner::new();
        let mut cuts = Vec::new();
        for frame in frames {
            cuts.extend(planner.push(frame));
        }
        (cuts, planner.finish())
    }

    #[test]
    fn cuts_in_the_middle_of_the_first_long_pause_after_twenty_seconds() {
        // Arrange
        let frames = [
            speech(0, 10_000),
            pause(10_000, 10_600), // long, but before PREFERRED_PIECE
            speech(10_600, 21_000),
            pause(21_000, 21_100), // too short
            speech(21_100, 22_000),
            pause(22_000, 22_400),
            speech(22_400, 25_000),
            pause(25_000, 25_600),
            speech(25_600, 26_000),
        ]
        .concat();

        // Act
        let (cuts, end) = plan(frames);

        // Assert
        assert_eq!(cuts, vec![22_200]); // the pause at 25 s is under 20 s into the next piece
        assert_eq!(end, Some(26_000));
    }

    #[test]
    fn ignores_a_pause_before_the_minimum_piece_length() {
        // Arrange
        let frames = [
            speech(0, 6_000),
            pause(6_000, 7_000),
            speech(7_000, 15_000),
            pause(15_000, 15_040),
            speech(15_040, 40_000),
        ]
        .concat();

        // Act
        let (cuts, _) = plan(frames);

        // Assert
        assert_eq!(cuts[0], 15_020);
    }

    #[test]
    fn forced_cut_at_the_limit_picks_the_longest_pause() {
        // Arrange
        let frames = [
            speech(0, 10_000),
            pause(10_000, 10_100),
            speech(10_100, 15_000),
            pause(15_000, 15_200),
            speech(15_200, 25_000),
            pause(25_000, 25_100),
            speech(25_100, 31_000),
        ]
        .concat();

        // Act
        let (cuts, end) = plan(frames);

        // Assert
        assert_eq!(cuts, vec![15_100]);
        assert_eq!(end, Some(31_000));
    }

    #[test]
    fn without_any_silence_the_quietest_frame_wins() {
        // Arrange
        let mut frames = speech(0, 35_000);
        frames[850] = VoiceFrame::new(17_000, 17_020, false, 0.2);
        frames[300] = VoiceFrame::new(6_000, 6_020, false, 0.0); // before MIN_PIECE

        // Act
        let (cuts, _) = plan(frames);

        // Assert
        assert_eq!(cuts, vec![17_010]);
    }

    #[test]
    fn a_short_recording_is_one_piece() {
        // Arrange
        let frames = [speech(0, 3_000), pause(3_000, 4_000), speech(4_000, 5_000)].concat();

        // Act
        let (cuts, end) = plan(frames);

        // Assert
        assert!(cuts.is_empty());
        assert_eq!(end, Some(5_000));
    }

    #[test]
    fn finish_without_audio_has_no_piece() {
        // Arrange
        let mut planner = CutPlanner::new();

        // Act
        let end = planner.finish();

        // Assert
        assert_eq!(end, None);
    }

    #[test]
    fn a_pause_still_open_at_the_end_is_not_a_cut() {
        // Arrange
        let frames = [speech(0, 21_000), pause(21_000, 22_000)].concat();

        // Act
        let (cuts, end) = plan(frames);

        // Assert
        assert!(cuts.is_empty());
        assert_eq!(end, Some(22_000));
    }

    #[test]
    fn keeps_only_the_frames_of_the_open_piece() {
        // Arrange
        let mut planner = CutPlanner::new();
        let frames = [
            speech(0, 21_000),
            pause(21_000, 21_400),
            speech(21_400, 22_000),
        ]
        .concat();

        // Act
        for frame in frames {
            planner.push(frame);
        }

        // Assert
        assert_eq!(planner.piece_start_ms(), 21_200);
        assert!(planner.frames.len() <= ((22_000 - 21_200) / FRAME_MS) as usize + 1);
    }

    #[test]
    fn cuts_increase_and_pieces_stay_within_limits() {
        // Arrange: pseudo-random speech and pauses over ten minutes.
        let mut state = 0x2545_f491_u64;
        let mut next = move || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            state >> 33
        };
        let mut frames = Vec::new();
        let mut t = 0;
        while t < 600_000 {
            let length = 20 * (1 + next() % 150);
            let silent = next() % 3 == 0;
            let score = (next() % 100) as f32 / 100.0;
            frames.extend(run(t, t + length, silent, if silent { 0.0 } else { score }));
            t += length;
        }
        let total = t;

        // Act
        let (cuts, end) = plan(frames);

        // Assert
        let mut previous = 0;
        for &cut in &cuts {
            assert!(cut > previous);
            assert!(cut - previous >= MIN_PIECE_MS, "piece {previous}..{cut}");
            assert!(cut - previous <= MAX_PIECE_MS, "piece {previous}..{cut}");
            previous = cut;
        }
        assert_eq!(end, Some(total));
        assert!(total - previous <= MAX_PIECE_MS);
    }

    #[test]
    fn voice_frame_clamps_its_score_and_orders_its_times() {
        // Arrange
        let loud = VoiceFrame::new(100, 50, false, 3.0);
        let nan = VoiceFrame::new(0, 20, true, f32::NAN);

        // Act
        let (start, end, score, nan_score) = (
            loud.start_ms(),
            loud.end_ms(),
            loud.speech_score(),
            nan.speech_score(),
        );

        // Assert
        assert_eq!((start, end), (100, 100));
        assert_eq!(score, 1.0);
        assert!(!loud.silent());
        assert_eq!(nan_score, 0.0);
    }
}
