//! Playback use-case host. Commands are non-blocking for the UI.
//!
//! One worker thread owns the [`PlaybackSession`]. Commands flow one way; the
//! worker publishes the latest [`PlayerSnapshot`] (state, not events), including
//! the position while playing and `Idle` when a recording ends.

use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::domain::ids::RecordingId;
use crate::services::ports::{PlaybackOutput, PlaybackSession};

/// How often the worker checks for the end of playback and a new position.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PlayerPhase {
    Idle,
    Playing,
    Paused,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PlayerSnapshot {
    pub(crate) phase: PlayerPhase,
    pub(crate) recording_id: Option<RecordingId>,
    pub(crate) notice: Option<&'static str>,
    /// Position in the recording. Zero when `Idle` or `Error`.
    pub(crate) position_ms: u64,
    /// Length of the recording, when the decoder knows it.
    pub(crate) duration_ms: Option<u64>,
}

impl PlayerSnapshot {
    pub(crate) fn idle() -> Self {
        Self {
            phase: PlayerPhase::Idle,
            recording_id: None,
            notice: None,
            position_ms: 0,
            duration_ms: None,
        }
    }
}

enum PlayerCommand {
    Play {
        id: RecordingId,
        path: PathBuf,
        start_ms: u64,
    },
    Pause,
    Resume,
    Seek {
        position_ms: u64,
    },
    Stop,
    Shutdown,
}

/// Owns a dedicated player worker.
#[derive(Clone)]
pub(crate) struct PlayerHost {
    tx: Sender<PlayerCommand>,
    /// Mirrored for tests via [`Self::snapshot`]; UI uses bridge events.
    #[allow(dead_code)]
    latest: Arc<Mutex<PlayerSnapshot>>,
}

impl PlayerHost {
    pub(crate) fn spawn(
        output: Box<dyn PlaybackOutput>,
        on_change: Box<dyn Fn(PlayerSnapshot) + Send>,
    ) -> (Self, JoinHandle<()>) {
        let (tx, rx) = mpsc::channel();
        let latest = Arc::new(Mutex::new(PlayerSnapshot::idle()));
        let worker_latest = Arc::clone(&latest);
        let handle = thread::spawn(move || {
            player_loop(rx, output.as_ref(), &worker_latest, on_change.as_ref());
        });
        (Self { tx, latest }, handle)
    }

    pub(crate) fn play(&self, id: RecordingId, path: PathBuf, start_ms: u64) {
        let _ = self.tx.send(PlayerCommand::Play { id, path, start_ms });
    }

    pub(crate) fn pause(&self) {
        let _ = self.tx.send(PlayerCommand::Pause);
    }

    pub(crate) fn resume(&self) {
        let _ = self.tx.send(PlayerCommand::Resume);
    }

    pub(crate) fn seek(&self, position_ms: u64) {
        let _ = self.tx.send(PlayerCommand::Seek { position_ms });
    }

    pub(crate) fn stop(&self) {
        let _ = self.tx.send(PlayerCommand::Stop);
    }

    pub(crate) fn shutdown(&self) {
        let _ = self.tx.send(PlayerCommand::Shutdown);
    }

    #[cfg(test)]
    pub(crate) fn snapshot(&self) -> PlayerSnapshot {
        self.latest
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

struct Active {
    id: RecordingId,
    session: Box<dyn PlaybackSession>,
    paused: bool,
    duration_ms: Option<u64>,
    published_position_ms: u64,
}

impl Active {
    fn snapshot(&self, position_ms: u64) -> PlayerSnapshot {
        PlayerSnapshot {
            phase: if self.paused {
                PlayerPhase::Paused
            } else {
                PlayerPhase::Playing
            },
            recording_id: Some(self.id),
            notice: None,
            position_ms,
            duration_ms: self.duration_ms,
        }
    }
}

struct Publisher<'a> {
    latest: &'a Mutex<PlayerSnapshot>,
    on_change: &'a dyn Fn(PlayerSnapshot),
}

impl Publisher<'_> {
    fn publish(&self, snapshot: PlayerSnapshot) {
        if let Ok(mut guard) = self.latest.lock() {
            *guard = snapshot.clone();
        }
        (self.on_change)(snapshot);
    }

    /// Publish the active session's state and remember the position.
    fn publish_active(&self, active: &mut Active, position_ms: u64) {
        active.published_position_ms = position_ms;
        self.publish(active.snapshot(position_ms));
    }
}

