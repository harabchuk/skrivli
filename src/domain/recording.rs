//! Recording identity, container format, and library metadata.
//!
//! No paths, GPUI, Windows, or file I/O live here. Infrastructure attaches
//! filesystem locations when it scans a folder.

use crate::domain::audio::CivilTime;
use crate::domain::ids::RecordingId;

/// Lifecycle phase shared by the recording service and the shell snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RecordPhase {
    Idle,
    Starting,
    Recording,
    Stopping,
    Error,
}

/// Longest recording Skrivli writes. Reaching it finalizes the file.
pub(crate) const MAX_RECORDING_SECONDS: u64 = 12 * 60 * 60;

/// User-chosen maximum meeting duration. Stops a forgotten recording before
/// it writes a very long file. Only the hours in [`Self::CHOICES_HOURS`] are valid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MaxMeetingDuration {
    hours: u32,
}

impl MaxMeetingDuration {
    pub(crate) const CHOICES_HOURS: [u32; 7] = [1, 2, 3, 4, 6, 8, 12];
    pub(crate) const DEFAULT_HOURS: u32 = 3;

    pub(crate) fn from_hours(hours: u32) -> Option<Self> {
        Self::CHOICES_HOURS
            .contains(&hours)
            .then_some(Self { hours })
    }

    pub(crate) fn hours(self) -> u32 {
        self.hours
    }

    pub(crate) fn frames(self, sample_rate: u32) -> u64 {
        u64::from(self.hours) * 3_600 * u64::from(sample_rate)
    }
}

impl Default for MaxMeetingDuration {
    fn default() -> Self {
        Self {
            hours: Self::DEFAULT_HOURS,
        }
    }
}

/// On-disk audio container the library can list. FLAC is the only one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RecordingContainer {
    Flac,
}

impl RecordingContainer {
    pub(crate) fn extension(self) -> &'static str {
        match self {
            Self::Flac => "flac",
        }
    }

    pub(crate) fn from_extension(ext: &str) -> Option<Self> {
        match ext {
            "flac" => Some(Self::Flac),
            _ => None,
        }
    }
}

/// Durably recorded PCM description from a sidecar (not live capture format).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RecordingFormat {
    sample_rate: u32,
    channels: u16,
    pcm16: bool,
}

impl RecordingFormat {
    pub(crate) fn new(sample_rate: u32, channels: u16, pcm16: bool) -> Option<Self> {
        if sample_rate == 0 || channels == 0 || channels > 8 {
            return None;
        }
        Some(Self {
            sample_rate,
            channels,
            pcm16,
        })
    }

    #[cfg(test)]
    pub(crate) fn skrivli_default() -> Self {
        Self {
            sample_rate: 48_000,
            channels: 2,
            pcm16: true,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn sample_rate(self) -> u32 {
        self.sample_rate
    }

    #[allow(dead_code)]
    pub(crate) fn channels(self) -> u16 {
        self.channels
    }

    #[allow(dead_code)]
    pub(crate) fn pcm16(self) -> bool {
        self.pcm16
    }
}

/// Transcript availability shown in the library list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TranscriptStatus {
    None,
    Queued,
    Progress(u8),
    Ready,
    Failed,
}

/// Status after a job ends without a new transcript, or after a restart.
///
/// An existing transcript file always wins over a failed or cancelled run, because the
/// old text is only replaced when a new one is ready. A cancelled first run goes back to
/// "no transcript" instead of Failed.
pub(crate) fn settled_transcript_status(
    stored: TranscriptStatus,
    cancelled: bool,
    has_transcript_file: bool,
) -> TranscriptStatus {
    match (stored, cancelled, has_transcript_file) {
        (TranscriptStatus::Failed, _, true) => TranscriptStatus::Ready,
        (TranscriptStatus::Failed, true, false) => TranscriptStatus::None,
        (status, _, _) => status,
    }
}

/// Whether the JSON sidecar loaded cleanly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SidecarState {
    Present,
    Missing,
    Malformed,
}

/// Pure library row: identity plus display metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RecordingInfo {
    id: RecordingId,
    name: String,
    container: RecordingContainer,
    started_at_unix_ms: Option<u64>,
    duration_ms: Option<u64>,
    size_bytes: u64,
    format: Option<RecordingFormat>,
    transcript: TranscriptStatus,
    sidecar: SidecarState,
    problems: Vec<String>,
}

