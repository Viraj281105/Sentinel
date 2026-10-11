//! Where a volume's NTFS change journal stands, readable without elevation.
//!
//! Measured in docs/spikes/PHASE7_CHANGE_TRACKING.md: an unelevated process can ask a
//! *folder* handle for the journal's id and next sequence number, but cannot read the records
//! themselves. That is enough to answer one question at startup: "did anything at all change on
//! this volume since I last recorded the position?" Equal position means no. A different one
//! only means "something did, somewhere on the volume", which is why callers treat it as
//! "rescan", never as "these folders changed".
//!
//! Read-only. A volume with no journal (FAT, ReFS without one, network shares) reports an error
//! and callers fall back to a full analysis.

use std::ffi::c_void;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE,
    FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::IO::DeviceIoControl;
use windows::Win32::System::Ioctl::FSCTL_QUERY_USN_JOURNAL;
use windows::core::PCWSTR;

/// Identifies one incarnation of a volume's journal and how far it has advanced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JournalPosition {
    /// Changes if the journal is deleted and recreated, which resets `next_usn`.
    pub journal_id: u64,
    /// The sequence number the next change will receive.
    pub next_usn: u64,
}

/// How a recorded position compares with the current one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JournalChange {
    /// Nothing at all was journaled on the volume since: the recorded state is still exact.
    Unchanged,
    /// Something changed somewhere on the volume. `bytes` is how far the journal advanced,
    /// a rough size of the activity, not a count of files.
    Changed { bytes: u64 },
    /// The journal was recreated, so nothing can be said about what happened in between.
    Reset,
}

impl JournalPosition {
    /// Compare this *recorded* position with the `now` one.
    pub fn compare(self, now: Self) -> JournalChange {
        if self.journal_id != now.journal_id || now.next_usn < self.next_usn {
            JournalChange::Reset
        } else if now.next_usn == self.next_usn {
            JournalChange::Unchanged
        } else {
            JournalChange::Changed {
                bytes: now.next_usn - self.next_usn,
            }
        }
    }
}

fn le_u64(b: &[u8], at: usize) -> Option<u64> {
    b.get(at..at + 8)?.try_into().ok().map(u64::from_le_bytes)
}

/// The journal position of the volume that holds `folder`.
pub fn journal_position(folder: &Path) -> io::Result<JournalPosition> {
    let wide: Vec<u16> = folder
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: `wide` is a NUL-terminated UTF-16 string that outlives the call.
    let handle = unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            FILE_READ_ATTRIBUTES.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            None,
        )
    }
    .map_err(|e| io::Error::other(e.message()))?;
    let result = query(handle);
    // SAFETY: the handle was opened above and is not used again.
    unsafe {
        let _ = CloseHandle(handle);
    }
    result
}

fn query(handle: HANDLE) -> io::Result<JournalPosition> {
    let mut out = vec![0u8; 4096];
    let mut got = 0u32;
    // SAFETY: `out` is valid for its length for the duration of the call.
    unsafe {
        DeviceIoControl(
            handle,
            FSCTL_QUERY_USN_JOURNAL,
            None,
            0,
            Some(out.as_mut_ptr().cast::<c_void>()),
            out.len() as u32,
            Some(&mut got),
            None,
        )
    }
    .map_err(|e| io::Error::other(e.message()))?;
    out.truncate(got as usize);
    // USN_JOURNAL_DATA: journal id at 0, first USN at 8, next USN at 16.
    match (le_u64(&out, 0), le_u64(&out, 16)) {
        (Some(journal_id), Some(next_usn)) => Ok(JournalPosition {
            journal_id,
            next_usn,
        }),
        _ => Err(io::Error::other("the journal answer was too short")),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn pos(id: u64, usn: u64) -> JournalPosition {
        JournalPosition {
            journal_id: id,
            next_usn: usn,
        }
    }

    #[test]
    fn comparison_distinguishes_unchanged_changed_and_reset() {
        let was = pos(7, 1000);
        assert_eq!(was.compare(pos(7, 1000)), JournalChange::Unchanged);
        assert_eq!(
            was.compare(pos(7, 1600)),
            JournalChange::Changed { bytes: 600 }
        );
        // A new journal id, or a position that went backwards, means the journal was
        // recreated: nothing about the gap is known.
        assert_eq!(was.compare(pos(8, 1000)), JournalChange::Reset);
        assert_eq!(was.compare(pos(7, 10)), JournalChange::Reset);
    }

    #[test]
    fn a_real_ntfs_volume_reports_a_position_that_advances_when_files_change() {
        let dir = tempfile::tempdir().unwrap();
        let before = journal_position(dir.path()).unwrap();
        for i in 0..5 {
            std::fs::write(dir.path().join(format!("f{i}")), vec![1u8; 4096]).unwrap();
        }
        let after = journal_position(dir.path()).unwrap();
        assert_eq!(before.journal_id, after.journal_id);
        assert!(after.next_usn > before.next_usn, "{before:?} -> {after:?}");
        assert!(matches!(
            before.compare(after),
            JournalChange::Changed { .. }
        ));
    }

    #[test]
    fn a_missing_folder_is_an_error_not_a_position() {
        assert!(journal_position(Path::new(r"C:\definitely\not\here\sentinel")).is_err());
    }
}