fn player_loop(
    rx: mpsc::Receiver<PlayerCommand>,
    output: &dyn PlaybackOutput,
    latest: &Mutex<PlayerSnapshot>,
    on_change: &dyn Fn(PlayerSnapshot),
) {
    let publisher = Publisher { latest, on_change };
    let mut active: Option<Active> = None;
    loop {
        // An idle worker sleeps until a command arrives; a live session is polled.
        let received = if active.is_some() {
            match rx.recv_timeout(POLL_INTERVAL) {
                Ok(command) => Some(command),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match rx.recv() {
                Ok(command) => Some(command),
                Err(_) => break,
            }
        };
        match received {
            Some(PlayerCommand::Shutdown) => {
                drop(active.take());
                publisher.publish(PlayerSnapshot::idle());
                break;
            }
            Some(PlayerCommand::Stop) => {
                drop(active.take());
                publisher.publish(PlayerSnapshot::idle());
            }
            Some(PlayerCommand::Play { id, path, start_ms }) => {
                drop(active.take());
                match output.open(&path, start_ms) {
                    Ok(session) => {
                        let duration_ms = session.duration_ms();
                        let start_ms = duration_ms.map_or(start_ms, |end| start_ms.min(end));
                        let mut next = Active {
                            id,
                            session,
                            paused: false,
                            duration_ms,
                            published_position_ms: start_ms,
                        };
                        publisher.publish_active(&mut next, start_ms);
                        active = Some(next);
                    }
                    Err(error) => {
                        tracing::error!(error = %error, "playback failed to start");
                        publisher.publish(PlayerSnapshot {
                            phase: PlayerPhase::Error,
                            notice: Some(error.user_message()),
                            ..PlayerSnapshot::idle()
                        });
                    }
                }
            }
            Some(PlayerCommand::Pause) => set_paused(&publisher, &mut active, true),
            Some(PlayerCommand::Resume) => set_paused(&publisher, &mut active, false),
            Some(PlayerCommand::Seek { position_ms }) => {
                if let Some(current) = active.as_mut() {
                    let target = current
                        .duration_ms
                        .map_or(position_ms, |end| position_ms.min(end));
                    match current.session.seek(target) {
                        Ok(()) => publisher.publish_active(current, target),
                        Err(error) => {
                            tracing::warn!(error = %error, "playback seek failed");
                        }
                    }
                }
            }
            None => {}
        }
        tick(&publisher, &mut active);
    }
}

fn set_paused(publisher: &Publisher<'_>, active: &mut Option<Active>, paused: bool) {
    if let Some(current) = active.as_mut() {
        if current.paused != paused {
            current.session.set_paused(paused);
            current.paused = paused;
            let position_ms = current.session.position_ms();
            publisher.publish_active(current, position_ms);
        }
    }
}

/// Report the end of playback, or a changed position while playing.
fn tick(publisher: &Publisher<'_>, active: &mut Option<Active>) {
    let Some(current) = active.as_mut() else {
        return;
    };
    if current.session.finished() {
        drop(active.take());
        publisher.publish(PlayerSnapshot::idle());
        return;
    }
    if !current.paused {
        let position_ms = current.session.position_ms();
        if position_ms != current.published_position_ms {
            publisher.publish_active(current, position_ms);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{PlayerHost, PlayerPhase, PlayerSnapshot};
    use crate::common::error::AppError;
    use crate::domain::ids::RecordingId;
    use crate::services::ports::{PlaybackOutput, PlaybackSession};
    use std::path::{Path, PathBuf};
    use std::sync::mpsc::{self, Receiver};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    #[derive(Default)]
    struct FakeState {
        position_ms: u64,
        duration_ms: Option<u64>,
        finished: bool,
        paused: bool,
        seeks: Vec<u64>,
        opens: Vec<(PathBuf, u64)>,
        fail_next_open: bool,
    }

    #[derive(Clone, Default)]
    struct FakeOutput(Arc<Mutex<FakeState>>);

    impl FakeOutput {
        fn with_duration(duration_ms: u64) -> Self {
            let fake = Self::default();
            fake.0.lock().expect("lock").duration_ms = Some(duration_ms);
            fake
        }

        fn update(&self, change: impl FnOnce(&mut FakeState)) {
            change(&mut self.0.lock().expect("lock"));
        }

        fn read<T>(&self, read: impl FnOnce(&FakeState) -> T) -> T {
            read(&self.0.lock().expect("lock"))
        }
    }

    struct FakeSession(Arc<Mutex<FakeState>>);

    impl PlaybackOutput for FakeOutput {
        fn open(&self, path: &Path, start_ms: u64) -> Result<Box<dyn PlaybackSession>, AppError> {
            let mut state = self.0.lock().expect("lock");
            if state.fail_next_open {
                state.fail_next_open = false;
                return Err(AppError::message("playback", "no output device"));
            }
            state.opens.push((path.to_path_buf(), start_ms));
            state.position_ms = start_ms;
            state.finished = false;
            state.paused = false;
            Ok(Box::new(FakeSession(Arc::clone(&self.0))))
        }
    }

    impl PlaybackSession for FakeSession {
        fn position_ms(&self) -> u64 {
            self.0.lock().expect("lock").position_ms
        }

        fn duration_ms(&self) -> Option<u64> {
            self.0.lock().expect("lock").duration_ms
        }

        fn finished(&self) -> bool {
            self.0.lock().expect("lock").finished
        }

        fn set_paused(&self, paused: bool) {
            self.0.lock().expect("lock").paused = paused;
        }

        fn seek(&self, position_ms: u64) -> Result<(), AppError> {
            let mut state = self.0.lock().expect("lock");
            state.seeks.push(position_ms);
            state.position_ms = position_ms;
            Ok(())
        }
    }

    fn spawn_host(output: &FakeOutput) -> (PlayerHost, std::thread::JoinHandle<()>, Collector) {
        let (tx, rx) = mpsc::channel();
        let (host, handle) = PlayerHost::spawn(
            Box::new(output.clone()),
            Box::new(move |snapshot| {
                let _ = tx.send(snapshot);
            }),
        );
        (
            host,
            handle,
            Collector {
                rx,
                seen: Vec::new(),
            },
        )
    }

    struct Collector {
        rx: Receiver<PlayerSnapshot>,
        seen: Vec<PlayerSnapshot>,
    }

    impl Collector {
        /// Wait until a published snapshot satisfies `matches`.
        fn wait_for(&mut self, matches: impl Fn(&PlayerSnapshot) -> bool) -> PlayerSnapshot {
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                let left = deadline.saturating_duration_since(Instant::now());
                match self.rx.recv_timeout(left) {
                    Ok(snapshot) => {
                        self.seen.push(snapshot.clone());
                        if matches(&snapshot) {
                            return snapshot;
                        }
                    }
                    Err(_) => panic!("no matching snapshot; saw {:?}", self.seen),
                }
            }
        }
    }

    fn play(host: &PlayerHost, start_ms: u64) {
        host.play(RecordingId::from_raw(1), PathBuf::from("a.wav"), start_ms);
    }

    fn finish(host: &PlayerHost, handle: std::thread::JoinHandle<()>) {
        host.shutdown();
        handle.join().expect("join");
    }

    #[test]
    fn finished_session_publishes_idle_without_commands() {
        // Arrange
        let output = FakeOutput::with_duration(5_000);
        let (host, handle, mut seen) = spawn_host(&output);
        play(&host, 0);
        seen.wait_for(|s| s.phase == PlayerPhase::Playing);

        // Act
        output.update(|state| state.finished = true);
        let snapshot = seen.wait_for(|s| s.phase == PlayerPhase::Idle);

        // Assert
        assert_eq!(snapshot, PlayerSnapshot::idle());
        finish(&host, handle);
    }

    #[test]
    fn pause_then_resume_publishes_paused_then_playing() {
        // Arrange
        let output = FakeOutput::with_duration(5_000);
        let (host, handle, mut seen) = spawn_host(&output);
        play(&host, 0);
        seen.wait_for(|s| s.phase == PlayerPhase::Playing);

        // Act
        host.pause();
        let paused = seen.wait_for(|s| s.phase == PlayerPhase::Paused);
        let paused_flag = output.read(|state| state.paused);
        host.resume();
        let resumed = seen.wait_for(|s| s.phase == PlayerPhase::Playing);

        // Assert
        assert_eq!(paused.recording_id, Some(RecordingId::from_raw(1)));
        assert!(paused_flag);
        assert_eq!(resumed.phase, PlayerPhase::Playing);
        assert!(!output.read(|state| state.paused));
        finish(&host, handle);
    }

    #[test]
    fn seek_forwards_position_and_clamps_beyond_the_end() {
        // Arrange
        let output = FakeOutput::with_duration(5_000);
        let (host, handle, mut seen) = spawn_host(&output);
        play(&host, 0);
        seen.wait_for(|s| s.phase == PlayerPhase::Playing);

        // Act
        host.seek(2_000);
        let inside = seen.wait_for(|s| s.position_ms == 2_000);
        host.seek(99_000);
        let clamped = seen.wait_for(|s| s.position_ms == 5_000);

        // Assert
        assert_eq!(inside.duration_ms, Some(5_000));
        assert_eq!(clamped.phase, PlayerPhase::Playing);
        assert_eq!(output.read(|state| state.seeks.clone()), vec![2_000, 5_000]);
        finish(&host, handle);
    }

    #[test]
    fn seek_while_paused_stays_paused() {
        // Arrange
        let output = FakeOutput::with_duration(5_000);
        let (host, handle, mut seen) = spawn_host(&output);
        play(&host, 0);
        host.pause();
        seen.wait_for(|s| s.phase == PlayerPhase::Paused);

        // Act
        host.seek(1_500);
        let snapshot = seen.wait_for(|s| s.position_ms == 1_500);

        // Assert
        assert_eq!(snapshot.phase, PlayerPhase::Paused);
        assert!(output.read(|state| state.paused));
        finish(&host, handle);
    }

    #[test]
    fn play_starts_at_the_requested_position() {
        // Arrange
        let output = FakeOutput::with_duration(60_000);
        let (host, handle, mut seen) = spawn_host(&output);

        // Act
        play(&host, 12_000);
        let snapshot = seen.wait_for(|s| s.phase == PlayerPhase::Playing);

        // Assert
        assert_eq!(snapshot.position_ms, 12_000);
        assert_eq!(output.read(|state| state.opens[0].1), 12_000);
        finish(&host, handle);
    }

    #[test]
    fn position_publishes_while_playing_and_not_while_paused() {
        // Arrange
        let output = FakeOutput::with_duration(60_000);
        let (host, handle, mut seen) = spawn_host(&output);
        play(&host, 0);
        seen.wait_for(|s| s.phase == PlayerPhase::Playing);

        // Act
        output.update(|state| state.position_ms = 700);
        let advanced = seen.wait_for(|s| s.position_ms == 700);
        host.pause();
        seen.wait_for(|s| s.phase == PlayerPhase::Paused);
        output.update(|state| state.position_ms = 900);
        std::thread::sleep(Duration::from_millis(350));
        host.resume();
        let resumed = seen.wait_for(|s| s.phase == PlayerPhase::Playing);

        // Assert
        assert_eq!(advanced.phase, PlayerPhase::Playing);
        assert!(!seen
            .seen
            .iter()
            .any(|s| s.position_ms == 900 && s.phase == PlayerPhase::Paused));
        assert_eq!(resumed.position_ms, 900);
        finish(&host, handle);
    }

    #[test]
    fn open_failure_publishes_error_and_next_play_works() {
        // Arrange
        let output = FakeOutput::with_duration(5_000);
        output.update(|state| state.fail_next_open = true);
        let (host, handle, mut seen) = spawn_host(&output);

        // Act
        play(&host, 0);
        let failed = seen.wait_for(|s| s.phase == PlayerPhase::Error);
        play(&host, 0);
        let recovered = seen.wait_for(|s| s.phase == PlayerPhase::Playing);

        // Assert
        assert_eq!(failed.notice, Some("Could not play the recording."));
        assert_eq!(recovered.recording_id, Some(RecordingId::from_raw(1)));
        finish(&host, handle);
    }

    #[test]
    fn commands_are_handled_while_a_session_is_playing() {
        // Arrange
        let output = FakeOutput::with_duration(60_000);
        let (host, handle, mut seen) = spawn_host(&output);
        play(&host, 0);
        seen.wait_for(|s| s.phase == PlayerPhase::Playing);

        // Act
        host.stop();
        let snapshot = seen.wait_for(|s| s.phase == PlayerPhase::Idle);

        // Assert
        assert_eq!(snapshot, PlayerSnapshot::idle());
        finish(&host, handle);
        assert_eq!(host.snapshot(), PlayerSnapshot::idle());
    }
}