impl RecordingInfo {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        id: RecordingId,
        name: impl Into<String>,
        container: RecordingContainer,
        started_at_unix_ms: Option<u64>,
        duration_ms: Option<u64>,
        size_bytes: u64,
        format: Option<RecordingFormat>,
        transcript: TranscriptStatus,
        sidecar: SidecarState,
        problems: Vec<String>,
    ) -> Self {
        Self {
            id,
            name: name.into(),
            container,
            started_at_unix_ms,
            duration_ms,
            size_bytes,
            format,
            transcript,
            sidecar,
            problems,
        }
    }

    pub(crate) fn id(&self) -> RecordingId {
        self.id
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn container(&self) -> RecordingContainer {
        self.container
    }

    pub(crate) fn started_at_unix_ms(&self) -> Option<u64> {
        self.started_at_unix_ms
    }

    pub(crate) fn duration_ms(&self) -> Option<u64> {
        self.duration_ms
    }

    pub(crate) fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    #[allow(dead_code)]
    pub(crate) fn format(&self) -> Option<RecordingFormat> {
        self.format
    }

    pub(crate) fn transcript(&self) -> TranscriptStatus {
        self.transcript
    }

    pub(crate) fn sidecar(&self) -> SidecarState {
        self.sidecar
    }

    pub(crate) fn problems(&self) -> &[String] {
        &self.problems
    }

    /// Sort key: newest first. Prefer sidecar start time, then size as a weak tie-break.
    pub(crate) fn sort_key(&self) -> (u64, u64) {
        (self.started_at_unix_ms.unwrap_or(0), self.size_bytes)
    }
}

/// Format duration milliseconds as `HH:MM:SS` (or `MM:SS` under one hour).
pub(crate) fn format_duration_ms(duration_ms: u64) -> String {
    let total = duration_ms / 1_000;
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let seconds = total % 60;
    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

/// Which list group a recording belongs to, by local calendar day.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RecordingDay {
    Today,
    Yesterday,
    Earlier,
}

/// Group a recording by its local calendar day relative to `today`.
///
/// A recording dated after `today` (clock skew) counts as Today.
pub(crate) fn recording_day(recorded: CivilTime, today: CivilTime) -> RecordingDay {
    let recorded = (recorded.year, recorded.month, recorded.day);
    let today = (today.year, today.month, today.day);
    if recorded >= today {
        RecordingDay::Today
    } else if recorded == previous_day(today) {
        RecordingDay::Yesterday
    } else {
        RecordingDay::Earlier
    }
}

fn previous_day((year, month, day): (i32, u8, u8)) -> (i32, u8, u8) {
    if day > 1 {
        (year, month, day - 1)
    } else if month > 1 {
        (year, month - 1, days_in_month(year, month - 1))
    } else {
        (year - 1, 12, 31)
    }
}

