//! Batched directory enumeration via `GetFileInformationByHandleEx`.
//!
//! Unlike `std::fs::read_dir` (FindFirstFile), this returns each entry's *allocation*
//! size in the same call, so sparse, compressed and cloud-placeholder files report the
//! space they really occupy without an extra system call per file.

use std::ffi::c_void;
use std::fs::OpenOptions;
use std::io;
use std::mem::{offset_of, size_of};
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::Path;

use windows::Win32::Foundation::{ERROR_NO_MORE_FILES, HANDLE};
use windows::Win32::Storage::FileSystem::{
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_FULL_DIR_INFO,
    FileFullDirectoryInfo, FileFullDirectoryRestartInfo, GetFileInformationByHandleEx,
};

const FILE_LIST_DIRECTORY: u32 = 0x0001;
const FILE_SHARE_ALL: u32 = 0x7;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
/// 64 KiB, 8-byte aligned.
const BUF_WORDS: usize = 8 * 1024;

#[derive(Debug, Clone)]
pub(crate) struct RawEntry {
    pub name: String,
    pub attributes: u32,
    pub logical_bytes: u64,
    pub allocated_bytes: u64,
    /// Reparse tag, valid only when [`RawEntry::is_reparse_point`] is true.
    pub reparse_tag: u32,
}

impl RawEntry {
    pub fn is_dir(&self) -> bool {
        self.attributes & FILE_ATTRIBUTE_DIRECTORY != 0
    }

    pub fn is_reparse_point(&self) -> bool {
        self.attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
}

/// List the entries of `dir` (excluding `.` and `..`).
///
/// The directory is opened without following a reparse point and with full sharing,
/// so enumeration never locks anything for other processes.
pub(crate) fn read_dir(dir: &Path) -> io::Result<Vec<RawEntry>> {
    let handle = OpenOptions::new()
        .access_mode(FILE_LIST_DIRECTORY)
        .share_mode(FILE_SHARE_ALL)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0 | FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(dir)?;
    let raw = HANDLE(handle.as_raw_handle());

    let mut buf = vec![0u64; BUF_WORDS];
    let buf_bytes = buf.len() * size_of::<u64>();
    let mut out = Vec::new();
    let mut class = FileFullDirectoryRestartInfo;
    loop {
        // SAFETY: `raw` is valid while `handle` lives; the buffer pointer and size match.
        let res = unsafe {
            GetFileInformationByHandleEx(
                raw,
                class,
                buf.as_mut_ptr().cast::<c_void>(),
                buf_bytes as u32,
            )
        };
        if let Err(err) = res {
            if err.code() == ERROR_NO_MORE_FILES.to_hresult() {
                break;
            }
            return Err(io::Error::from_raw_os_error(err.code().0 & 0xFFFF));
        }
        // SAFETY: the buffer is fully initialized (zeroed) and the OS wrote a chain of
        // FILE_FULL_DIR_INFO records into it; `parse` bounds-checks every access.
        unsafe { parse(buf.as_ptr().cast::<u8>(), buf_bytes, &mut out) };
        class = FileFullDirectoryInfo;
    }
    Ok(out)
}

/// Walk a chain of `FILE_FULL_DIR_INFO` records.
///
/// # Safety
/// `base` must point to `len` readable bytes.
unsafe fn parse(base: *const u8, len: usize, out: &mut Vec<RawEntry>) {
    let name_off = offset_of!(FILE_FULL_DIR_INFO, FileName);
    let mut off = 0usize;
    loop {
        if off + name_off > len {
            return;
        }
        // SAFETY: bounds checked above; read_unaligned tolerates any alignment.
        let info = unsafe { base.add(off).cast::<FILE_FULL_DIR_INFO>().read_unaligned() };
        let name_len = info.FileNameLength as usize / 2;
        if off + name_off + name_len * 2 > len {
            return;
        }
        let name: Vec<u16> = (0..name_len)
            // SAFETY: each u16 lies within the bounds checked above.
            .map(|i| unsafe {
                base.add(off + name_off + i * 2)
                    .cast::<u16>()
                    .read_unaligned()
            })
            .collect();
        let name = String::from_utf16_lossy(&name);
        if name != "." && name != ".." {
            let attributes = info.FileAttributes;
            out.push(RawEntry {
                name,
                attributes,
                logical_bytes: u64::try_from(info.EndOfFile).unwrap_or(0),
                allocated_bytes: u64::try_from(info.AllocationSize).unwrap_or(0),
                // For reparse points the EaSize field carries the reparse tag.
                reparse_tag: if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                    info.EaSize
                } else {
                    0
                },
            });
        }
        if info.NextEntryOffset == 0 {
            return;
        }
        off += info.NextEntryOffset as usize;
    }
}
