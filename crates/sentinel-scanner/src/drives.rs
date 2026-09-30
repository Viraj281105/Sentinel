//! Drive discovery via Win32 volume APIs.
//!
//! Only local volumes (fixed, removable, RAM disk) are queried for capacity. Network and
//! optical drives are listed but not queried, because querying an offline share or an
//! empty optical drive can block for a long time.

use serde::Serialize;
use ts_rs::TS;
use windows::Win32::Foundation::ERROR_NOT_READY;
use windows::Win32::Storage::FileSystem::{
    GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDriveStringsW, GetVolumeInformationW,
};
use windows::Win32::System::Diagnostics::Debug::{
    SEM_FAILCRITICALERRORS, SEM_NOOPENFILEERRORBOX, SetThreadErrorMode, THREAD_ERROR_MODE,
};
use windows::Win32::System::SystemInformation::GetSystemWindowsDirectoryW;
use windows::core::PCWSTR;

/// A drive is considered low on space when less than this share of it is free.
pub const LOW_SPACE_PERCENT: u64 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum DriveKind {
    Fixed,
    Removable,
    Network,
    Optical,
    RamDisk,
    Unknown,
}

impl DriveKind {
    /// Map a `GetDriveTypeW` result.
    pub fn from_win32(code: u32) -> Self {
        match code {
            2 => Self::Removable,
            3 => Self::Fixed,
            4 => Self::Network,
            5 => Self::Optical,
            6 => Self::RamDisk,
            _ => Self::Unknown,
        }
    }

    /// Whether volume information is safe to query without risking a long block.
    fn is_local(self) -> bool {
        matches!(self, Self::Fixed | Self::Removable | Self::RamDisk)
    }
}

/// Capacity figures for a volume. All values in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Space {
    #[ts(type = "number")]
    pub total_bytes: u64,
    #[ts(type = "number")]
    pub used_bytes: u64,
    /// Free space on the volume.
    #[ts(type = "number")]
    pub free_bytes: u64,
    /// Free space usable by the current user (smaller than `free_bytes` under quotas).
    #[ts(type = "number")]
    pub available_bytes: u64,
    /// Less than [`LOW_SPACE_PERCENT`] of the volume is free.
    pub low_space: bool,
}

