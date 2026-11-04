//! Shared-mode render loopback on its own COM-owning thread.
//!
//! WASAPI often delivers no packets while the endpoint is silent. That is a
//! normal idle, not a stopped recording. This milestone does not open a
//! silent-render keep-alive: that stream stays deferred until a real output
//! shows it is inaudible and does not change power behavior. The mixer still
//! fills timestamp gaps, including a long silent stretch.
//!
//! Packets use the same bounded reject-new queue as the microphone (D-04).

#[cfg(test)]
use std::collections::VecDeque;
use std::sync::atomic::AtomicI64;
#[cfg(test)]
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
#[cfg(test)]
use std::sync::Mutex;
#[cfg(test)]
use std::thread::{self, JoinHandle};
#[cfg(test)]
use std::time::Duration;

use windows::Win32::Media::Audio::{
    eConsole, eRender, AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_LOOPBACK,
};

use crate::domain::audio::SessionOrigin;
use crate::services::recording::{CaptureError, CapturePort, CaptureStream};

use super::capture::{run_endpoint, spawn_audio_worker};
#[cfg(test)]
use super::queue::AudioBlock;

/// Loopback flag together with the shared event-driven buffer.
pub(crate) const LOOPBACK_STREAM_FLAGS: u32 =
    AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK;

pub(crate) struct WasapiLoopback;

impl CapturePort for WasapiLoopback {
    fn open(
        &mut self,
        endpoint_id: &str,
        origin: SessionOrigin,
        drift_error: Arc<AtomicI64>,
    ) -> Result<Box<dyn CaptureStream>, CaptureError> {
        let endpoint = endpoint_id.to_string();
        spawn_audio_worker(move |tx, stop, overruns, ready| {
            if let Err(error) = run_endpoint(
                &endpoint,
                origin,
                eRender,
                eConsole,
                LOOPBACK_STREAM_FLAGS,
                &tx,
                &stop,
                &overruns,
                &ready,
                drift_error,
            ) {
                let _ = ready.send(Err(error));
            }
        })
    }
}

/// In-memory loopback. Tests use this instead of a render endpoint.
/// `close` joins the stand-in thread, the same way a WASAPI worker is joined.
#[cfg(test)]
#[derive(Clone)]
struct FakeLoopback {
    inner: Arc<Mutex<FakeLoopState>>,
    live: Arc<AtomicUsize>,
}

#[cfg(test)]
struct FakeLoopState {
    error: Option<CaptureError>,
    blocks: VecDeque<AudioBlock>,
}

#[cfg(test)]
struct FakeLoopStream {
    blocks: VecDeque<AudioBlock>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    overruns: u64,
}

#[cfg(test)]
impl FakeLoopback {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(FakeLoopState {
                error: None,
                blocks: VecDeque::new(),
            })),
            live: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub(crate) fn push(&self, block: AudioBlock) {
        let mut state = self
            .inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        state.blocks.push_back(block);
    }

    pub(crate) fn live(&self) -> usize {
        self.live.load(Ordering::Acquire)
    }
}

#[cfg(test)]
impl CapturePort for FakeLoopback {
    fn open(
        &mut self,
        _endpoint_id: &str,
        _origin: SessionOrigin,
        _drift_error: Arc<AtomicI64>,
    ) -> Result<Box<dyn CaptureStream>, CaptureError> {
        let mut state = self
            .inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if let Some(error) = state.error.take() {
            return Err(error);
        }
        let blocks = std::mem::take(&mut state.blocks);
        drop(state);
        let stop = Arc::new(AtomicBool::new(false));
        let live = Arc::clone(&self.live);
        live.fetch_add(1, Ordering::AcqRel);
        let stop_thread = Arc::clone(&stop);
        let worker = thread::spawn(move || {
            while !stop_thread.load(Ordering::Acquire) {
                thread::sleep(Duration::from_millis(5));
            }
            live.fetch_sub(1, Ordering::AcqRel);
        });
        Ok(Box::new(FakeLoopStream {
            blocks,
            stop,
            worker: Some(worker),
            overruns: 0,
        }))
    }
}

#[cfg(test)]
impl CaptureStream for FakeLoopStream {
    fn pull(&mut self) -> Result<Option<AudioBlock>, CaptureError> {
        Ok(self.blocks.pop_front())
    }

    fn close(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }

    fn overruns(&self) -> u64 {
        self.overruns
    }
}

#[cfg(test)]
impl Drop for FakeLoopStream {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::{FakeLoopback, LOOPBACK_STREAM_FLAGS};
    use crate::domain::audio::SessionOrigin;
    use crate::infrastructure::audio::queue::{AudioBlock, CAPTURE_QUEUE_CAPACITY};
    use crate::services::recording::CapturePort;
    use std::sync::atomic::AtomicI64;
    use std::sync::Arc;

    #[test]
    fn fake_loopback_delivers_a_block_without_wasapi() {
        // Arrange
        let mut port = FakeLoopback::new();
        port.push(AudioBlock {
            samples: vec![0.2, 0.2],
            timeline_frame: Some(0),
            discontinuity: false,
        });
        let origin = SessionOrigin::new(0, 48_000, 48_000).expect("origin");

        // Act
        let mut stream = port
            .open("", origin, Arc::new(AtomicI64::new(0)))
            .expect("open");
        let block = stream.pull().expect("pull").expect("block");
        let live_while_open = port.live();
        stream.close();

        // Assert
        assert_eq!(block.samples, vec![0.2, 0.2]);
        assert_eq!(live_while_open, 1);
        assert_eq!(port.live(), 0);
        let _ = (LOOPBACK_STREAM_FLAGS, CAPTURE_QUEUE_CAPACITY);
    }
}
