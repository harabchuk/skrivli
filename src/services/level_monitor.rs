//! Idle peak monitor for the selected microphone and render endpoint.
//!
//! It opens those devices only while nothing is recording. Before a recording
//! starts, the recording thread takes the endpoint lease and this monitor
//! closes its clients. It opens them again after the recording releases the
//! lease, including after a failed start.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::domain::audio::SessionOrigin;
use crate::domain::gain::SYSTEM_GAIN;
use crate::infrastructure::audio::levels::{GainControl, LevelSnapshot};
use crate::services::recording::CapturePort;

use super::lease::EndpointLease;

struct TargetState {
    mic: String,
    system: String,
    generation: u64,
}

pub(crate) struct MonitorTargets {
    inner: Mutex<TargetState>,
}

impl MonitorTargets {
    pub(crate) fn new() -> Self {
        Self {
            inner: Mutex::new(TargetState {
                mic: String::new(),
                system: String::new(),
                generation: 1,
            }),
        }
    }

    pub(crate) fn set(&self, mic: String, system: String) {
        let mut state = self
            .inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if state.mic == mic && state.system == system {
            return;
        }
        state.mic = mic;
        state.system = system;
        state.generation = state.generation.saturating_add(1);
    }

    fn snapshot(&self) -> (String, String, u64) {
        let state = self
            .inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        (state.mic.clone(), state.system.clone(), state.generation)
    }
}

pub(crate) fn spawn_level_monitor(
    mic: Box<dyn CapturePort>,
    system: Box<dyn CapturePort>,
    lease: Arc<EndpointLease>,
    levels: Arc<LevelSnapshot>,
    gain: Arc<GainControl>,
    targets: Arc<MonitorTargets>,
    stop: Arc<AtomicBool>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        monitor_loop(mic, system, &lease, &levels, &gain, &targets, &stop);
    })
}

