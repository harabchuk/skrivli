//! Ordered processors and taps. A failed step never ends the chain.

use std::panic::{catch_unwind, AssertUnwindSafe};

use crate::domain::audio_frame::AudioFrame;
use crate::services::ports::{AudioProcessor, AudioTap, ProcessContext};

struct ProcessStep {
    name: &'static str,
    processor: Box<dyn AudioProcessor>,
    disabled: bool,
    logged: bool,
}

enum Step {
    Process(ProcessStep),
    Tap(Box<dyn AudioTap>),
}

#[must_use]
pub(crate) struct Chain {
    steps: Vec<Step>,
    failure_count: u64,
    /// A step was left out because its constructor failed.
    omitted_step: bool,
}

impl Chain {
    pub(crate) fn new() -> Self {
        Self {
            steps: Vec::new(),
            failure_count: 0,
            omitted_step: false,
        }
    }

    /// Records that a step was skipped because it could not be built.
    ///
    /// Omitting a step because system audio is off is not a failure. Call
    /// this only when construction returned an error.
    pub(crate) fn construction_failed(mut self) -> Self {
        self.omitted_step = true;
        self
    }

    pub(crate) fn process(mut self, processor: impl AudioProcessor + 'static) -> Self {
        let name = processor.name();
        self.steps.push(Step::Process(ProcessStep {
            name,
            processor: Box::new(processor),
            disabled: false,
            logged: false,
        }));
        self
    }

    pub(crate) fn tap(mut self, tap: impl AudioTap + 'static) -> Self {
        self.steps.push(Step::Tap(Box::new(tap)));
        self
    }

    /// Applies each step in builder order.
    ///
    /// `Err` leaves the frame as the processor returned it and does not stop
    /// the chain. A panic disables that step for the rest of this chain; the
    /// frame may already have been partly updated.
    pub(crate) fn run(&mut self, frame: &mut AudioFrame, context: &ProcessContext<'_>) {
        let mut failures = 0_u64;
        for step in &mut self.steps {
            match step {
                Step::Tap(tap) => tap.observe(frame),
                Step::Process(process) => {
                    if process.disabled {
                        continue;
                    }
                    let ProcessStep {
                        name,
                        processor,
                        disabled,
                        logged,
                    } = process;
                    let outcome =
                        catch_unwind(AssertUnwindSafe(|| processor.process(frame, context)));
                    match outcome {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => {
                            failures = failures.saturating_add(1);
                            if !*logged {
                                *logged = true;
                                tracing::warn!(
                                    step = *name,
                                    error = %error,
                                    "audio processor failed"
                                );
                            }
                        }
                        Err(_) => {
                            failures = failures.saturating_add(1);
                            *disabled = true;
                            if !*logged {
                                *logged = true;
                                tracing::error!(
                                    step = *name,
                                    "audio processor panicked and was disabled"
                                );
                            }
                        }
                    }
                }
            }
        }
        self.failure_count = self.failure_count.saturating_add(failures);
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn failure_count(&self) -> u64 {
        self.failure_count
    }

    /// `Err`, a panic, or a constructor that was skipped after it failed.
    pub(crate) fn processing_problem(&self) -> bool {
        self.omitted_step || self.failure_count > 0
    }

    /// Processor names in chain order. Taps are omitted.
    #[cfg(test)]
    pub(crate) fn process_names(&self) -> Vec<&'static str> {
        self.steps
            .iter()
            .filter_map(|step| match step {
                Step::Process(process) => Some(process.name),
                Step::Tap(_) => None,
            })
            .collect()
    }
}

impl Default for Chain {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use crate::domain::audio_frame::{AudioFrame, ChannelLayout, FRAME_LEN};
    use crate::services::audio_pipeline::steps::FixedGain;
    use crate::services::audio_pipeline::Chain;
    use crate::services::ports::{AudioProcessor, AudioTap, ProcessContext, ProcessError};

    fn mono_frame(value: f32) -> AudioFrame {
        AudioFrame::new(ChannelLayout::Mono, vec![value; FRAME_LEN], FRAME_LEN, 0)
            .expect("mono frame")
    }

