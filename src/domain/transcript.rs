//! Transcript segments and deterministic plain-text formatting.
//!
//! No file I/O, GPUI, Windows, or AI/engine types live here. Infrastructure
//! resamples audio and writes the formatted UTF-8 bytes to `<basename>.txt`.

use std::time::Duration;

/// Pause between segments that starts a new paragraph (~1.5 s).
pub(crate) const PARAGRAPH_PAUSE: Duration = Duration::from_millis(1_500);
/// Maximum wall span covered by one paragraph before a forced break (~60 s).
pub(crate) const PARAGRAPH_MAX_SPAN: Duration = Duration::from_secs(60);
/// Keep this many identical consecutive segments; drop further runaway repeats.
const REPEAT_KEEP: usize = 2;

/// One timed transcript segment from the recording start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TranscriptSegment {
    start_ms: u64,
    end_ms: u64,
    text: String,
}

impl TranscriptSegment {
    pub(crate) fn new(start_ms: u64, end_ms: u64, text: impl Into<String>) -> Self {
        let end_ms = end_ms.max(start_ms);
        Self {
            start_ms,
            end_ms,
            text: text.into(),
        }
    }

    pub(crate) fn start_ms(&self) -> u64 {
        self.start_ms
    }

    pub(crate) fn end_ms(&self) -> u64 {
        self.end_ms
    }

    pub(crate) fn text(&self) -> &str {
        &self.text
    }
}

/// Format elapsed wall time as `[HH:MM:SS]` (always includes hours).
pub(crate) fn format_timestamp(elapsed: Duration) -> String {
    let total = elapsed.as_secs();
    let hours = total / 3_600;
    let minutes = (total % 3_600) / 60;
    let seconds = total % 60;
    format!("[{hours:02}:{minutes:02}:{seconds:02}]")
}

/// Format milliseconds as `[HH:MM:SS]`.
pub(crate) fn format_timestamp_ms(elapsed_ms: u64) -> String {
    format_timestamp(Duration::from_millis(elapsed_ms))
}

/// Deterministic UTF-8 transcript body with timestamps and paragraph breaks.
pub(crate) fn format_transcript(segments: &[TranscriptSegment]) -> String {
    let cleaned = cleanup_repeated_lines(segments);
    if cleaned.is_empty() {
        return String::new();
    }

    let mut body = String::new();
    let mut paragraph_start_ms = cleaned[0].start_ms();
    let mut previous_end_ms = cleaned[0].start_ms();
    let mut first_in_paragraph = true;

    for segment in &cleaned {
        let text = segment.text().trim();
        if text.is_empty() {
            continue;
        }

        let pause = segment.start_ms().saturating_sub(previous_end_ms);
        let span = segment.start_ms().saturating_sub(paragraph_start_ms);
        let new_paragraph = !first_in_paragraph
            && (pause >= PARAGRAPH_PAUSE.as_millis() as u64
                || span >= PARAGRAPH_MAX_SPAN.as_millis() as u64);

        if new_paragraph {
            body.push('\n');
            body.push('\n');
            paragraph_start_ms = segment.start_ms();
            first_in_paragraph = true;
        } else if !first_in_paragraph {
            body.push(' ');
        }

        if first_in_paragraph {
            body.push_str(&format_timestamp_ms(segment.start_ms()));
            body.push(' ');
            first_in_paragraph = false;
        }

        body.push_str(text);
        previous_end_ms = segment.end_ms();
    }

    if !body.is_empty() && !body.ends_with('\n') {
        body.push('\n');
    }
    body
}

/// Drop runaway identical lines while keeping a short genuine repeat.
fn cleanup_repeated_lines(segments: &[TranscriptSegment]) -> Vec<TranscriptSegment> {
    let mut out = Vec::with_capacity(segments.len());
    let mut last_norm: Option<String> = None;
    let mut streak = 0_usize;

    for segment in segments {
        let trimmed = segment.text().trim();
        if trimmed.is_empty() {
            continue;
        }
        let norm = normalize_line(trimmed);
        if last_norm.as_ref() == Some(&norm) {
            streak = streak.saturating_add(1);
            if streak > REPEAT_KEEP {
                continue;
            }
        } else {
            streak = 1;
            last_norm = Some(norm);
        }
        out.push(TranscriptSegment::new(
            segment.start_ms(),
            segment.end_ms(),
            trimmed.to_string(),
        ));
    }
    out
}

