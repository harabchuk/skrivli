//! Endpoint catalog from flexaudio, with a COM wake for hot-plug.
//!
//! Device ids come from [`flexaudio::devices`] (cpal names for microphones,
//! FriendlyName / native id for render). `IMMNotificationClient` only enqueues
//! a wake; re-enumeration always goes through flexaudio so capture selection
//! ids stay aligned. flexaudio's own watcher is a no-op on Windows.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use flexaudio::{DeviceInfo, SourceKind};
use windows::core::PCWSTR;
use windows::Win32::Media::Audio::{
    IMMDeviceEnumerator, IMMNotificationClient, IMMNotificationClient_Impl, MMDeviceEnumerator,
    DEVICE_STATE,
};
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_ALL};

use crate::common::error::AppError;
use crate::domain::audio::{EndpointAvailability, EndpointDescriptor, EndpointFlow, EndpointId};

use super::capture::ComApartment;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DeviceCatalog {
    pub(crate) capture: Vec<EndpointDescriptor>,
    pub(crate) render: Vec<EndpointDescriptor>,
    pub(crate) default_capture: Option<EndpointId>,
    pub(crate) default_render: Option<EndpointId>,
}

impl DeviceCatalog {
    pub(crate) fn empty() -> Self {
        Self {
            capture: Vec::new(),
            render: Vec::new(),
            default_capture: None,
            default_render: None,
        }
    }
}

/// Reject a second wake-up when one is already queued. Never blocks.
pub(crate) fn enqueue_device_event(tx: &SyncSender<()>) {
    let _ = tx.try_send(());
}

pub(crate) fn spawn_device_watch(
    emit: Box<dyn Fn(DeviceCatalog) + Send>,
    stop: Arc<AtomicBool>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        if let Err(error) = watch(emit, &stop) {
            tracing::error!(error = %error, "endpoint watch stopped");
        }
    })
}

fn watch(emit: Box<dyn Fn(DeviceCatalog) + Send>, stop: &AtomicBool) -> Result<(), AppError> {
    let _apartment = ComApartment::multithreaded()?;
    let enumerator = enumerator()?;
    let (tx, rx) = sync_channel(1);
    let callback: IMMNotificationClient = EndpointEvents { tx: tx.clone() }.into();
    unsafe { enumerator.RegisterEndpointNotificationCallback(&callback) }
        .map_err(|error| AppError::message("microphone", error.to_string()))?;
    emit(enumerate_catalog());
    while !stop.load(Ordering::Acquire) {
        match recv_wake(&rx) {
            Wake::Event => emit(enumerate_catalog()),
            Wake::Timeout => {}
            Wake::Closed => break,
        }
    }
    let _ = unsafe { enumerator.UnregisterEndpointNotificationCallback(&callback) };
    Ok(())
}

enum Wake {
    Event,
    Timeout,
    Closed,
}

fn recv_wake(rx: &Receiver<()>) -> Wake {
    match rx.recv_timeout(Duration::from_millis(200)) {
        Ok(()) => Wake::Event,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Wake::Timeout,
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Wake::Closed,
    }
}

fn enumerator() -> Result<IMMDeviceEnumerator, AppError> {
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
        .map_err(|error| AppError::message("microphone", error.to_string()))
}

/// Build a catalog from flexaudio device ids (same ids `CapturePort` opens).
pub(crate) fn enumerate_catalog() -> DeviceCatalog {
    match flexaudio::devices() {
        Ok(devices) => catalog_from_devices(&devices),
        Err(error) => {
            tracing::warn!(error = %error, "flexaudio devices were not enumerated");
            DeviceCatalog::empty()
        }
    }
}

pub(crate) fn catalog_from_devices(devices: &[DeviceInfo]) -> DeviceCatalog {
    let mut capture = Vec::new();
    let mut render = Vec::new();
    let mut default_capture = None;
    let mut default_render = None;
    for device in devices {
        let Some(descriptor) = describe(device) else {
            continue;
        };
        match descriptor.flow() {
            EndpointFlow::Capture => {
                if device.is_default && default_capture.is_none() {
                    default_capture = Some(descriptor.id().clone());
                }
                capture.push(descriptor);
            }
            EndpointFlow::Render => {
                if device.is_default && default_render.is_none() {
                    default_render = Some(descriptor.id().clone());
                }
                render.push(descriptor);
            }
        }
    }
    DeviceCatalog {
        capture,
        render,
        default_capture,
        default_render,
    }
}

