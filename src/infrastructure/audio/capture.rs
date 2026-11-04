//! COM apartment helpers and QPC sampling shared by audio workers.
//!
//! Capture itself goes through flexaudio ([`super::flex_capture`]). Device
//! watch still needs a COM apartment for `IMMNotificationClient`.

use windows::core::HRESULT;
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};

use crate::services::recording::CaptureError;

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

pub(crate) fn qpc_now() -> Result<(u64, u64), CaptureError> {
    let mut counter = 0_i64;
    let mut frequency = 0_i64;
    unsafe { QueryPerformanceCounter(&mut counter) }.map_err(|_| CaptureError::Failed)?;
    unsafe { QueryPerformanceFrequency(&mut frequency) }.map_err(|_| CaptureError::Failed)?;
    if frequency <= 0 {
        return Err(CaptureError::Failed);
    }
    Ok((counter as u64, frequency as u64))
}
