//! `IMMNotificationClient` wake for device hot-plug.
//!
//! Re-enumeration always goes through `flexaudio` via
//! [`crate::infrastructure::audio::devices::enumerate_catalog`].

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use windows::core::PCWSTR;
use windows::Win32::Media::Audio::{
    IMMDeviceEnumerator, IMMNotificationClient, IMMNotificationClient_Impl, MMDeviceEnumerator,
    DEVICE_STATE,
};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};

use crate::common::error::AppError;
use crate::infrastructure::audio::devices::{
    enqueue_device_event, enumerate_catalog, DeviceCatalog,
};

use super::time_and_disk::ComApartment;

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
