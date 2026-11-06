//! Fixed 10 ms frames for the processing pipeline.
//!
//! No GPUI, Windows, COM, file I/O, or AI types. Callers reuse one frame's
//! buffer across pulls so the steady path does not allocate.

// The session loop starts using frames in a later phase. Tests use them now.
#![cfg_attr(not(test), allow(dead_code))]

use std::fmt;

/// 10 ms at 48 kHz.
pub(crate) const FRAME_LEN: usize = 480;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChannelLayout {
    Mono,
    Stereo,
}

impl ChannelLayout {
    fn sample_len(self) -> usize {
        match self {
            Self::Mono => FRAME_LEN,
            Self::Stereo => FRAME_LEN * 2,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AudioFrameError {
    WrongLength { expected: usize, actual: usize },
    ValidFrames { valid_frames: usize },
}

impl fmt::Display for AudioFrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongLength { expected, actual } => {
                write!(f, "audio frame has {actual} samples, expected {expected}")
            }
            Self::ValidFrames { valid_frames } => {
                write!(f, "valid frame count {valid_frames} exceeds {FRAME_LEN}")
            }
        }
    }
}

impl std::error::Error for AudioFrameError {}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AudioFrame {
    layout: ChannelLayout,
    samples: Vec<f32>,
    valid_frames: usize,
    timeline_frame: u64,
}

impl AudioFrame {
    pub(crate) fn new(
        layout: ChannelLayout,
        samples: Vec<f32>,
        valid_frames: usize,
        timeline_frame: u64,
    ) -> Result<Self, AudioFrameError> {
        let expected = layout.sample_len();
        if samples.len() != expected {
            return Err(AudioFrameError::WrongLength {
                expected,
                actual: samples.len(),
            });
        }
        if valid_frames > FRAME_LEN {
            return Err(AudioFrameError::ValidFrames { valid_frames });
        }
        Ok(Self {
            layout,
            samples,
            valid_frames,
            timeline_frame,
        })
    }

    pub(crate) fn silence(layout: ChannelLayout, timeline_frame: u64) -> Self {
        Self {
            layout,
            samples: vec![0.0; layout.sample_len()],
            valid_frames: FRAME_LEN,
            timeline_frame,
        }
    }

    pub(crate) fn layout(&self) -> ChannelLayout {
        self.layout
    }

    pub(crate) fn samples(&self) -> &[f32] {
        &self.samples
    }

    pub(crate) fn samples_mut(&mut self) -> &mut [f32] {
        &mut self.samples
    }

    pub(crate) fn valid_frames(&self) -> usize {
        self.valid_frames
    }

    pub(crate) fn timeline_frame(&self) -> u64 {
        self.timeline_frame
    }

    /// Copies `samples` into the existing buffer when the length already matches.
    pub(crate) fn fill_samples(&mut self, samples: &[f32]) -> Result<(), AudioFrameError> {
        if samples.len() != self.samples.len() {
            return Err(AudioFrameError::WrongLength {
                expected: self.samples.len(),
                actual: samples.len(),
            });
        }
        self.samples.copy_from_slice(samples);
        Ok(())
    }