impl Space {
    pub fn new(total_bytes: u64, free_bytes: u64, available_bytes: u64) -> Self {
        let free_bytes = free_bytes.min(total_bytes);
        Self {
            total_bytes,
            used_bytes: total_bytes - free_bytes,
            free_bytes,
            available_bytes: available_bytes.min(free_bytes),
            low_space: total_bytes > 0
                && u128::from(free_bytes) * 100
                    < u128::from(total_bytes) * u128::from(LOW_SPACE_PERCENT),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "state"
)]
#[ts(export)]
pub enum DriveStatus {
    Ready,
    /// Removable or optical drive with no media inserted.
    NoMedia,
    /// Network or optical drive that Sentinel deliberately did not query.
    NotQueried,
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Drive {
    /// Root path, e.g. `C:\`.
    pub root: String,
    pub kind: DriveKind,
    pub label: Option<String>,
    pub file_system: Option<String>,
    pub space: Option<Space>,
    /// The volume Windows is installed on.
    pub is_system: bool,
    pub status: DriveStatus,
}

#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    #[error("Windows could not list the drives on this computer: {0}")]
    Enumerate(windows::core::Error),
}

/// Suppresses the "There is no disk in the drive" dialog for this thread while alive.
struct NoErrorDialogs(THREAD_ERROR_MODE);

impl NoErrorDialogs {
    fn new() -> Self {
        let mut old = THREAD_ERROR_MODE(0);
        // SAFETY: `old` is a valid out-pointer for the duration of the call.
        let _ = unsafe {
            SetThreadErrorMode(
                SEM_FAILCRITICALERRORS | SEM_NOOPENFILEERRORBOX,
                Some(&mut old),
            )
        };
        Self(old)
    }
}

impl Drop for NoErrorDialogs {
    fn drop(&mut self) {
        // SAFETY: restores the mode saved in `new`; no out-pointer is passed.
        let _ = unsafe { SetThreadErrorMode(self.0, None) };
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// Split a double-NUL-terminated list of NUL-terminated strings.
pub fn split_multi_sz(buf: &[u16]) -> Vec<String> {
    buf.split(|&c| c == 0)
        .take_while(|s| !s.is_empty())
        .map(String::from_utf16_lossy)
        .collect()
}

/// Root of the volume Windows is installed on, e.g. `C:\`.
fn system_root() -> Option<String> {
    let mut buf = [0u16; 260];
    // SAFETY: the buffer is valid and its length is passed via the slice.
    let len = unsafe { GetSystemWindowsDirectoryW(Some(&mut buf)) } as usize;
    if len == 0 || len > buf.len() {
        return None;
    }
    let dir = from_wide(&buf[..len]);
    dir.get(..3).map(str::to_owned)
}

fn logical_drive_roots() -> Result<Vec<String>, DiscoveryError> {
    // 26 letters * "X:\\\0" + final NUL fits in 105; use a generous buffer.
    let mut buf = [0u16; 256];
    // SAFETY: the buffer is valid and its length is passed via the slice.
    let len = unsafe { GetLogicalDriveStringsW(Some(&mut buf)) } as usize;
    if len == 0 || len > buf.len() {
        return Err(DiscoveryError::Enumerate(
            windows::core::Error::from_thread(),
        ));
    }
    Ok(split_multi_sz(&buf[..=len.min(buf.len() - 1)]))
}

/// Discover all drives. Per-drive failures are reported in [`Drive::status`]; only a
/// failure to enumerate drives at all is an error.
pub fn list_drives() -> Result<Vec<Drive>, DiscoveryError> {
    let _guard = NoErrorDialogs::new();
    let system = system_root();
    let drives: Vec<Drive> = logical_drive_roots()?
        .iter()
        .map(|root| query(root, system.as_deref()))
        .collect();
    tracing::debug!(count = drives.len(), "drive discovery finished");
    Ok(drives)
}

/// Query a single drive root such as `C:\`.
pub fn query_drive(root: &str) -> Drive {
    let _guard = NoErrorDialogs::new();
    query(root, system_root().as_deref())
}

fn query(root: &str, system_root: Option<&str>) -> Drive {
    let root_w = wide(root);
    let root_p = PCWSTR(root_w.as_ptr());
    // SAFETY: `root_w` is NUL-terminated and outlives the call.
    let kind = DriveKind::from_win32(unsafe { GetDriveTypeW(root_p) });
    let mut drive = Drive {
        root: root.to_owned(),
        kind,
        label: None,
        file_system: None,
        space: None,
        is_system: system_root.is_some_and(|s| s.eq_ignore_ascii_case(root)),
        status: DriveStatus::NotQueried,
    };
    if !kind.is_local() {
        return drive;
    }

    let mut label = [0u16; 261];
    let mut fs_name = [0u16; 261];
    // SAFETY: buffers are valid for their slice lengths; `root_w` outlives the call.
    let volume = unsafe {
        GetVolumeInformationW(
            root_p,
            Some(&mut label),
            None,
            None,
            None,
            Some(&mut fs_name),
        )
    };
    if let Err(err) = volume {
        drive.status = status_from(&err);
        if !matches!(drive.status, DriveStatus::NoMedia) {
            tracing::warn!(root, error = %err, "volume information unavailable");
        }
        return drive;
    }
    drive.label = Some(from_wide(&label)).filter(|s| !s.is_empty());
    drive.file_system = Some(from_wide(&fs_name)).filter(|s| !s.is_empty());

    let (mut available, mut total, mut free) = (0u64, 0u64, 0u64);
    // SAFETY: out-pointers reference live locals; `root_w` outlives the call.
    let space = unsafe {
        GetDiskFreeSpaceExW(
            root_p,
            Some(&mut available),
            Some(&mut total),
            Some(&mut free),
        )
    };
    match space {
        Ok(()) => {
            drive.space = Some(Space::new(total, free, available));
            drive.status = DriveStatus::Ready;
        }
        Err(err) => {
            tracing::warn!(root, error = %err, "free space unavailable");
            drive.status = status_from(&err);
        }
    }
    drive
}

fn status_from(err: &windows::core::Error) -> DriveStatus {
    if err.code() == ERROR_NOT_READY.to_hresult() {
        DriveStatus::NoMedia
    } else {
        DriveStatus::Error {
            message: err.message(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_drive_type_codes() {
        assert_eq!(DriveKind::from_win32(3), DriveKind::Fixed);
        assert_eq!(DriveKind::from_win32(2), DriveKind::Removable);
        assert_eq!(DriveKind::from_win32(4), DriveKind::Network);
        assert_eq!(DriveKind::from_win32(5), DriveKind::Optical);
        assert_eq!(DriveKind::from_win32(6), DriveKind::RamDisk);
        assert_eq!(DriveKind::from_win32(0), DriveKind::Unknown);
        assert_eq!(DriveKind::from_win32(1), DriveKind::Unknown);
        assert!(!DriveKind::Network.is_local());
        assert!(!DriveKind::Optical.is_local());
    }

    #[test]
    fn splits_multi_sz() {
        let buf: Vec<u16> = "C:\\\0D:\\\0\0".encode_utf16().collect();
        assert_eq!(split_multi_sz(&buf), ["C:\\", "D:\\"]);
        assert!(split_multi_sz(&[0, 0]).is_empty());
    }

    #[test]
    fn space_math_and_low_threshold() {
        let s = Space::new(1000, 250, 200);
        assert_eq!(s.used_bytes, 750);
        assert!(!s.low_space);
        assert!(Space::new(1000, 99, 99).low_space);
        assert!(!Space::new(1000, 100, 100).low_space);
        // Inconsistent OS figures are clamped rather than underflowing.
        let odd = Space::new(100, 150, 500);
        assert_eq!(
            (odd.free_bytes, odd.used_bytes, odd.available_bytes),
            (100, 0, 100)
        );
        assert!(!Space::new(0, 0, 0).low_space);
        let huge = Space::new(u64::MAX, u64::MAX / 20, 0);
        assert!(huge.low_space);
    }

    #[test]
    fn serializes_status_with_tag() {
        let json = serde_json::to_string(&DriveStatus::NoMedia).unwrap_or_default();
        assert_eq!(json, r#"{"state":"noMedia"}"#);
        let json = serde_json::to_string(&DriveStatus::Error {
            message: "x".into(),
        })
        .unwrap_or_default();
        assert_eq!(json, r#"{"state":"error","message":"x"}"#);
    }
}
