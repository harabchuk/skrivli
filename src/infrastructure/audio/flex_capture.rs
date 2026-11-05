//! flexaudio CapturePort adapters for microphone and system loopback.
//!
//! Each `open` starts a worker that owns one flexaudio [`Stream`]. Chunks are
//! already 48 kHz stereo `f32`. This module maps them onto the session timeline,
//! applies the shared drift resampler, and offers blocks on the bounded queue.

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, SyncSender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use flexaudio::{
    open_with_options, AudioChunk, ChunkFlags, Error, ErrorKind, Event, OpenOptions, OutputFormat,
    SourceKind, Stream, StreamConfig,
};

use crate::domain::audio::{AudioBlock, SessionOrigin, INTERNAL_CHANNELS, INTERNAL_SAMPLE_RATE};
use crate::services::ports::{CaptureError, CapturePort, CaptureStream, PerformanceCounter};

use super::queue::CAPTURE_QUEUE_CAPACITY;
use super::resampler::StereoResampler;

/// Microphone capture through flexaudio (`SourceKind::Mic`).
pub(crate) struct FlexMicCapture {
    clock: Arc<dyn PerformanceCounter>,
}

/// System loopback through flexaudio (`SourceKind::SystemLoopback`) with classic
/// WASAPI silent-render keepalive enabled (flexaudio default).
pub(crate) struct FlexSystemCapture {
    clock: Arc<dyn PerformanceCounter>,
}

impl FlexMicCapture {
    pub(crate) fn new(clock: Arc<dyn PerformanceCounter>) -> Self {
        Self { clock }
    }
}

impl FlexSystemCapture {
    pub(crate) fn new(clock: Arc<dyn PerformanceCounter>) -> Self {
        Self { clock }
    }
}

impl CapturePort for FlexMicCapture {
    fn open(
        &mut self,
        endpoint_id: &str,
        origin: SessionOrigin,
        drift_error: Arc<AtomicI64>,
    ) -> Result<Box<dyn CaptureStream>, CaptureError> {
        open_flex_port(
            SourceKind::Mic,
            endpoint_id,
            origin,
            drift_error,
            Arc::clone(&self.clock),
        )
    }
}

impl CapturePort for FlexSystemCapture {
    fn open(
        &mut self,
        endpoint_id: &str,
        origin: SessionOrigin,
        drift_error: Arc<AtomicI64>,
    ) -> Result<Box<dyn CaptureStream>, CaptureError> {
        open_flex_port(
            SourceKind::SystemLoopback,
            endpoint_id,
            origin,
            drift_error,
            Arc::clone(&self.clock),
        )
    }
}