    /// Zeros the buffer and replaces the metadata. The allocation is kept when
    /// `layout` needs the same number of samples.
    pub(crate) fn reset(
        &mut self,
        layout: ChannelLayout,
        valid_frames: usize,
        timeline_frame: u64,
    ) -> Result<(), AudioFrameError> {
        if valid_frames > FRAME_LEN {
            return Err(AudioFrameError::ValidFrames { valid_frames });
        }
        let needed = layout.sample_len();
        if self.samples.len() != needed {
            self.samples.resize(needed, 0.0);
        }
        self.samples.fill(0.0);
        self.layout = layout;
        self.valid_frames = valid_frames;
        self.timeline_frame = timeline_frame;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{AudioFrame, AudioFrameError, ChannelLayout, FRAME_LEN};

    #[test]
    fn audio_frame_rejects_a_buffer_of_the_wrong_length() {
        // Arrange
        let short_mono = vec![0.0; FRAME_LEN - 1];
        let short_stereo = vec![0.0; FRAME_LEN];
        let long_mono = vec![0.0; FRAME_LEN * 2];

        // Act
        let mono = AudioFrame::new(ChannelLayout::Mono, short_mono, FRAME_LEN, 0);
        let stereo = AudioFrame::new(ChannelLayout::Stereo, short_stereo, FRAME_LEN, 0);
        let long = AudioFrame::new(ChannelLayout::Mono, long_mono, FRAME_LEN, 0);

        // Assert
        assert!(matches!(
            mono,
            Err(AudioFrameError::WrongLength { expected, actual })
                if expected == FRAME_LEN && actual == FRAME_LEN - 1
        ));
        assert!(matches!(
            stereo,
            Err(AudioFrameError::WrongLength { expected, actual })
                if expected == FRAME_LEN * 2 && actual == FRAME_LEN
        ));
        assert!(matches!(
            long,
            Err(AudioFrameError::WrongLength { expected, actual })
                if expected == FRAME_LEN && actual == FRAME_LEN * 2
        ));
    }

    #[test]
    fn audio_frame_rejects_too_many_valid_frames() {
        // Arrange
        let samples = vec![0.0; FRAME_LEN];
        let mut frame = AudioFrame::silence(ChannelLayout::Mono, 3);

        // Act
        let created = AudioFrame::new(ChannelLayout::Mono, samples, FRAME_LEN + 1, 0);
        let reset = frame.reset(ChannelLayout::Mono, FRAME_LEN + 1, 9);

        // Assert
        assert!(matches!(
            created,
            Err(AudioFrameError::ValidFrames { valid_frames }) if valid_frames == FRAME_LEN + 1
        ));
        assert!(matches!(
            reset,
            Err(AudioFrameError::ValidFrames { valid_frames }) if valid_frames == FRAME_LEN + 1
        ));
        assert_eq!(frame.valid_frames(), FRAME_LEN);
        assert_eq!(frame.timeline_frame(), 3);
    }

    #[test]
    fn fill_samples_reuses_the_buffer_when_the_length_matches() {
        // Arrange
        let mut frame = AudioFrame::silence(ChannelLayout::Mono, 4);
        let pointer = frame.samples().as_ptr();
        let incoming = vec![0.25; FRAME_LEN];

        // Act
        let filled = frame.fill_samples(&incoming);

        // Assert
        assert!(filled.is_ok());
        assert_eq!(frame.samples().as_ptr(), pointer);
        assert_eq!(frame.samples(), incoming.as_slice());
        assert_eq!(frame.timeline_frame(), 4);
        let rejected = frame.fill_samples(&[1.0; 4]);
        assert!(matches!(
            rejected,
            Err(AudioFrameError::WrongLength {
                expected: FRAME_LEN,
                actual: 4
            })
        ));
        assert_eq!(frame.samples()[0], 0.25);
        let copy = frame.clone();
        assert_eq!(copy.samples(), frame.samples());
    }

    #[test]
    fn reset_clears_samples_without_reallocating_the_same_layout() {
        // Arrange
        let mut frame = AudioFrame::new(
            ChannelLayout::Stereo,
            vec![0.5; FRAME_LEN * 2],
            FRAME_LEN,
            1,
        )
        .expect("stereo frame");
        let pointer = frame.samples().as_ptr();

        // Act
        let reset = frame.reset(ChannelLayout::Stereo, 12, 80);

        // Assert
        assert!(reset.is_ok());
        assert_eq!(frame.samples().as_ptr(), pointer);
        assert!(frame.samples().iter().all(|sample| *sample == 0.0));
        assert_eq!(frame.layout(), ChannelLayout::Stereo);
        assert_eq!(frame.valid_frames(), 12);
        assert_eq!(frame.timeline_frame(), 80);
    }
}
