//! Endpoint enumeration through MMDevice.
//!
//! The notification callback only enqueues a wake-up. It does not open a
//! stream or touch the UI. Real enumeration is not called from unit tests.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use windows::core::PCWSTR;
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Media::Audio::{
    eCapture, eCommunications, eConsole, eRender, IMMDevice, IMMDeviceEnumerator,
    IMMNotificationClient, IMMNotificationClient_Impl, MMDeviceEnumerator, DEVICE_STATE,
    DEVICE_STATE_ACTIVE, DEVICE_STATE_DISABLED, DEVICE_STATE_NOTPRESENT, DEVICE_STATE_UNPLUGGED,
};
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_ALL, STGM_READ};

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
    emit(enumerate(&enumerator));
    while !stop.load(Ordering::Acquire) {
        match recv_wake(&rx) {
            Wake::Event => emit(enumerate(&enumerator)),
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

fn enumerate(enumerator: &IMMDeviceEnumerator) -> DeviceCatalog {
    let capture = list_flow(enumerator, eCapture, EndpointFlow::Capture);
    let render = list_flow(enumerator, eRender, EndpointFlow::Render);
    let default_capture = default_id(enumerator, eCapture, eCommunications);
    let default_render = default_id(enumerator, eRender, eConsole);
    DeviceCatalog {
        capture,
        render,
        default_capture,
        default_render,
    }
}

fn list_flow(
    enumerator: &IMMDeviceEnumerator,
    flow: windows::Win32::Media::Audio::EDataFlow,
    kind: EndpointFlow,
) -> Vec<EndpointDescriptor> {
    let mask = DEVICE_STATE(
        DEVICE_STATE_ACTIVE.0
            | DEVICE_STATE_DISABLED.0
            | DEVICE_STATE_UNPLUGGED.0
            | DEVICE_STATE_NOTPRESENT.0,
    );
    let collection = match unsafe { enumerator.EnumAudioEndpoints(flow, mask) } {
        Ok(collection) => collection,
        Err(error) => {
            tracing::warn!(error = %error, "endpoints were not enumerated");
            return Vec::new();
        }
    };
    let count = match unsafe { collection.GetCount() } {
        Ok(count) => count,
        Err(error) => {
            tracing::warn!(error = %error, "endpoint count was not read");
            return Vec::new();
        }
    };
    let mut devices = Vec::new();
    for index in 0..count {
        let Ok(device) = (unsafe { collection.Item(index) }) else {
            continue;
        };
        if let Some(descriptor) = describe(&device, kind) {
            devices.push(descriptor);
        }
    }
    devices
}

fn describe(device: &IMMDevice, flow: EndpointFlow) -> Option<EndpointDescriptor> {
    let id = device_id(device)?;
    let state = unsafe { device.GetState() }.unwrap_or(DEVICE_STATE_DISABLED);
    let availability = if state_contains(state, DEVICE_STATE_ACTIVE) {
        EndpointAvailability::Active
    } else if state_contains(state, DEVICE_STATE_NOTPRESENT) {
        EndpointAvailability::Absent
    } else {
        EndpointAvailability::Disabled
    };
    let name = friendly_name(device).unwrap_or_else(|| match flow {
        EndpointFlow::Capture => "Microphone".to_string(),
        EndpointFlow::Render => "Output".to_string(),
    });
    Some(EndpointDescriptor::new(id, name, flow, availability))
}

fn default_id(
    enumerator: &IMMDeviceEnumerator,
    flow: windows::Win32::Media::Audio::EDataFlow,
    role: windows::Win32::Media::Audio::ERole,
) -> Option<EndpointId> {
    let device = unsafe { enumerator.GetDefaultAudioEndpoint(flow, role) }.ok()?;
    device_id(&device)
}

fn device_id(device: &IMMDevice) -> Option<EndpointId> {
    let raw = unsafe { device.GetId() }.ok()?;
    let text = take_pwstr(raw);
    EndpointId::new(text)
}

fn friendly_name(device: &IMMDevice) -> Option<String> {
    let store = unsafe { device.OpenPropertyStore(STGM_READ) }.ok()?;
    let value = unsafe { store.GetValue(&PKEY_Device_FriendlyName) }.ok()?;
    let name = value.to_string();
    if name.trim().is_empty() {
        None
    } else {
        Some(name)
    }
}

fn state_contains(state: DEVICE_STATE, flag: DEVICE_STATE) -> bool {
    state.0 & flag.0 != 0
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

    use super::enqueue_device_event;

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
}