fn normalize_line(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// One paragraph of a transcript file, as shown in the transcript list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TranscriptParagraph {
    start_ms: Option<u64>,
    text: String,
}

impl TranscriptParagraph {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn start_ms(&self) -> Option<u64> {
        self.start_ms
    }

    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    /// `HH:MM:SS` without brackets, or `None` when the paragraph has no timestamp.
    pub(crate) fn timestamp_label(&self) -> Option<String> {
        let total = self.start_ms? / 1_000;
        Some(format!(
            "{:02}:{:02}:{:02}",
            total / 3_600,
            (total % 3_600) / 60,
            total % 60
        ))
    }
}

/// Split transcript file text into paragraphs. Never drops text.
///
/// A paragraph starts at a blank line or at a line beginning with `[HH:MM:SS]`,
/// which is what `format_transcript` writes. Other lines join the paragraph above.
pub(crate) fn parse_transcript(text: &str) -> Vec<TranscriptParagraph> {
    let mut paragraphs = Vec::new();
    let mut current: Option<TranscriptParagraph> = None;
    let flush = |current: &mut Option<TranscriptParagraph>, out: &mut Vec<TranscriptParagraph>| {
        if let Some(paragraph) = current.take() {
            if !paragraph.text.is_empty() {
                out.push(paragraph);
            }
        }
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            flush(&mut current, &mut paragraphs);
            continue;
        }
        if let Some((start_ms, rest)) = split_timestamp(line) {
            flush(&mut current, &mut paragraphs);
            current = Some(TranscriptParagraph {
                start_ms: Some(start_ms),
                text: rest.to_string(),
            });
            continue;
        }
        match current.as_mut() {
            Some(paragraph) if paragraph.text.is_empty() => paragraph.text.push_str(line),
            Some(paragraph) => {
                paragraph.text.push('\n');
                paragraph.text.push_str(line);
            }
            None => {
                current = Some(TranscriptParagraph {
                    start_ms: None,
                    text: line.to_string(),
                });
            }
        }
    }
    flush(&mut current, &mut paragraphs);
    paragraphs
}

/// Split a leading `[HH:MM:SS]` off a line. Returns the start in ms and the rest.
fn split_timestamp(line: &str) -> Option<(u64, &str)> {
    let inner = line.strip_prefix('[')?;
    let (stamp, rest) = inner.split_once(']')?;
    let mut parts = stamp.split(':');
    let hours = parse_digits(parts.next()?)?;
    let minutes = parse_digits(parts.next()?)?;
    let seconds = parse_digits(parts.next()?)?;
    if parts.next().is_some() || minutes > 59 || seconds > 59 {
        return None;
    }
    let total = hours
        .checked_mul(3_600)?
        .checked_add(minutes * 60 + seconds)?
        .checked_mul(1_000)?;
    Some((total, rest.trim()))
}

fn parse_digits(part: &str) -> Option<u64> {
    if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    part.parse().ok()
}

/// Index of the paragraph being spoken at `position_ms`: the last one that starts at or
/// before it. Paragraphs with no timestamp are skipped, and `None` means the position is
/// before the first timestamped paragraph.
pub(crate) fn current_paragraph(
    paragraphs: &[TranscriptParagraph],
    position_ms: u64,
) -> Option<usize> {
    paragraphs
        .iter()
        .enumerate()
        .filter(|(_, paragraph)| paragraph.start_ms.is_some_and(|start| start <= position_ms))
        .map(|(index, _)| index)
        .next_back()
}

#[cfg(test)]
mod tests {
    use super::{
        cleanup_repeated_lines, current_paragraph, format_timestamp, format_transcript,
        parse_transcript, TranscriptSegment, PARAGRAPH_MAX_SPAN, PARAGRAPH_PAUSE,
    };
    use std::time::Duration;

