//! Fake worker that proves a background thread can publish a structured event.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::views::actions::BridgeMessage;

pub(crate) const SHELL_PULSE_INTERVAL: Duration = Duration::from_secs(2);

pub(crate) fn spawn_shell_producer(
    tx: Sender<BridgeMessage>,
    stop: std::sync::Arc<AtomicBool>,
    interval: Duration,
) -> JoinHandle<()> {
    thread::spawn(move || run_shell_producer(&tx, &stop, interval))
}

fn run_shell_producer(tx: &Sender<BridgeMessage>, stop: &AtomicBool, interval: Duration) {
    let _ = tx.send(BridgeMessage::ShellPulse);
    while !stop.load(Ordering::Acquire) {
        if wait_for_stop(stop, interval) {
            break;
        }
        let _ = tx.send(BridgeMessage::ShellPulse);
    }
}

fn wait_for_stop(stop: &AtomicBool, interval: Duration) -> bool {
    let slice = Duration::from_millis(50);
    let mut waited = Duration::ZERO;
    while waited < interval {
        if stop.load(Ordering::Acquire) {
            return true;
        }
        thread::sleep(slice);
        waited = waited.saturating_add(slice);
    }
    stop.load(Ordering::Acquire)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;
    use std::sync::{mpsc, Arc};
    use std::time::Duration;

    use super::spawn_shell_producer;
    use crate::views::actions::BridgeMessage;

    #[test]
    fn producer_emits_a_pulse_then_stops() {
        // Arrange
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let handle = spawn_shell_producer(tx, Arc::clone(&stop), Duration::from_secs(30));

        // Act
        let first = rx.recv_timeout(Duration::from_secs(2));
        stop.store(true, std::sync::atomic::Ordering::Release);
        let joined = handle.join();

        // Assert
        assert!(matches!(first, Ok(BridgeMessage::ShellPulse)));
        assert!(joined.is_ok());
    }
}
