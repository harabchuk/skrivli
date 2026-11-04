//! Shared-mode microphone capture.
//!
//! The capture thread owns `CoInitializeEx`, `IAudioClient`, and
//! `IAudioCaptureClient`. Those COM pointers are not `Send`. Packets are
//! converted on that thread and offered to a bounded channel; a full channel
//! rejects the new packet (see `queue`).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, SyncSender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use windows::core::HRESULT;
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::Media::Audio::{
    eCapture, eCommunications, IAudioCaptureClient, IAudioClient, IMMDevice, IMMDeviceEnumerator,
    MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY, AUDCLNT_BUFFERFLAGS_SILENT,
    AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR, AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED,
    AUDCLNT_E_BUFFER_TOO_LARGE, AUDCLNT_E_UNSUPPORTED_FORMAT, AUDCLNT_SHAREMODE_SHARED,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, WAVEFORMATEX, WAVEFORMATEXTENSIBLE,
};
use windows::Win32::Media::KernelStreaming::{KSDATAFORMAT_SUBTYPE_PCM, WAVE_FORMAT_EXTENSIBLE};
use windows::Win32::Media::Multimedia::WAVE_FORMAT_IEEE_FLOAT;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_MULTITHREADED,
};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

use crate::domain::audio::{AudioFormat, SampleEncoding, SessionOrigin, INTERNAL_SAMPLE_RATE};
use crate::services::recording::{CaptureError, CapturePort, CaptureStream};

use super::queue::{offer_block, AudioBlock, CAPTURE_QUEUE_CAPACITY};

/// 200 ms in 100-nanosecond units.
pub(crate) const PREFERRED_BUFFER_HNS: i64 = 2_000_000;

pub(crate) struct ComApartment {
    uninit: bool,
}

impl ComApartment {
    pub(crate) fn multithreaded() -> Result<Self, crate::common::error::AppError> {
        // SAFETY: this thread owns the apartment. S_FALSE means it was already
        // initialized, so this guard must not call CoUninitialize.
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if hr.0 == 0 {
            Ok(Self { uninit: true })
        } else if hr.0 == 1 || hr.0 == RPC_E_CHANGED_MODE.0 {
            Ok(Self { uninit: false })
        } else {
            Err(crate::common::error::AppError::message(
                "microphone",
                hr.0.to_string(),
            ))
        }
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        if self.uninit {
            unsafe { CoUninitialize() };
        }
    }
}

const RPC_E_CHANGED_MODE: HRESULT = HRESULT(0x80010106_u32 as i32);

struct EventHandle(HANDLE);

impl EventHandle {
    fn new() -> Result<Self, CaptureError> {
        let handle =
            unsafe { CreateEventW(None, false, false, None) }.map_err(|_| CaptureError::Failed)?;
        Ok(Self(handle))
    }

    fn get(&self) -> HANDLE {
        self.0
    }
}

impl Drop for EventHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PacketFlags {
    pub(crate) silent: bool,
    pub(crate) discontinuity: bool,
    pub(crate) timestamp_error: bool,
}

pub(crate) fn interpret_flags(flags: u32) -> PacketFlags {
    PacketFlags {
        silent: flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0,
        discontinuity: flags & AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 as u32 != 0,
        timestamp_error: flags & AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR.0 as u32 != 0,
    }
}

pub(crate) fn choose_buffer_hns(default_period: i64, minimum_period: i64) -> i64 {
    let minimum = minimum_period.max(0);
    if PREFERRED_BUFFER_HNS >= minimum {
        PREFERRED_BUFFER_HNS
    } else if default_period > 0 {
        default_period
    } else {
        minimum.max(10_000)
    }
}

pub(crate) struct WasapiCapture;