fn monitor_loop(
    mut mic_port: Box<dyn CapturePort>,
    mut system_port: Box<dyn CapturePort>,
    lease: &EndpointLease,
    levels: &LevelSnapshot,
    gain: &GainControl,
    targets: &MonitorTargets,
    stop: &AtomicBool,
) {
    let origin = SessionOrigin::new(0, 48_000, 48_000)
        .unwrap_or_else(|| SessionOrigin::new(0, 1, 48_000).expect("meter origin"));
    while !stop.load(Ordering::Acquire) {
        lease.wait_until_idle(stop);
        if stop.load(Ordering::Acquire) {
            break;
        }
        if !lease.try_monitor() {
            continue;
        }
        let (mic_id, system_id, generation) = targets.snapshot();
        let mut mic = mic_port
            .open(
                &mic_id,
                origin,
                Arc::new(std::sync::atomic::AtomicI64::new(0)),
            )
            .ok();
        let mut system = system_port
            .open(
                &system_id,
                origin,
                Arc::new(std::sync::atomic::AtomicI64::new(0)),
            )
            .ok();
        if mic.is_none() && system.is_none() {
            lease.end_monitor();
            thread::sleep(Duration::from_millis(200));
            continue;
        }
        while !stop.load(Ordering::Acquire) && !lease.recording() {
            let (_, _, current) = targets.snapshot();
            if current != generation {
                break;
            }
            if let Some(stream) = mic.as_mut() {
                match stream.pull() {
                    Ok(Some(block)) => {
                        levels.publish(peak(&block.samples, gain.slider()), 0.0, 0.0);
                    }
                    Ok(None) => {}
                    Err(_) => {
                        stream.close();
                        mic = None;
                    }
                }
            }
            if let Some(stream) = system.as_mut() {
                match stream.pull() {
                    Ok(Some(block)) => {
                        levels.publish(0.0, peak(&block.samples, SYSTEM_GAIN), 0.0);
                    }
                    Ok(None) => {}
                    Err(_) => {
                        stream.close();
                        system = None;
                    }
                }
            }
            if mic.is_none() && system.is_none() {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        if let Some(stream) = mic.as_mut() {
            stream.close();
        }
        if let Some(stream) = system.as_mut() {
            stream.close();
        }
        lease.end_monitor();
    }
}

fn peak(samples: &[f32], gain: f32) -> f32 {
    let mut loudest = 0.0_f32;
    for sample in samples {
        let shaped = sample * gain;
        if shaped.is_finite() {
            loudest = loudest.max(shaped.abs());
        }
    }
    loudest
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    use super::{spawn_level_monitor, MonitorTargets};
    use crate::domain::audio::AudioBlock;
    use crate::domain::audio::SessionOrigin;
    use crate::infrastructure::audio::levels::{GainControl, LevelSnapshot};
    use crate::services::lease::EndpointLease;
    use crate::services::recording::{CaptureError, CapturePort, CaptureStream};

    struct CountingPort {
        live: Arc<AtomicUsize>,
        opened: Arc<AtomicUsize>,
    }

    struct CountingStream {
        live: Arc<AtomicUsize>,
        stop: Arc<AtomicBool>,
        closed: bool,
    }

    impl CapturePort for CountingPort {
        fn open(
            &mut self,
            _: &str,
            _: SessionOrigin,
            _: Arc<std::sync::atomic::AtomicI64>,
        ) -> Result<Box<dyn CaptureStream>, CaptureError> {
            self.opened.fetch_add(1, Ordering::AcqRel);
            self.live.fetch_add(1, Ordering::AcqRel);
            Ok(Box::new(CountingStream {
                live: Arc::clone(&self.live),
                stop: Arc::new(AtomicBool::new(false)),
                closed: false,
            }))
        }
    }

    impl CaptureStream for CountingStream {
        fn pull(&mut self) -> Result<Option<AudioBlock>, CaptureError> {
            thread::sleep(Duration::from_millis(5));
            Ok(None)
        }

        fn close(&mut self) {
            if self.closed {
                return;
            }
            self.closed = true;
            self.stop.store(true, Ordering::Release);
            self.live.fetch_sub(1, Ordering::AcqRel);
        }

        fn overruns(&self) -> u64 {
            0
        }
    }

    impl Drop for CountingStream {
        fn drop(&mut self) {
            self.close();
        }
    }

    #[test]
    fn recording_waits_until_the_monitor_releases_both_endpoints() {
        // Arrange
        let live = Arc::new(AtomicUsize::new(0));
        let opened = Arc::new(AtomicUsize::new(0));
        let mic = CountingPort {
            live: Arc::clone(&live),
            opened: Arc::clone(&opened),
        };
        let system = CountingPort {
            live: Arc::clone(&live),
            opened: Arc::clone(&opened),
        };
        let lease = Arc::new(EndpointLease::new());
        let stop = Arc::new(AtomicBool::new(false));
        let handle = spawn_level_monitor(
            Box::new(mic),
            Box::new(system),
            Arc::clone(&lease),
            Arc::new(LevelSnapshot::new()),
            Arc::new(GainControl::new(1.0, false)),
            Arc::new(MonitorTargets::new()),
            Arc::clone(&stop),
        );
        let started = std::time::Instant::now();
        while opened.load(Ordering::Acquire) < 2 && started.elapsed() < Duration::from_secs(2) {
            thread::sleep(Duration::from_millis(10));
        }

        // Act
        let waiting = Arc::clone(&lease);
        let acquired = thread::spawn(move || {
            waiting.acquire_recording();
        });
        acquired.join().expect("acquire");
        let during = live.load(Ordering::Acquire);
        lease.release_recording();
        let resumed = std::time::Instant::now();
        let mut again = opened.load(Ordering::Acquire);
        while again < 4 && resumed.elapsed() < Duration::from_secs(2) {
            thread::sleep(Duration::from_millis(10));
            again = opened.load(Ordering::Acquire);
        }
        stop.store(true, Ordering::Release);
        lease.release_recording();
        let _ = handle.join();

        // Assert
        assert!(opened.load(Ordering::Acquire) >= 2);
        assert_eq!(during, 0);
        assert!(again >= 4);
        assert_eq!(live.load(Ordering::Acquire), 0);
    }

    #[test]
    fn monitor_does_not_open_while_a_recording_holds_the_lease() {
        // Arrange
        let live = Arc::new(AtomicUsize::new(0));
        let opened = Arc::new(AtomicUsize::new(0));
        let lease = Arc::new(EndpointLease::new());
        lease.acquire_recording();
        let stop = Arc::new(AtomicBool::new(false));
        let handle = spawn_level_monitor(
            Box::new(CountingPort {
                live: Arc::clone(&live),
                opened: Arc::clone(&opened),
            }),
            Box::new(CountingPort {
                live: Arc::clone(&live),
                opened,
            }),
            Arc::clone(&lease),
            Arc::new(LevelSnapshot::new()),
            Arc::new(GainControl::new(1.0, false)),
            Arc::new(MonitorTargets::new()),
            Arc::clone(&stop),
        );

        // Act
        thread::sleep(Duration::from_millis(80));
        let while_held = live.load(Ordering::Acquire);
        stop.store(true, Ordering::Release);
        lease.release_recording();
        let _ = handle.join();

        // Assert
        assert_eq!(while_held, 0);
        assert_eq!(live.load(Ordering::Acquire), 0);
    }
}
