//! Sample formats, endpoint choice, and the session timeline.
//!
//! No Windows, GPUI, or file types live here. Capture code converts into the
//! internal format before a sample index is placed on the timeline.

use std::time::Duration;

/// Internal processing and FLAC rate.
pub(crate) const INTERNAL_SAMPLE_RATE: u32 = 48_000;
/// Internal channel count. Mono sources are duplicated onto both channels.
pub(crate) const INTERNAL_CHANNELS: u16 = 2;

/// One capture packet on the session timeline.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AudioBlock {
    pub(crate) samples: Vec<f32>,
    /// First output frame on the session timeline. `None` appends.
    pub(crate) timeline_frame: Option<u64>,
    pub(crate) discontinuity: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum SampleEncoding {
    I16,
    I32,
    F32,
}

impl SampleEncoding {
    pub(crate) fn bytes_per_sample(self) -> u64 {
        match self {
            Self::I16 => 2,
            Self::I32 | Self::F32 => 4,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AudioFormat {
    sample_rate: u32,
    channels: u16,
    encoding: SampleEncoding,
}

#[allow(dead_code)]
impl AudioFormat {
    pub(crate) fn new(sample_rate: u32, channels: u16, encoding: SampleEncoding) -> Option<Self> {
        if sample_rate == 0 || channels == 0 || channels > 8 {
            return None;
        }
        Some(Self {
            sample_rate,
            channels,
            encoding,
        })
    }

    pub(crate) fn internal() -> Self {
        Self {
            sample_rate: INTERNAL_SAMPLE_RATE,
            channels: INTERNAL_CHANNELS,
            encoding: SampleEncoding::F32,
        }
    }

    pub(crate) fn sample_rate(self) -> u32 {
        self.sample_rate
    }

    pub(crate) fn channels(self) -> u16 {
        self.channels
    }

    pub(crate) fn encoding(self) -> SampleEncoding {
        self.encoding
    }

    pub(crate) fn bytes_per_frame(self) -> Option<u64> {
        u64::from(self.channels).checked_mul(self.encoding.bytes_per_sample())
    }

    /// Frame count when `bytes` is an exact multiple of the frame size.
    pub(crate) fn frames_from_bytes(self, bytes: u64) -> Option<u64> {
        let frame = self.bytes_per_frame()?;
        if frame == 0 || !bytes.is_multiple_of(frame) {
            return None;
        }
        Some(bytes / frame)
    }

    pub(crate) fn bytes_for_frames(self, frames: u64) -> Option<u64> {
        self.bytes_per_frame()?.checked_mul(frames)
    }
}

/// Stable selectable endpoint id from flexaudio device enumeration.
///
/// Microphones use cpal device names; system outputs typically use FriendlyName
/// (native id only when names collide or are unreadable).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct EndpointId(String);

impl EndpointId {
    pub(crate) fn new(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(Self(trimmed.to_string()))
        }
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EndpointFlow {
    Capture,
    Render,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum EndpointAvailability {
    Active,
    Disabled,
    Absent,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EndpointDescriptor {
    id: EndpointId,
    name: String,
    flow: EndpointFlow,
    availability: EndpointAvailability,
}

impl EndpointDescriptor {
    pub(crate) fn new(
        id: EndpointId,
        name: impl Into<String>,
        flow: EndpointFlow,
        availability: EndpointAvailability,
    ) -> Self {
        let name = name.into();
        let name = if name.trim().is_empty() {
            match flow {
                EndpointFlow::Capture => "Microphone".to_string(),
                EndpointFlow::Render => "Output".to_string(),
            }
        } else {
            name
        };
        Self {
            id,
            name,
            flow,
            availability,
        }
    }

    pub(crate) fn id(&self) -> &EndpointId {
        &self.id
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn flow(&self) -> EndpointFlow {
        self.flow
    }

    pub(crate) fn availability(&self) -> EndpointAvailability {
        self.availability
    }

    pub(crate) fn is_active(&self) -> bool {
        self.availability() == EndpointAvailability::Active
    }
}

/// Explicit endpoint id, or the OS default for the role.
///
/// Defaults come from flexaudio device enumeration (`is_default`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum EndpointChoice {
    #[default]
    Default,
    Explicit(EndpointId),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ResolvedEndpoint {
    id: EndpointId,
    name: String,
    fell_back: bool,
}

impl ResolvedEndpoint {
    pub(crate) fn id(&self) -> &EndpointId {
        &self.id
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn fell_back(&self) -> bool {
        self.fell_back
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResolveError {
    NoneAvailable,
}

/// Pick an active endpoint. A missing or disabled explicit id falls back to
/// the active default, then to the first active endpoint, and reports that
/// fallback. Choosing Default uses the default directly.
pub(crate) fn resolve_endpoint(
    choice: &EndpointChoice,
    flow: EndpointFlow,
    devices: &[EndpointDescriptor],
    default_id: Option<&EndpointId>,
) -> Result<ResolvedEndpoint, ResolveError> {
    let active: Vec<&EndpointDescriptor> = devices
        .iter()
        .filter(|device| device.flow() == flow && device.is_active())
        .collect();
    let found = |id: &EndpointId| active.iter().copied().find(|device| device.id() == id);
    match choice {
        EndpointChoice::Explicit(id) => {
            if let Some(device) = found(id) {
                return Ok(resolved(device, false));
            }
            if let Some(id) = default_id {
                if let Some(device) = found(id) {
                    return Ok(resolved(device, true));
                }
            }
            active
                .first()
                .copied()
                .map(|device| resolved(device, true))
                .ok_or(ResolveError::NoneAvailable)
        }
        EndpointChoice::Default => {
            if let Some(id) = default_id {
                if let Some(device) = found(id) {
                    return Ok(resolved(device, false));
                }
            }
            active
                .first()
                .copied()
                .map(|device| resolved(device, true))
                .ok_or(ResolveError::NoneAvailable)
        }
    }
}

fn resolved(device: &EndpointDescriptor, fell_back: bool) -> ResolvedEndpoint {
    ResolvedEndpoint {
        id: device.id().clone(),
        name: device.name().to_string(),
        fell_back,
    }
}

/// QPC ticks at `t0` and the counter frequency. Sample indexes use integer
/// arithmetic so the timeline does not drift with floating-point rounding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SessionOrigin {
    origin_ticks: u64,
    ticks_per_second: u64,
    sample_rate: u32,
}

impl SessionOrigin {
    pub(crate) fn new(origin_ticks: u64, ticks_per_second: u64, sample_rate: u32) -> Option<Self> {
        if ticks_per_second == 0 || sample_rate == 0 {
            return None;
        }
        Some(Self {
            origin_ticks,
            ticks_per_second,
            sample_rate,
        })
    }

    pub(crate) fn elapsed_ticks(self, timestamp: u64) -> Option<u64> {
        timestamp.checked_sub(self.origin_ticks)
    }

    /// Output-frame index at `timestamp`, measured from the session origin.
    pub(crate) fn sample_index(self, timestamp: u64) -> Option<u64> {
        let delta = self.elapsed_ticks(timestamp)?;
        let samples = u128::from(delta)
            .checked_mul(u128::from(self.sample_rate))?
            .checked_div(u128::from(self.ticks_per_second))?;
        u64::try_from(samples).ok()
    }

    #[allow(dead_code)]
    pub(crate) fn ticks_per_second(self) -> u64 {
        self.ticks_per_second
    }
}

/// Local civil time used only to name a recording. The clock is injected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CivilTime {
    pub(crate) year: i32,
    pub(crate) month: u8,
    pub(crate) day: u8,
    pub(crate) hour: u8,
    pub(crate) minute: u8,
    pub(crate) second: u8,
}

impl CivilTime {
    pub(crate) fn new(
        year: i32,
        month: u8,
        day: u8,
        hour: u8,
        minute: u8,
        second: u8,
    ) -> Option<Self> {
        if !(1..=12).contains(&month)
            || !(1..=31).contains(&day)
            || hour > 23
            || minute > 59
            || second > 59
        {
            return None;
        }
        Some(Self {
            year,
            month,
            day,
            hour,
            minute,
            second,
        })
    }
}

pub(crate) fn format_elapsed(elapsed: Duration) -> String {
    let total = elapsed.as_secs();
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let seconds = total % 60;
    format!("{hours:02}:{minutes:02}:{seconds:02}")
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{
        format_elapsed, resolve_endpoint, AudioFormat, CivilTime, EndpointAvailability,
        EndpointChoice, EndpointDescriptor, EndpointFlow, EndpointId, ResolveError, SampleEncoding,
        SessionOrigin, INTERNAL_CHANNELS, INTERNAL_SAMPLE_RATE,
    };

    fn id(value: &str) -> EndpointId {
        EndpointId::new(value).expect("id")
    }

    fn mic(value: &str, availability: EndpointAvailability) -> EndpointDescriptor {
        EndpointDescriptor::new(id(value), value, EndpointFlow::Capture, availability)
    }

    #[test]
    fn frame_counts_use_checked_arithmetic() {
        // Arrange
        let format = AudioFormat::new(48_000, 2, SampleEncoding::I16).expect("format");

        // Act
        let frames = format.frames_from_bytes(8);
        let bytes = format.bytes_for_frames(3);
        let overflow = format.bytes_for_frames(u64::MAX);

        // Assert
        assert_eq!(format.sample_rate(), INTERNAL_SAMPLE_RATE);
        assert_eq!(AudioFormat::internal().channels(), INTERNAL_CHANNELS);
        assert_eq!(frames, Some(2));
        assert_eq!(bytes, Some(12));
        assert_eq!(format.frames_from_bytes(6), None);
        assert_eq!(overflow, None);
        assert!(AudioFormat::new(0, 2, SampleEncoding::F32).is_none());
        assert!(AudioFormat::new(48_000, 0, SampleEncoding::F32).is_none());
    }

    #[test]
    fn sample_index_is_monotonic_and_rejects_overflow() {
        // Arrange
        let origin = SessionOrigin::new(1_000, 48_000, 48_000).expect("origin");

        // Act
        let at_origin = origin.sample_index(1_000);
        let one_second = origin.sample_index(49_000);
        let before = origin.sample_index(999);
        let huge = SessionOrigin::new(0, 1, u32::MAX)
            .expect("origin")
            .sample_index(u64::MAX);

        // Assert
        assert_eq!(origin.elapsed_ticks(1_500), Some(500));
        assert_eq!(at_origin, Some(0));
        assert_eq!(one_second, Some(48_000));
        assert_eq!(before, None);
        assert_eq!(huge, None);
        assert!(SessionOrigin::new(0, 0, 48_000).is_none());
    }

    #[test]
    fn missing_or_disabled_microphone_falls_back_to_the_default() {
        // Arrange
        let devices = vec![
            mic("gone", EndpointAvailability::Disabled),
            mic("default-mic", EndpointAvailability::Active),
            EndpointDescriptor::new(
                id("speakers"),
                "Speakers",
                EndpointFlow::Render,
                EndpointAvailability::Active,
            ),
        ];
        let default_id = id("default-mic");

        // Act
        let disabled = resolve_endpoint(
            &EndpointChoice::Explicit(id("gone")),
            EndpointFlow::Capture,
            &devices,
            Some(&default_id),
        )
        .expect("fallback");
        let absent = resolve_endpoint(
            &EndpointChoice::Explicit(id("unplugged")),
            EndpointFlow::Capture,
            &devices,
            Some(&default_id),
        )
        .expect("fallback");
        let explicit = resolve_endpoint(
            &EndpointChoice::Explicit(id("default-mic")),
            EndpointFlow::Capture,
            &devices,
            Some(&default_id),
        )
        .expect("explicit");
        let default_choice = resolve_endpoint(
            &EndpointChoice::Default,
            EndpointFlow::Capture,
            &devices,
            Some(&default_id),
        )
        .expect("default");
        let empty = resolve_endpoint(&EndpointChoice::Default, EndpointFlow::Capture, &[], None);

        // Assert
        assert!(disabled.fell_back());
        assert_eq!(disabled.id().as_str(), "default-mic");
        assert!(absent.fell_back());
        assert!(!explicit.fell_back());
        assert!(!default_choice.fell_back());
        assert_eq!(default_choice.name(), "default-mic");
        let absent_device = EndpointDescriptor::new(
            id("missing"),
            "Missing",
            EndpointFlow::Capture,
            EndpointAvailability::Absent,
        );
        assert_eq!(absent_device.availability(), EndpointAvailability::Absent);
        assert_eq!(empty, Err(ResolveError::NoneAvailable));
        assert!(CivilTime::new(2026, 11, 4, 13, 5, 6).is_some());
        assert!(CivilTime::new(2026, 13, 4, 0, 0, 0).is_none());
    }

    #[test]
    fn elapsed_clock_formats_hours() {
        // Arrange
        let elapsed = Duration::from_secs(3661);

        // Act
        let text = format_elapsed(elapsed);

        // Assert
        assert_eq!(text, "01:01:01");
    }
}