    fn context() -> ProcessContext<'static> {
        ProcessContext {
            echo_reference: None,
        }
    }

    struct SampleTap {
        seen: Arc<Mutex<Vec<f32>>>,
    }

    impl AudioTap for SampleTap {
        fn observe(&mut self, frame: &AudioFrame) {
            *self.seen.lock().expect("tap samples") = frame.samples().to_vec();
        }
    }

    struct RestoringFailure {
        calls: Arc<AtomicUsize>,
    }

    impl AudioProcessor for RestoringFailure {
        fn name(&self) -> &'static str {
            "failing-step"
        }

        fn process(
            &mut self,
            frame: &mut AudioFrame,
            _context: &ProcessContext<'_>,
        ) -> Result<(), ProcessError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let original = frame.samples().to_vec();
            frame.samples_mut().fill(7.0);
            frame.samples_mut().copy_from_slice(&original);
            Err(ProcessError::new(self.name(), "step failed"))
        }
    }

    struct Scale {
        calls: Arc<AtomicUsize>,
        seen: Arc<Mutex<f32>>,
        gain: f32,
    }

    impl AudioProcessor for Scale {
        fn name(&self) -> &'static str {
            "next-step"
        }

        fn process(
            &mut self,
            frame: &mut AudioFrame,
            _context: &ProcessContext<'_>,
        ) -> Result<(), ProcessError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            *self.seen.lock().expect("seen") = frame.samples()[0];
            for sample in frame.samples_mut() {
                *sample *= self.gain;
            }
            Ok(())
        }
    }

    struct Panicker {
        calls: Arc<AtomicUsize>,
    }

    impl AudioProcessor for Panicker {
        fn name(&self) -> &'static str {
            "panicking-step"
        }

        fn process(
            &mut self,
            _frame: &mut AudioFrame,
            _context: &ProcessContext<'_>,
        ) -> Result<(), ProcessError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            panic!("audio step panicked");
        }
    }

    struct Add {
        calls: Arc<AtomicUsize>,
        offset: f32,
    }

    impl AudioProcessor for Add {
        fn name(&self) -> &'static str {
            "later-step"
        }

        fn process(
            &mut self,
            frame: &mut AudioFrame,
            _context: &ProcessContext<'_>,
        ) -> Result<(), ProcessError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            for sample in frame.samples_mut() {
                *sample += self.offset;
            }
            Ok(())
        }
    }

    struct AddThenLog {
        name: &'static str,
        offset: f32,
        log: Arc<Mutex<Vec<&'static str>>>,
    }

    impl AudioProcessor for AddThenLog {
        fn name(&self) -> &'static str {
            self.name
        }

        fn process(
            &mut self,
            frame: &mut AudioFrame,
            _context: &ProcessContext<'_>,
        ) -> Result<(), ProcessError> {
            self.log.lock().expect("order log").push(self.name);
            for sample in frame.samples_mut() {
                *sample += self.offset;
            }
            Ok(())
        }
    }

    struct ScaleThenLog {
        name: &'static str,
        gain: f32,
        log: Arc<Mutex<Vec<&'static str>>>,
    }

    impl AudioProcessor for ScaleThenLog {
        fn name(&self) -> &'static str {
            self.name
        }

        fn process(
            &mut self,
            frame: &mut AudioFrame,
            _context: &ProcessContext<'_>,
        ) -> Result<(), ProcessError> {
            self.log.lock().expect("order log").push(self.name);
            for sample in frame.samples_mut() {
                *sample *= self.gain;
            }
            Ok(())
        }
    }

    #[test]
    fn processors_run_in_builder_order() {
        // Arrange
        // Addition then scaling does not commute, so a reversed chain cannot pass.
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut chain = Chain::new()
            .process(AddThenLog {
                name: "add",
                offset: 0.25,
                log: Arc::clone(&log),
            })
            .process(ScaleThenLog {
                name: "scale",
                gain: 2.0,
                log: Arc::clone(&log),
            });
        let mut frame = mono_frame(0.5);

        // Act
        chain.run(&mut frame, &context());

        // Assert
        assert_eq!(*log.lock().expect("order log"), vec!["add", "scale"]);
        assert_eq!(frame.samples()[0], 1.5);
        assert_eq!(frame.samples()[FRAME_LEN - 1], 1.5);
    }

    #[test]
    fn tap_between_steps_sees_the_first_step_only() {
        // Arrange
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut chain = Chain::new()
            .process(FixedGain::new(2.0).expect("gain"))
            .tap(SampleTap {
                seen: Arc::clone(&seen),
            })
            .process(FixedGain::new(0.5).expect("gain"));
        let mut frame = mono_frame(0.25);

        // Act
        chain.run(&mut frame, &context());

        // Assert
        let recorded = seen.lock().expect("tap samples").clone();
        assert_eq!(recorded, vec![0.5; FRAME_LEN]);
        assert_eq!(frame.samples(), vec![0.25; FRAME_LEN]);
    }

    #[test]
    fn failed_step_leaves_audio_and_recording_running() {
        // Arrange
        let fail_calls = Arc::new(AtomicUsize::new(0));
        let next_calls = Arc::new(AtomicUsize::new(0));
        let seen = Arc::new(Mutex::new(0.0));
        let mut chain = Chain::new()
            .process(RestoringFailure {
                calls: Arc::clone(&fail_calls),
            })
            .process(Scale {
                calls: Arc::clone(&next_calls),
                seen: Arc::clone(&seen),
                gain: 2.0,
            });
        let mut frame = mono_frame(0.25);

        // Act
        chain.run(&mut frame, &context());
        let after_first = frame.samples()[0];
        let seen_first = *seen.lock().expect("seen");
        chain.run(&mut frame, &context());

        // Assert
        assert_eq!(seen_first, 0.25);
        assert_eq!(after_first, 0.5);
        assert_eq!(frame.samples()[0], 1.0);
        assert_eq!(fail_calls.load(Ordering::SeqCst), 2);
        assert_eq!(next_calls.load(Ordering::SeqCst), 2);
        assert_eq!(chain.failure_count(), 2);
    }

    #[test]
    fn panicking_step_is_disabled_for_the_session() {
        // Arrange
        let panic_calls = Arc::new(AtomicUsize::new(0));
        let later_calls = Arc::new(AtomicUsize::new(0));
        let mut chain = Chain::new()
            .process(Panicker {
                calls: Arc::clone(&panic_calls),
            })
            .process(Add {
                calls: Arc::clone(&later_calls),
                offset: 0.25,
            });
        let mut frame = mono_frame(0.5);

        // Act
        chain.run(&mut frame, &context());
        let after_first = frame.samples()[0];
        chain.run(&mut frame, &context());

        // Assert
        assert_eq!(panic_calls.load(Ordering::SeqCst), 1);
        assert_eq!(later_calls.load(Ordering::SeqCst), 2);
        assert_eq!(after_first, 0.75);
        assert_eq!(frame.samples()[0], 1.0);
        assert_eq!(frame.samples()[FRAME_LEN - 1], 1.0);
        assert_eq!(chain.failure_count(), 1);
    }
}