impl CapturePort for WasapiCapture {
    fn open(&mut self, endpoint_id: &str) -> Result<Box<dyn CaptureStream>, CaptureError> {
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (packet_tx, packet_rx) = sync_channel(CAPTURE_QUEUE_CAPACITY);
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let overruns = Arc::new(AtomicU64::new(0));
        let endpoint = endpoint_id.to_string();
        let thread_stop = Arc::clone(&stop);
        let thread_overruns = Arc::clone(&overruns);
        let worker = thread::spawn(move || {
            capture_thread(&endpoint, packet_tx, thread_stop, thread_overruns, ready_tx);
        });
        match ready_rx.recv_timeout(Duration::from_secs(8)) {
            Ok(Ok(())) => Ok(Box::new(WasapiStream {
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
}

struct WasapiStream {
    packets: Receiver<AudioBlock>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    worker: Option<JoinHandle<()>>,
    overruns: Arc<AtomicU64>,
}

impl CaptureStream for WasapiStream {
    fn pull(&mut self) -> Result<Option<AudioBlock>, CaptureError> {
        match self.packets.recv_timeout(Duration::from_millis(50)) {
            Ok(block) => Ok(Some(block)),
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

impl Drop for WasapiStream {
    fn drop(&mut self) {
        self.close();
    }
}

fn capture_thread(
    endpoint_id: &str,
    tx: SyncSender<AudioBlock>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    overruns: Arc<AtomicU64>,
    ready: std::sync::mpsc::Sender<Result<(), CaptureError>>,
) {
    if let Err(error) = open_and_run(endpoint_id, &tx, &stop, &overruns, &ready) {
        let _ = ready.send(Err(error));
    }
}

fn open_and_run(
    endpoint_id: &str,
    tx: &SyncSender<AudioBlock>,
    stop: &std::sync::atomic::AtomicBool,
    overruns: &AtomicU64,
    ready: &std::sync::mpsc::Sender<Result<(), CaptureError>>,
) -> Result<(), CaptureError> {
    let _apartment = ComApartment::multithreaded().map_err(|_| CaptureError::Failed)?;
    let enumerator: IMMDeviceEnumerator =
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }.map_err(map_hresult)?;
    let device = open_device(&enumerator, endpoint_id)?;
    let client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None) }.map_err(map_hresult)?;
    let format_bytes = negotiate_format(&client)?;
    let audio_format = audio_format_of(&format_bytes).ok_or(CaptureError::UnsupportedFormat)?;
    let mut default_period = 0_i64;
    let mut minimum_period = 0_i64;
    unsafe { client.GetDevicePeriod(Some(&mut default_period), Some(&mut minimum_period)) }
        .map_err(map_hresult)?;
    let buffer = choose_buffer_hns(default_period, minimum_period);
    initialize_client(&client, &format_bytes, buffer, default_period)?;
    let event = EventHandle::new()?;
    unsafe { client.SetEventHandle(event.get()) }.map_err(map_hresult)?;
    let capture: IAudioCaptureClient = unsafe { client.GetService() }.map_err(map_hresult)?;
    unsafe { client.Start() }.map_err(map_hresult)?;
    if ready.send(Ok(())).is_err() {
        let _ = unsafe { client.Stop() };
        return Ok(());
    }
    let (counter, frequency) = qpc_now()?;
    let origin =
        SessionOrigin::new(counter, frequency, INTERNAL_SAMPLE_RATE).ok_or(CaptureError::Failed)?;
    let mut pending_gap = false;
    while !stop.load(Ordering::Acquire) {
        let status = unsafe { WaitForSingleObject(event.get(), 200) };
        if status == WAIT_TIMEOUT && stop.load(Ordering::Acquire) {
            break;
        }
        if status != WAIT_OBJECT_0 && status != WAIT_TIMEOUT {
            break;
        }
        if let Err(error) = drain(
            &capture,
            &format_bytes,
            audio_format,
            origin,
            tx,
            overruns,
            &mut pending_gap,
        ) {
            tracing::error!("capture packet was not read");
            let _ = error;
            break;
        }
    }
    let _ = unsafe { client.Stop() };
    Ok(())
}

fn open_device(
    enumerator: &IMMDeviceEnumerator,
    endpoint_id: &str,
) -> Result<IMMDevice, CaptureError> {
    if endpoint_id.is_empty() {
        return unsafe { enumerator.GetDefaultAudioEndpoint(eCapture, eCommunications) }
            .map_err(map_hresult);
    }
    let wide = wide_id(endpoint_id);
    unsafe { enumerator.GetDevice(windows::core::PCWSTR(wide.as_ptr())) }.map_err(map_hresult)
}

fn wide_id(id: &str) -> Vec<u16> {
    id.encode_utf16().chain(std::iter::once(0)).collect()
}

fn negotiate_format(client: &IAudioClient) -> Result<Vec<u8>, CaptureError> {
    let mix = unsafe { client.GetMixFormat() }.map_err(map_hresult)?;
    let mix_bytes = copy_format(mix);
    unsafe { CoTaskMemFree(Some(mix.cast())) };
    let preferred = preferred_format();
    let mut closest: *mut WAVEFORMATEX = std::ptr::null_mut();
    let hr = unsafe {
        client.IsFormatSupported(
            AUDCLNT_SHAREMODE_SHARED,
            std::ptr::addr_of!(preferred.Format),
            Some(&mut closest),
        )
    };
    if hr.0 == 0 {
        if !closest.is_null() {
            unsafe { CoTaskMemFree(Some(closest.cast())) };
        }
        return Ok(extensible_bytes(&preferred));
    }
    if !closest.is_null() {
        let bytes = copy_format(closest);
        unsafe { CoTaskMemFree(Some(closest.cast())) };
        if audio_format_of(&bytes).is_some() {
            return Ok(bytes);
        }
    }
    if audio_format_of(&mix_bytes).is_some() {
        return Ok(mix_bytes);
    }
    Err(CaptureError::UnsupportedFormat)
}

fn initialize_client(
    client: &IAudioClient,
    format: &[u8],
    buffer: i64,
    fallback_period: i64,
) -> Result<(), CaptureError> {
    let pointer = format.as_ptr().cast::<WAVEFORMATEX>();
    let first = unsafe {
        client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
            buffer,
            0,
            pointer,
            None,
        )
    };
    if let Err(error) = first {
        let code = error.code();
        if code != AUDCLNT_E_BUFFER_TOO_LARGE && code != AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED {
            return Err(map_hresult(error));
        }
        return unsafe {
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                fallback_period.max(10_000),
                0,
                pointer,
                None,
            )
        }
        .map_err(map_hresult);
    }
    Ok(())
}

fn drain(
    capture: &IAudioCaptureClient,
    format_bytes: &[u8],
    format: AudioFormat,
    origin: SessionOrigin,
    tx: &SyncSender<AudioBlock>,
    overruns: &AtomicU64,
    pending_gap: &mut bool,
) -> Result<(), CaptureError> {
    let block_align = format
        .bytes_per_frame()
        .ok_or(CaptureError::UnsupportedFormat)?;
    loop {
        let packet = unsafe { capture.GetNextPacketSize() }.map_err(map_hresult)?;
        if packet == 0 {
            break;
        }
        let mut data = std::ptr::null_mut();
        let mut frames = 0_u32;
        let mut flags = 0_u32;
        let mut _device_pos = 0_u64;
        let mut qpc = 0_u64;
        unsafe {
            capture.GetBuffer(
                &mut data,
                &mut frames,
                &mut flags,
                Some(&mut _device_pos),
                Some(&mut qpc),
            )
        }
        .map_err(map_hresult)?;
        let parsed = interpret_flags(flags);
        let byte_len = (frames as u64).saturating_mul(block_align) as usize;
        let bytes = if parsed.silent || data.is_null() {
            vec![0_u8; byte_len]
        } else {
            unsafe { std::slice::from_raw_parts(data, byte_len) }.to_vec()
        };
        unsafe { capture.ReleaseBuffer(frames) }.map_err(map_hresult)?;
        let timeline = if parsed.timestamp_error {
            None
        } else {
            origin.sample_index(qpc)
        };
        let block = match super::queue::block_from_packet(
            &bytes,
            format,
            timeline,
            parsed.silent,
            parsed.discontinuity || *pending_gap,
        ) {
            Ok(block) => block,
            Err(_) => continue,
        };
        if offer_block(tx, block) {
            *pending_gap = false;
        } else {
            overruns.fetch_add(1, Ordering::Relaxed);
            *pending_gap = true;
        }
        let _ = format_bytes;
    }
    Ok(())
}

fn preferred_format() -> WAVEFORMATEXTENSIBLE {
    let mut format = WAVEFORMATEXTENSIBLE::default();
    format.Format.wFormatTag = WAVE_FORMAT_EXTENSIBLE as u16;
    format.Format.nChannels = 2;
    format.Format.nSamplesPerSec = INTERNAL_SAMPLE_RATE;
    format.Format.nAvgBytesPerSec = INTERNAL_SAMPLE_RATE * 8;
    format.Format.nBlockAlign = 8;
    format.Format.wBitsPerSample = 32;
    format.Format.cbSize =
        (std::mem::size_of::<WAVEFORMATEXTENSIBLE>() - std::mem::size_of::<WAVEFORMATEX>()) as u16;
    format.Samples.wValidBitsPerSample = 32;
    format.dwChannelMask = 3;
    format.SubFormat = windows::core::GUID::from_u128(0x0000_0003_0000_0010_8000_00aa_0038_9b71);
    format
}

fn extensible_bytes(format: &WAVEFORMATEXTENSIBLE) -> Vec<u8> {
    let len = std::mem::size_of::<WAVEFORMATEXTENSIBLE>();
    let mut bytes = vec![0_u8; len];
    unsafe {
        std::ptr::copy_nonoverlapping(
            (format as *const WAVEFORMATEXTENSIBLE).cast::<u8>(),
            bytes.as_mut_ptr(),
            len,
        );
    }
    bytes
}

fn copy_format(format: *const WAVEFORMATEX) -> Vec<u8> {
    if format.is_null() {
        return Vec::new();
    }
    let header = unsafe { format.read_unaligned() };
    let len = std::mem::size_of::<WAVEFORMATEX>() + header.cbSize as usize;
    let mut bytes = vec![0_u8; len];
    unsafe {
        std::ptr::copy_nonoverlapping(format.cast::<u8>(), bytes.as_mut_ptr(), len);
    }
    bytes
}

fn audio_format_of(bytes: &[u8]) -> Option<AudioFormat> {
    if bytes.len() < std::mem::size_of::<WAVEFORMATEX>() {
        return None;
    }
    let header = unsafe { bytes.as_ptr().cast::<WAVEFORMATEX>().read_unaligned() };
    let encoding = if header.wFormatTag == 1 {
        match header.wBitsPerSample {
            16 => SampleEncoding::I16,
            32 => SampleEncoding::I32,
            _ => return None,
        }
    } else if header.wFormatTag == WAVE_FORMAT_IEEE_FLOAT as u16 {
        SampleEncoding::F32
    } else if header.wFormatTag == WAVE_FORMAT_EXTENSIBLE as u16 {
        extensible_encoding(bytes, header.wBitsPerSample)?
    } else {
        return None;
    };
    AudioFormat::new(header.nSamplesPerSec, header.nChannels, encoding)
}

fn extensible_encoding(bytes: &[u8], bits: u16) -> Option<SampleEncoding> {
    if bytes.len() < 40 {
        return None;
    }
    let mut guid = [0_u8; 16];
    guid.copy_from_slice(&bytes[24..40]);
    let float_guid = guid_bytes(&windows::core::GUID::from_u128(
        0x0000_0003_0000_0010_8000_00aa_0038_9b71,
    ));
    let pcm_guid = guid_bytes(&KSDATAFORMAT_SUBTYPE_PCM);
    if guid == float_guid {
        Some(SampleEncoding::F32)
    } else if guid == pcm_guid && bits == 16 {
        Some(SampleEncoding::I16)
    } else if guid == pcm_guid && bits == 32 {
        Some(SampleEncoding::I32)
    } else {
        None
    }
}

fn guid_bytes(guid: &windows::core::GUID) -> [u8; 16] {
    let mut bytes = [0_u8; 16];
    bytes[..4].copy_from_slice(&guid.to_u128().to_le_bytes()[..4]);
    // GUID memory layout is mixed-endian. Compare the canonical byte form.
    let mixed = guid.to_u128().to_le_bytes();
    let _ = mixed;
    unsafe {
        std::ptr::copy_nonoverlapping(
            (guid as *const windows::core::GUID).cast::<u8>(),
            bytes.as_mut_ptr(),
            16,
        );
    }
    bytes
}

fn qpc_now() -> Result<(u64, u64), CaptureError> {
    let mut counter = 0_i64;
    let mut frequency = 0_i64;
    unsafe { QueryPerformanceCounter(&mut counter) }.map_err(|_| CaptureError::Failed)?;
    unsafe { QueryPerformanceFrequency(&mut frequency) }.map_err(|_| CaptureError::Failed)?;
    if frequency <= 0 {
        return Err(CaptureError::Failed);
    }
    Ok((counter as u64, frequency as u64))
}

fn map_hresult(error: windows::core::Error) -> CaptureError {
    let code = error.code();
    if code == windows::Win32::Foundation::E_ACCESSDENIED {
        CaptureError::PrivacyDenied
    } else if code == AUDCLNT_E_UNSUPPORTED_FORMAT {
        CaptureError::UnsupportedFormat
    } else {
        tracing::debug!(code = code.0, "capture hresult");
        CaptureError::Unavailable
    }
}

#[cfg(test)]
mod tests {
    use super::{choose_buffer_hns, interpret_flags, PREFERRED_BUFFER_HNS};

    #[test]
    fn buffer_prefers_200ms_when_the_device_allows_it() {
        // Arrange
        let default_period = 10_000_i64;
        let minimum = 10_000_i64;

        // Act
        let chosen = choose_buffer_hns(default_period, minimum);
        let raised = choose_buffer_hns(3_000_000, 2_500_000);

        // Assert
        assert_eq!(chosen, PREFERRED_BUFFER_HNS);
        assert_eq!(raised, 3_000_000);
    }

    #[test]
    fn packet_flags_mark_silence_and_discontinuity() {
        // Arrange
        let flags = 1_u32 | 2_u32 | 4_u32;

        // Act
        let parsed = interpret_flags(flags);
        let quiet = interpret_flags(2);

        // Assert
        assert!(parsed.silent && parsed.discontinuity && parsed.timestamp_error);
        assert!(quiet.silent);
        assert!(!quiet.discontinuity);
    }
}