    #[test]
    fn formats_timestamp_past_one_hour() {
        // Arrange
        let elapsed = Duration::from_secs(3_661);

        // Act
        let text = format_timestamp(elapsed);

        // Assert
        assert_eq!(text, "[01:01:01]");
    }

    #[test]
    fn keeps_genuine_repeat_but_drops_runaway_lines() {
        // Arrange
        let segments = vec![
            TranscriptSegment::new(0, 500, "yes"),
            TranscriptSegment::new(600, 1_100, "yes"),
            TranscriptSegment::new(1_200, 1_700, "yes"),
            TranscriptSegment::new(1_800, 2_300, "yes"),
            TranscriptSegment::new(2_400, 3_000, "okay"),
        ];

        // Act
        let cleaned = cleanup_repeated_lines(&segments);

        // Assert
        assert_eq!(cleaned.len(), 3);
        assert_eq!(cleaned[0].text(), "yes");
        assert_eq!(cleaned[1].text(), "yes");
        assert_eq!(cleaned[2].text(), "okay");
    }

    #[test]
    fn formats_empty_silent_and_unicode() {
        // Arrange
        let empty: Vec<TranscriptSegment> = Vec::new();
        let silent = vec![TranscriptSegment::new(0, 100, "   ")];
        let unicode = vec![TranscriptSegment::new(0, 500, "café 日本語")];

        // Act
        let empty_text = format_transcript(&empty);
        let silent_text = format_transcript(&silent);
        let unicode_text = format_transcript(&unicode);

        // Assert
        assert_eq!(empty_text, "");
        assert_eq!(silent_text, "");
        assert!(unicode_text.starts_with("[00:00:00] café 日本語"));
        assert!(unicode_text.ends_with('\n'));
    }

    #[test]
    fn paragraph_break_on_pause_or_span() {
        // Arrange
        let pause_ms = PARAGRAPH_PAUSE.as_millis() as u64;
        let span_ms = PARAGRAPH_MAX_SPAN.as_millis() as u64;
        let with_pause = vec![
            TranscriptSegment::new(0, 500, "one"),
            TranscriptSegment::new(500 + pause_ms, 600 + pause_ms, "two"),
        ];
        let with_span = vec![
            TranscriptSegment::new(0, 500, "alpha"),
            TranscriptSegment::new(span_ms, span_ms + 500, "beta"),
        ];

        // Act
        let pause_text = format_transcript(&with_pause);
        let span_text = format_transcript(&with_span);

        // Assert
        assert!(pause_text.contains("\n\n["));
        assert!(span_text.contains("\n\n["));
        assert!(pause_text.contains("[00:00:00] one"));
        assert!(span_text.contains("alpha"));
    }

    fn para(start_ms: Option<u64>, text: &str) -> super::TranscriptParagraph {
        super::TranscriptParagraph {
            start_ms,
            text: text.to_string(),
        }
    }

    #[test]
    fn parses_two_paragraphs_from_formatted_output() {
        // Arrange
        let segments = vec![
            TranscriptSegment::new(0, 1_000, "Hello there."),
            TranscriptSegment::new(10_000, 11_000, "Second paragraph."),
        ];
        let text = format_transcript(&segments);

        // Act
        let paragraphs = parse_transcript(&text);

        // Assert
        assert_eq!(
            paragraphs,
            vec![
                para(Some(0), "Hello there."),
                para(Some(10_000), "Second paragraph.")
            ]
        );
    }

    #[test]
    fn parses_timestamp_past_one_hour_and_labels_it() {
        // Arrange
        let text = "[01:01:01] Late remark.
";

        // Act
        let paragraphs = parse_transcript(text);

        // Assert
        assert_eq!(paragraphs, vec![para(Some(3_661_000), "Late remark.")]);
        assert_eq!(paragraphs[0].timestamp_label().as_deref(), Some("01:01:01"));
        assert_eq!(paragraphs[0].start_ms(), Some(3_661_000));
        assert_eq!(para(None, "x").timestamp_label(), None);
    }