fn days_in_month(year: i32, month: u8) -> u8 {
    match month {
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Stable synthetic id when a sidecar is missing or unreadable.
///
/// Based on the display stem only so an in-app rename can recompute the same
/// id after the basename changes only if the stem is passed through; external
/// renames of unpaired files may drop selection (see decisions.md).
pub(crate) fn synthetic_recording_id(stem: &str) -> RecordingId {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in stem.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    if hash == 0 {
        hash = 1;
    }
    RecordingId::from_raw(hash)
}

/// Keep selection when a catalog is replaced. Prefers the previous id when still
/// present; otherwise `None`.
pub(crate) fn preserve_selection(
    previous: Option<RecordingId>,
    catalog: &[RecordingInfo],
) -> Option<RecordingId> {
    let previous = previous?;
    catalog
        .iter()
        .find(|row| row.id() == previous)
        .map(|row| row.id())
}

#[cfg(test)]
mod tests {
    use super::MaxMeetingDuration;
    use super::{
        format_duration_ms, preserve_selection, recording_day, synthetic_recording_id,
        RecordingContainer, RecordingDay, RecordingFormat, RecordingInfo, SidecarState,
        TranscriptStatus,
    };
    use crate::domain::audio::CivilTime;
    use crate::domain::ids::RecordingId;

    #[test]
    fn max_meeting_duration_defaults_to_three_hours() {
        // Arrange

        // Act
        let cap = MaxMeetingDuration::default();

        // Assert
        assert_eq!(cap.hours(), 3);
    }

    #[test]
    fn max_meeting_duration_accepts_only_the_offered_choices() {
        // Arrange
        let hours = [0, 1, 5, 12, 13];

        // Act
        let parsed: Vec<_> = hours
            .into_iter()
            .map(|h| MaxMeetingDuration::from_hours(h).map(MaxMeetingDuration::hours))
            .collect();

        // Assert
        assert_eq!(parsed, vec![None, Some(1), None, Some(12), None]);
        assert_eq!(MaxMeetingDuration::CHOICES_HOURS, [1, 2, 3, 4, 6, 8, 12]);
    }

    #[test]
    fn max_meeting_duration_converts_to_frames() {
        // Arrange
        let one = MaxMeetingDuration::from_hours(1).expect("one");

        // Act
        let frames = one.frames(48_000);

        // Assert
        assert_eq!(frames, 3_600 * 48_000);
    }

    fn row(id: u64, name: &str, started: Option<u64>) -> RecordingInfo {
        RecordingInfo::new(
            RecordingId::from_raw(id),
            name,
            RecordingContainer::Flac,
            started,
            Some(1_000),
            100,
            Some(RecordingFormat::skrivli_default()),
            TranscriptStatus::None,
            SidecarState::Present,
            Vec::new(),
        )
    }

    #[test]
    fn settled_status_covers_every_combination() {
        use super::settled_transcript_status as settle;
        use TranscriptStatus::{Failed, None as Nothing, Progress, Queued, Ready};
        // Arrange
        let cases = [
            (Failed, false, false, Failed),
            (Failed, true, false, Nothing),
            (Failed, false, true, Ready),
            (Failed, true, true, Ready),
            (Ready, false, true, Ready),
            (Ready, true, false, Ready),
            (Nothing, true, false, Nothing),
            (Queued, false, true, Queued),
            (Progress(7), true, true, Progress(7)),
        ];

        for (stored, cancelled, file, expected) in cases {
            // Act
            let status = settle(stored, cancelled, file);

            // Assert
            assert_eq!(status, expected, "{stored:?} {cancelled} {file}");
        }
    }

    #[test]
    fn container_extension_round_trip() {
        // Arrange / Act / Assert
        assert_eq!(RecordingContainer::Flac.extension(), "flac");
        assert_eq!(
            RecordingContainer::from_extension("flac"),
            Some(RecordingContainer::Flac)
        );
        assert_eq!(RecordingContainer::from_extension("ogg"), None);
        assert_eq!(RecordingContainer::from_extension("m4a"), None);
        assert_eq!(RecordingContainer::from_extension("partial"), None);
    }

    #[test]
    fn synthetic_id_is_stable_for_same_stem() {
        // Arrange
        let stem = "Skrivli_2026-11-04_13-01-02";

        // Act
        let a = synthetic_recording_id(stem);
        let b = synthetic_recording_id(stem);

        // Assert
        assert_eq!(a, b);
        assert_ne!(a.raw(), 0);
    }

    #[test]
    fn preserve_selection_keeps_id_across_catalog_refresh() {
        // Arrange
        let previous = Some(RecordingId::from_raw(7));
        let catalog = vec![row(3, "older", Some(1)), row(7, "kept", Some(2))];

        // Act
        let selected = preserve_selection(previous, &catalog);

        // Assert
        assert_eq!(selected, Some(RecordingId::from_raw(7)));
        assert_eq!(preserve_selection(previous, &[]), None);
    }

    #[test]
    fn format_helpers_are_stable() {
        // Arrange / Act / Assert
        assert_eq!(format_duration_ms(65_000), "01:05");
        assert_eq!(format_duration_ms(3_661_000), "01:01:01");
    }

    fn civil(year: i32, month: u8, day: u8, hour: u8, minute: u8) -> CivilTime {
        CivilTime::new(year, month, day, hour, minute, 0).expect("valid civil time")
    }

    #[test]
    fn recording_day_splits_at_local_midnight() {
        // Arrange
        let today = civil(2026, 11, 7, 12, 0);
        let late_yesterday = civil(2026, 11, 6, 23, 59);
        let early_today = civil(2026, 11, 7, 0, 1);
        let two_days_ago = civil(2026, 11, 5, 23, 59);

        // Act
        let groups = [late_yesterday, early_today, two_days_ago].map(|t| recording_day(t, today));

        // Assert
        assert_eq!(
            groups,
            [
                RecordingDay::Yesterday,
                RecordingDay::Today,
                RecordingDay::Earlier
            ]
        );
    }

    #[test]
    fn recording_day_handles_month_and_year_boundaries() {
        // Arrange
        let new_year = civil(2027, 1, 1, 9, 0);
        let first_of_march_leap = civil(2028, 3, 1, 9, 0);
        let first_of_march = civil(2027, 3, 1, 9, 0);

        // Act
        let dec_31 = recording_day(civil(2026, 12, 31, 20, 0), new_year);
        let feb_29 = recording_day(civil(2028, 2, 29, 20, 0), first_of_march_leap);
        let feb_28 = recording_day(civil(2027, 2, 28, 20, 0), first_of_march);
        let feb_27_two_days_back = recording_day(civil(2027, 2, 27, 20, 0), first_of_march);

        // Assert
        assert_eq!(dec_31, RecordingDay::Yesterday);
        assert_eq!(feb_29, RecordingDay::Yesterday);
        assert_eq!(feb_28, RecordingDay::Yesterday);
        assert_eq!(feb_27_two_days_back, RecordingDay::Earlier);
    }

    #[test]
    fn recording_day_treats_future_dates_as_today() {
        // Arrange
        let today = civil(2026, 11, 7, 12, 0);

        // Act
        let day = recording_day(civil(2026, 11, 8, 1, 0), today);

        // Assert
        assert_eq!(day, RecordingDay::Today);
    }

    #[test]
    fn recording_format_rejects_zero_rate() {
        // Arrange / Act / Assert
        assert!(RecordingFormat::new(0, 2, true).is_none());
        assert_eq!(RecordingFormat::skrivli_default().sample_rate(), 48_000);
    }
}
