//! Rename an already-open object through its handle.
//!
//! Renaming by handle (not by path) means the object moved is exactly the one whose
//! identity was verified when the handle was opened. `ReplaceIfExists` is always false,
//! so nothing at the destination is ever overwritten, and a rename cannot cross volumes,
//! so a move is never silently turned into copy-and-delete.

use std::ffi::c_void;
use std::fs::File;
use std::io;
use std::mem::{offset_of, size_of};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::AsRawHandle;
use std::path::Path;

use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::{
    FILE_RENAME_INFO, FileRenameInfo, SetFileInformationByHandle,
};

/// Rename the object behind `file` to the absolute path `dest` on the same volume.
pub(crate) fn rename_by_handle(file: &File, dest: &Path) -> io::Result<()> {
    if !dest.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "destination must be absolute",
        ));
    }
    let mut verbatim = std::ffi::OsString::from(r"\\?\");
    verbatim.push(dest.as_os_str());
    let name: Vec<u16> = verbatim.encode_wide().collect();

    let name_off = offset_of!(FILE_RENAME_INFO, FileName);
    let total = name_off + (name.len() + 1) * size_of::<u16>();
    let mut buf = vec![0u64; total.div_ceil(size_of::<u64>())];
    let base = buf.as_mut_ptr().cast::<u8>();
    // SAFETY: `buf` is zeroed, 8-byte aligned and at least `total` bytes long. The
    // header fields are written in place (ReplaceIfExists stays 0 = false, RootDirectory
    // stays null) and the name is copied into the trailing flexible array.
    unsafe {
        let info = base.cast::<FILE_RENAME_INFO>();
        (*info).FileNameLength = u32::try_from(name.len() * size_of::<u16>())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path too long"))?;
        std::ptr::copy_nonoverlapping(name.as_ptr(), base.add(name_off).cast::<u16>(), name.len());
    }
    // SAFETY: the handle is valid while `file` lives; the buffer and size match.
    unsafe {
        SetFileInformationByHandle(
            HANDLE(file.as_raw_handle()),
            FileRenameInfo,
            base.cast::<c_void>(),
            u32::try_from(total).unwrap_or(u32::MAX),
        )
    }
    .map_err(|e| io::Error::from_raw_os_error(e.code().0 & 0xFFFF))
}

/// Errors that mean "another program is using this", as opposed to a real failure.
pub(crate) fn is_in_use(err: &io::Error) -> bool {
    matches!(err.raw_os_error(), Some(5 | 32 | 33))
}