    #[test]
    fn keeps_unicode_text() {
        // Arrange
        let text = "[00:00:05] Grüße, 世界 👋 — ok
";

        // Act
        let paragraphs = parse_transcript(text);

        // Assert
        assert_eq!(paragraphs, vec![para(Some(5_000), "Grüße, 世界 👋 — ok")]);
    }

    #[test]
    fn empty_or_blank_text_gives_no_paragraphs() {
        // Arrange
        let inputs = [
            "", "

  
",
        ];

        // Act
        let results = inputs.map(parse_transcript);

        // Assert
        assert!(results.iter().all(Vec::is_empty));
    }

    #[test]
    fn text_without_timestamp_is_kept_with_no_start() {
        // Arrange
        let text = "Typed by hand.

[00:00:09] Back to normal.
";

        // Act
        let paragraphs = parse_transcript(text);

        // Assert
        assert_eq!(
            paragraphs,
            vec![
                para(None, "Typed by hand."),
                para(Some(9_000), "Back to normal.")
            ]
        );
    }

    #[test]
    fn several_lines_under_one_timestamp_stay_in_one_paragraph() {
        // Arrange
        let text = "[00:00:01] First line.
Second line.
[00:00:02] Next.
";

        // Act
        let paragraphs = parse_transcript(text);

        // Assert
        assert_eq!(
            paragraphs,
            vec![
                para(
                    Some(1_000),
                    "First line.
Second line."
                ),
                para(Some(2_000), "Next.")
            ]
        );
    }

    #[test]
    fn round_trip_keeps_every_word() {
        // Arrange
        let segments = vec![
            TranscriptSegment::new(0, 1_000, "alpha beta"),
            TranscriptSegment::new(1_200, 2_000, "gamma"),
            TranscriptSegment::new(9_000, 10_000, "delta épsilon"),
            TranscriptSegment::new(70_000, 71_000, "zeta"),
        ];
        let text = format_transcript(&segments);

        // Act
        let parsed: Vec<String> = parse_transcript(&text)
            .iter()
            .flat_map(|p| {
                p.text()
                    .split_whitespace()
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .collect();

        // Assert
        assert_eq!(
            parsed,
            ["alpha", "beta", "gamma", "delta", "épsilon", "zeta"]
        );
    }

    const TIMED: &str = "[00:00:00] one

[00:00:10] two

[00:00:20] three
";

    #[test]
    fn current_paragraph_is_none_before_the_first_timestamp() {
        // Arrange
        let paragraphs = parse_transcript(
            "[00:00:05] one
",
        );

        // Act
        let index = current_paragraph(&paragraphs, 4_999);

        // Assert
        assert_eq!(index, None);
    }

    #[test]
    fn current_paragraph_picks_the_last_one_that_has_started() {
        // Arrange
        let paragraphs = parse_transcript(TIMED);

        // Act
        let at_start = current_paragraph(&paragraphs, 0);
        let between = current_paragraph(&paragraphs, 15_000);
        let on_boundary = current_paragraph(&paragraphs, 10_000);

        // Assert
        assert_eq!(at_start, Some(0));
        assert_eq!(between, Some(1));
        assert_eq!(on_boundary, Some(1));
    }

    #[test]
    fn current_paragraph_stays_on_the_last_one_after_the_end() {
        // Arrange
        let paragraphs = parse_transcript(TIMED);

        // Act
        let index = current_paragraph(&paragraphs, 3_600_000);

        // Assert
        assert_eq!(index, Some(2));
    }

    #[test]
    fn current_paragraph_skips_paragraphs_without_a_timestamp() {
        // Arrange
        let paragraphs = parse_transcript(
            "[00:00:00] one

note without time
",
        );

        // Act
        let index = current_paragraph(&paragraphs, 30_000);

        // Assert
        assert_eq!(paragraphs.len(), 2);
        assert_eq!(index, Some(0));
    }

    #[test]
    fn current_paragraph_is_none_for_an_empty_list() {
        // Arrange
        let paragraphs = parse_transcript("");

        // Act
        let index = current_paragraph(&paragraphs, 1_000);

        // Assert
        assert_eq!(index, None);
    }
}
