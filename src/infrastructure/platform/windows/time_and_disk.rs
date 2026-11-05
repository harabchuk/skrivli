//! Wall clock, QPC, free space, and COM apartment helpers.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use windows::core::{HRESULT, PCWSTR};
use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::SystemInformation::GetLocalTime;

use crate::common::error::AppError;
use crate::domain::audio::CivilTime;
use crate::services::ports::{CaptureError, FreeSpace, PerformanceCounter, RecordingClock};

pub(crate) struct SystemClock;

impl RecordingClock for SystemClock {
    fn unix_millis(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
            .unwrap_or(0)
    }

    fn local(&self) -> CivilTime {
        let time = unsafe { GetLocalTime() };
        CivilTime::new(
            i32::from(time.wYear),
            time.wMonth as u8,
            time.wDay as u8,
            time.wHour as u8,
            time.wMinute as u8,
            time.wSecond as u8,
        )
        .unwrap_or(CivilTime {
            year: 1970,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
        })
    }
}

pub(crate) struct QpcCounter;

impl PerformanceCounter for QpcCounter {
    fn sample(&self) -> (u64, u64) {
        qpc_now().unwrap_or((0, 0))
    }
}

pub(crate) struct WindowsDisk;

impl FreeSpace for WindowsDisk {
    fn available_bytes(&self, directory: &Path) -> Result<u64, AppError> {
        let target = existing_ancestor(directory);
        let wide = wide_path(&target);
        let mut free = 0_u64;
        unsafe {
            GetDiskFreeSpaceExW(PCWSTR(wide.as_ptr()), Some(&mut free), None, None)
                .map_err(|error| AppError::message("free space", error.to_string()))?;
        }
        Ok(free)
    }
}

pub(crate) struct ComApartment {
    uninit: bool,
}

impl ComApartment {
    pub(crate) fn multithreaded() -> Result<Self, AppError> {
        // SAFETY: this thread owns the apartment. S_FALSE means it was already
        // initialized, so this guard must not call CoUninitialize.
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if hr.0 == 0 {
            Ok(Self { uninit: true })
        } else if hr.0 == 1 || hr.0 == RPC_E_CHANGED_MODE.0 {
            Ok(Self { uninit: false })
        } else {
            Err(AppError::message("microphone", hr.0.to_string()))
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

fn existing_ancestor(path: &Path) -> std::path::PathBuf {
    let mut current = path.to_path_buf();
    while !current.as_os_str().is_empty() && !current.exists() {
        if !current.pop() {
            break;
        }
    }
    current
}

fn wide_path(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}