fn describe(device: &DeviceInfo) -> Option<EndpointDescriptor> {
    let flow = if device.is_loopback {
        EndpointFlow::Render
    } else if device.source_kind == SourceKind::Mic {
        EndpointFlow::Capture
    } else {
        return None;
    };
    let id = EndpointId::new(device.id.clone())?;
    let name = if device.name.trim().is_empty() {
        match flow {
            EndpointFlow::Capture => "Microphone".to_string(),
            EndpointFlow::Render => "Output".to_string(),
        }
    } else {
        device.name.clone()
    };
    // flexaudio only lists selectable endpoints; treat them as active.
    Some(EndpointDescriptor::new(
        id,
        name,
        flow,
        EndpointAvailability::Active,
    ))
}

pub(crate) fn take_pwstr(value: windows::core::PWSTR) -> String {
    let text = if value.is_null() {
        String::new()
    } else {
        unsafe { value.to_string() }.unwrap_or_default()
    };
    if !value.is_null() {
        unsafe { CoTaskMemFree(Some(value.0.cast())) };
    }
    text
}

#[windows::core::implement(IMMNotificationClient)]
struct EndpointEvents {
    tx: SyncSender<()>,
}

impl IMMNotificationClient_Impl for EndpointEvents_Impl {
    fn OnDeviceStateChanged(
        &self,
        _device_id: &PCWSTR,
        _new_state: DEVICE_STATE,
    ) -> windows::core::Result<()> {
        enqueue_device_event(&self.this.tx);
        Ok(())
    }

    fn OnDeviceAdded(&self, _device_id: &PCWSTR) -> windows::core::Result<()> {
        enqueue_device_event(&self.this.tx);
        Ok(())
    }

    fn OnDeviceRemoved(&self, _device_id: &PCWSTR) -> windows::core::Result<()> {
        enqueue_device_event(&self.this.tx);
        Ok(())
    }

    fn OnDefaultDeviceChanged(
        &self,
        _flow: windows::Win32::Media::Audio::EDataFlow,
        _role: windows::Win32::Media::Audio::ERole,
        _default_device_id: &PCWSTR,
    ) -> windows::core::Result<()> {
        enqueue_device_event(&self.this.tx);
        Ok(())
    }

    fn OnPropertyValueChanged(
        &self,
        _device_id: &PCWSTR,
        _key: &windows::Win32::UI::Shell::PropertiesSystem::PROPERTYKEY,
    ) -> windows::core::Result<()> {
        enqueue_device_event(&self.this.tx);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::sync_channel;
    use std::time::Instant;

    use flexaudio::{DeviceInfo, SourceKind};

    use super::{catalog_from_devices, enqueue_device_event};
    use crate::domain::audio::EndpointFlow;

    #[test]
    fn notification_enqueue_does_not_block_when_full() {
        // Arrange
        let (tx, rx) = sync_channel(1);
        enqueue_device_event(&tx);

        // Act
        let started = Instant::now();
        enqueue_device_event(&tx);
        let elapsed = started.elapsed();

        // Assert
        assert!(elapsed.as_millis() < 200);
        assert!(rx.try_recv().is_ok());
        assert!(matches!(
            rx.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));
    }

    #[test]
    fn catalog_splits_mic_and_loopback_and_keeps_defaults() {
        // Arrange
        let devices = vec![
            DeviceInfo {
                id: "Mic A".into(),
                name: "Mic A".into(),
                source_kind: SourceKind::Mic,
                sample_rate: 48_000,
                channels: 1,
                format_available: true,
                is_loopback: false,
                is_default: true,
            },
            DeviceInfo {
                id: "Speakers".into(),
                name: "Speakers".into(),
                source_kind: SourceKind::SystemLoopback,
                sample_rate: 48_000,
                channels: 2,
                format_available: true,
                is_loopback: true,
                is_default: true,
            },
            DeviceInfo {
                id: "Headset".into(),
                name: "Headset".into(),
                source_kind: SourceKind::Mic,
                sample_rate: 44_100,
                channels: 1,
                format_available: true,
                is_loopback: false,
                is_default: false,
            },
        ];

        // Act
        let catalog = catalog_from_devices(&devices);

        // Assert
        assert_eq!(catalog.capture.len(), 2);
        assert_eq!(catalog.render.len(), 1);
        assert_eq!(
            catalog.default_capture.as_ref().map(|id| id.as_str()),
            Some("Mic A")
        );
        assert_eq!(
            catalog.default_render.as_ref().map(|id| id.as_str()),
            Some("Speakers")
        );
        assert_eq!(catalog.capture[0].flow(), EndpointFlow::Capture);
        assert_eq!(catalog.render[0].flow(), EndpointFlow::Render);
    }
}