fn open_flex_port(
    kind: SourceKind,
    endpoint_id: &str,
    origin: SessionOrigin,
    drift_error: Arc<AtomicI64>,
    clock: Arc<dyn PerformanceCounter>,
) -> Result<Box<dyn CaptureStream>, CaptureError> {
    let device_id = if endpoint_id.is_empty() {
        None
    } else {
        Some(endpoint_id.to_string())
    };
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let (packet_tx, packet_rx) = sync_channel(CAPTURE_QUEUE_CAPACITY);
    let stop = Arc::new(AtomicBool::new(false));
    let overruns = Arc::new(AtomicU64::new(0));
    let thread_stop = Arc::clone(&stop);
    let thread_overruns = Arc::clone(&overruns);
    let worker = thread::spawn(move || {
        run_flex_worker(
            kind,
            device_id,
            origin,
            drift_error,
            packet_tx,
            thread_stop,
            thread_overruns,
            ready_tx,
            clock,
        );
    });
    match ready_rx.recv_timeout(Duration::from_secs(8)) {
        Ok(Ok(())) => Ok(Box::new(FlexCaptureStream {
            packets: packet_rx,
            stop,
            worker: Some(worker),
            overruns,
        })),
        Ok(Err(error)) => {
            let _ = worker.join();
            Err(error)
        }
        Err(_) => {
            stop.store(true, Ordering::Release);
            let _ = worker.join();
            Err(CaptureError::Unavailable)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_flex_worker(
    kind: SourceKind,
    device_id: Option<String>,
    origin: SessionOrigin,
    drift_error: Arc<AtomicI64>,
    tx: SyncSender<Result<AudioBlock, CaptureError>>,
    stop: Arc<AtomicBool>,
    overruns: Arc<AtomicU64>,
    ready: std::sync::mpsc::Sender<Result<(), CaptureError>>,
    clock: Arc<dyn PerformanceCounter>,
) {
    let config = StreamConfig {
        kind,
        device_id,
        output: OutputFormat {
            sample_rate: INTERNAL_SAMPLE_RATE,
            channels: INTERNAL_CHANNELS,
        },
        ..StreamConfig::default()
    };
    let mut stream = match open_with_options(config, OpenOptions::new()) {
        Ok(stream) => stream,
        Err(error) => {
            let _ = ready.send(Err(map_flex_error(&error)));
            return;
        }
    };
    if let Err(error) = stream.start() {
        let _ = ready.send(Err(map_flex_error(&error)));
        stream.stop();
        return;
    }
    if ready.send(Ok(())).is_err() {
        stream.stop();
        return;
    }

    let mut resampler = StereoResampler::new(INTERNAL_SAMPLE_RATE, INTERNAL_SAMPLE_RATE);
    let mut session_base: Option<u64> = None;
    let mut pending_gap = false;

    while !stop.load(Ordering::Acquire) {
        if let Some(error) = drain_events(&mut stream) {
            let _ = offer_result(&tx, Err(error));
            break;
        }
        match stream.poll_chunk() {
            Some(chunk) => {
                let drops = u64::from(chunk.dropped_before);
                if drops > 0 {
                    overruns.fetch_add(drops, Ordering::Relaxed);
                    pending_gap = true;
                }
                let timeline =
                    map_timeline(origin, clock.as_ref(), &mut session_base, chunk.frame_index);
                let discontinuity = chunk_discontinuity(&chunk) || pending_gap;
                resampler.set_drift_error(drift_error.load(Ordering::Relaxed));
                let samples = resampler.process(&chunk.data);
                if samples.is_empty() {
                    continue;
                }
                let block = AudioBlock {
                    samples,
                    timeline_frame: timeline,
                    discontinuity,
                };
                // Queue carries Result so fatal errors can share the channel.
                if offer_result(&tx, Ok(block)) {
                    pending_gap = false;
                } else if stop.load(Ordering::Acquire) {
                    break;
                } else {
                    overruns.fetch_add(1, Ordering::Relaxed);
                    pending_gap = true;
                }
            }
            None => {
                thread::sleep(Duration::from_millis(5));
            }
        }
    }

    let flushed = resampler.flush();
    if !flushed.is_empty() {
        let _ = offer_result(
            &tx,
            Ok(AudioBlock {
                samples: flushed,
                timeline_frame: None,
                discontinuity: false,
            }),
        );
    }
    stream.stop();
}

fn offer_result(
    tx: &SyncSender<Result<AudioBlock, CaptureError>>,
    packet: Result<AudioBlock, CaptureError>,
) -> bool {
    match tx.try_send(packet) {
        Ok(()) => true,
        Err(_) => false,
    }
}

fn drain_events(stream: &mut Stream) -> Option<CaptureError> {
    while let Some(event) = stream.poll_event() {
        match event {
            Event::TerminalError { error } => return Some(map_flex_error(&error)),
            Event::PermissionDenied { .. } => return Some(CaptureError::PrivacyDenied),
            Event::DeviceLost => return Some(CaptureError::Unavailable),
            Event::Error(_) => return Some(CaptureError::Failed),
            _ => {}
        }
    }
    None
}

/// Map a flexaudio chunk onto the shared session timeline.
pub(crate) fn map_timeline(
    origin: SessionOrigin,
    clock: &dyn PerformanceCounter,
    session_base: &mut Option<u64>,
    frame_index: u64,
) -> Option<u64> {
    let base = match *session_base {
        Some(base) => base,
        None => {
            let (ticks, _) = clock.sample();
            let now = origin.sample_index(ticks).unwrap_or(0);
            let base = now.saturating_sub(frame_index);
            *session_base = Some(base);
            base
        }
    };
    Some(base.saturating_add(frame_index))
}

pub(crate) fn chunk_discontinuity(chunk: &AudioChunk) -> bool {
    chunk.flags.contains(ChunkFlags::DISCONTINUITY)
        || chunk.flags.contains(ChunkFlags::RECOVERED)
        || chunk.dropped_before > 0
}

pub(crate) fn map_flex_error(error: &Error) -> CaptureError {
    match error.kind() {
        ErrorKind::PermissionDenied => CaptureError::PrivacyDenied,
        ErrorKind::UnsupportedFormat | ErrorKind::NativeFormatChanged => {
            CaptureError::UnsupportedFormat
        }
        ErrorKind::DeviceNotFound
        | ErrorKind::DeviceLost
        | ErrorKind::Unsupported
        | ErrorKind::UnsupportedOsVersion
        | ErrorKind::AmbiguousDeviceName => CaptureError::Unavailable,
        ErrorKind::InvalidArg
        | ErrorKind::InvalidState
        | ErrorKind::Backend
        | ErrorKind::IgnoredSettings => CaptureError::Failed,
        _ => CaptureError::Failed,
    }
}

struct FlexCaptureStream {
    packets: Receiver<Result<AudioBlock, CaptureError>>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    overruns: Arc<AtomicU64>,
}

impl CaptureStream for FlexCaptureStream {
    fn pull(&mut self) -> Result<Option<AudioBlock>, CaptureError> {
        match self.packets.recv_timeout(Duration::from_millis(50)) {
            Ok(Ok(block)) => Ok(Some(block)),
            Ok(Err(error)) => Err(error),
            Err(RecvTimeoutError::Timeout) | Err(RecvTimeoutError::Disconnected) => Ok(None),
        }
    }

    fn close(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }

    fn overruns(&self) -> u64 {
        self.overruns.load(Ordering::Acquire)
    }
}

impl Drop for FlexCaptureStream {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::{chunk_discontinuity, map_flex_error, map_timeline};
    use crate::domain::audio::{AudioBlock, SessionOrigin, INTERNAL_SAMPLE_RATE};
    use crate::infrastructure::audio::resampler::StereoResampler;
    use crate::services::ports::{CaptureError, PerformanceCounter};
    use flexaudio::{AudioChunk, ChunkFlags, Error, ErrorKind, Permission};

    fn block_from_chunk(
        chunk: &AudioChunk,
        timeline_frame: Option<u64>,
        pending_gap: bool,
        resampler: &mut StereoResampler,
    ) -> AudioBlock {
        let discontinuity = chunk_discontinuity(chunk) || pending_gap;
        let samples = resampler.process(&chunk.data);
        AudioBlock {
            samples,
            timeline_frame,
            discontinuity,
        }
    }

    struct FixedClock {
        ticks: u64,
        frequency: u64,
    }

    impl PerformanceCounter for FixedClock {
        fn sample(&self) -> (u64, u64) {
            (self.ticks, self.frequency)
        }
    }

    fn chunk(frame_index: u64, flags: ChunkFlags, dropped_before: u32) -> AudioChunk {
        AudioChunk {
            data: vec![0.1, -0.1, 0.2, -0.2],
            frames: 2,
            frame_index,
            pts_ns: 0,
            seq: 0,
            flags,
            dropped_before,
            peak: 0.2,
            rms: 0.1,
        }
    }

    #[test]
    fn timeline_anchors_first_chunk_then_advances_with_frame_index() {
        // Arrange
        let origin = SessionOrigin::new(0, 48_000, 48_000).expect("origin");
        let clock = FixedClock {
            ticks: 1_920,
            frequency: 48_000,
        };
        let mut base = None;

        // Act
        let first = map_timeline(origin, &clock, &mut base, 960).expect("first");
        let second = map_timeline(origin, &clock, &mut base, 1_920).expect("second");

        // Assert
        assert_eq!(first, 1_920);
        assert_eq!(second, 2_880);
        assert_eq!(base, Some(960));
    }

    #[test]
    fn discontinuity_includes_flags_and_dropped_chunks() {
        // Arrange
        let clean = chunk(0, ChunkFlags::empty(), 0);
        let flagged = chunk(0, ChunkFlags::DISCONTINUITY, 0);
        let dropped = chunk(0, ChunkFlags::empty(), 2);

        // Act / Assert
        assert!(!chunk_discontinuity(&clean));
        assert!(chunk_discontinuity(&flagged));
        assert!(chunk_discontinuity(&dropped));
    }

    #[test]
    fn block_from_chunk_marks_pending_gap() {
        // Arrange
        let source = chunk(0, ChunkFlags::empty(), 0);
        let mut resampler = StereoResampler::new(INTERNAL_SAMPLE_RATE, INTERNAL_SAMPLE_RATE);

        // Act
        let block = block_from_chunk(&source, Some(100), true, &mut resampler);

        // Assert
        assert!(block.discontinuity);
        assert_eq!(block.timeline_frame, Some(100));
    }

    #[test]
    fn flex_errors_map_to_capture_errors() {
        // Arrange
        let privacy = Error::PermissionDenied {
            permission: Permission::Microphone,
            detail: "denied".into(),
        };
        let missing = Error::DeviceNotFound;
        let format = Error::UnsupportedFormat("bad".into());

        // Act / Assert
        assert_eq!(map_flex_error(&privacy), CaptureError::PrivacyDenied);
        assert_eq!(map_flex_error(&missing), CaptureError::Unavailable);
        assert_eq!(map_flex_error(&format), CaptureError::UnsupportedFormat);
        assert_eq!(privacy.kind(), ErrorKind::PermissionDenied);
    }
}
