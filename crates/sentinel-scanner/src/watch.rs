//! Folder watcher: turns `ReadDirectoryChangesW` events into a set of directories whose
//! contents changed, for the incremental rescan in [`crate::incremental`].
//!
//! Design (from the Phase 7 spike, docs/spikes/PHASE7_CHANGE_TRACKING.md):
//! - Events are *hints*. A path in an event only says "something changed in this
//!   directory"; nothing is reconstructed from event contents, so a missed event costs a
//!   stale directory until the next rescan, never a wrong tree.
//! - The reader thread does nothing but call the API and hand raw bytes to a bounded
//!   queue. Overflow of the kernel buffer discards a whole interval, and a stalled reader
//!   is what causes it, so no parsing, locking or I/O happens on that thread.
//! - Anything lost (kernel overflow, a full queue, too many dirty directories, a watch
//!   that broke) is reported as [`Changes::root_dirty`]: rescan the root.
//! - Very noisy folders (`Temp`, `node_modules`, `.git`, caches, build output) are
//!   coalesced: an event anywhere under one marks that folder's whole subtree dirty
//!   instead of recording every directory in it.
//!
//! Read-only: the watched folder is opened for listing with full sharing, so it can still
//! be renamed or deleted while watched.

use std::collections::BTreeSet;
use std::ffi::c_void;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use windows::Win32::Foundation::{CloseHandle, ERROR_NOTIFY_ENUM_DIR, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_LIST_DIRECTORY, FILE_NOTIFY_CHANGE,
    FILE_NOTIFY_CHANGE_ATTRIBUTES, FILE_NOTIFY_CHANGE_CREATION, FILE_NOTIFY_CHANGE_DIR_NAME,
    FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, ReadDirectoryChangesW,
};
use windows::Win32::System::IO::CancelIoEx;
use windows::core::PCWSTR;

/// Folder names whose contents churn constantly and are rarely worth tracking one
/// directory at a time. Compared case-insensitively against each path component.
pub const NOISY_FOLDERS: &[&str] = &[
    "temp",
    "tmp",
    "node_modules",
    ".git",
    "__pycache__",
    ".cache",
    "cache",
    "caches",
    "inetcache",
    "target",
    "$recycle.bin",
];

const MIN_BUFFER: u32 = 4 * 1024;
const MAX_BUFFER: u32 = 4 * 1024 * 1024;
/// Largest buffer a network path accepts.
const NETWORK_BUFFER: u32 = 64 * 1024;
const QUEUE_BATCHES: usize = 1024;

#[derive(Debug, Clone)]
pub struct WatchOptions {
    /// Kernel buffer per watch, clamped to 4 KB..4 MB. A larger buffer survives longer
    /// stalls; 512 KB held a 200 ms stall under a 16,000 event/s burst in the spike.
    pub buffer_bytes: u32,
    /// Folder names to coalesce (see [`NOISY_FOLDERS`]).
    pub noisy_folders: Vec<String>,
    /// More dirty directories than this collapses to [`Changes::root_dirty`], which bounds
    /// memory.
    pub max_dirty_dirs: usize,
    /// Testing aid: sleep this long after each batch to simulate a stalled reader.
    #[doc(hidden)]
    pub reader_stall: Duration,
}

impl Default for WatchOptions {
    fn default() -> Self {
        Self {
            buffer_bytes: 512 * 1024,
            noisy_folders: NOISY_FOLDERS.iter().map(|s| (*s).to_owned()).collect(),
            max_dirty_dirs: 20_000,
            reader_stall: Duration::ZERO,
        }
    }
}

/// What changed since the last [`Watcher::drain`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Changes {
    /// Directories whose *direct* contents changed (entries added, removed, renamed or
    /// modified). Absolute paths.
    pub dirty_dirs: Vec<PathBuf>,
    /// Noisy folders with a change somewhere beneath them: rescan the whole subtree.
    pub dirty_subtrees: Vec<PathBuf>,
    /// Events were lost or the watch broke: the root must be rescanned in full.
    pub root_dirty: bool,
    /// Events received (including those folded into the sets above).
    pub events: u64,
    /// Kernel buffer overflows (each lost a whole interval of events).
    pub overflows: u64,
    /// Batches dropped because the queue was full.
    pub dropped_batches: u64,
    /// Why the watch stopped working, if it did.
    pub broken: Option<String>,
}

impl Changes {
    /// Nothing to do.
    pub fn is_empty(&self) -> bool {
        self.dirty_dirs.is_empty() && self.dirty_subtrees.is_empty() && !self.root_dirty
    }
}

#[derive(Default)]
struct State {
    dirty: BTreeSet<PathBuf>,
    subtrees: BTreeSet<PathBuf>,
    root_dirty: bool,
    events: u64,
    overflows: u64,
    dropped: u64,
    broken: Option<String>,
}

/// A running watch of one folder tree. Stops when dropped.
pub struct Watcher {
    root: PathBuf,
    handle: isize,
    stopping: Arc<AtomicBool>,
    state: Arc<Mutex<State>>,
    reader: Option<JoinHandle<()>>,
    worker: Option<JoinHandle<()>>,
}

fn wide(p: &Path) -> Vec<u16> {
    p.as_os_str()
        .to_string_lossy()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect()
}

const FILTER: FILE_NOTIFY_CHANGE = FILE_NOTIFY_CHANGE(
    FILE_NOTIFY_CHANGE_FILE_NAME.0
        | FILE_NOTIFY_CHANGE_DIR_NAME.0
        | FILE_NOTIFY_CHANGE_ATTRIBUTES.0
        | FILE_NOTIFY_CHANGE_SIZE.0
        | FILE_NOTIFY_CHANGE_LAST_WRITE.0
        | FILE_NOTIFY_CHANGE_CREATION.0,
);

/// Parse one batch of `FILE_NOTIFY_INFORMATION` records into `(action, relative path)`.
/// Stops at the first malformed record; a short batch is never read past its end.
pub(crate) fn parse_batch(bytes: &[u8]) -> Vec<(u32, String)> {
    let mut out = Vec::new();
    let mut off = 0usize;
    loop {
        let Some(head) = bytes.get(off..off + 12) else {
            break;
        };
        let next = u32::from_le_bytes([head[0], head[1], head[2], head[3]]) as usize;
        let action = u32::from_le_bytes([head[4], head[5], head[6], head[7]]);
        let len = u32::from_le_bytes([head[8], head[9], head[10], head[11]]) as usize;
        let Some(name) = bytes.get(off + 12..off + 12 + len) else {
            break;
        };
        let units: Vec<u16> = name
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        out.push((action, String::from_utf16_lossy(&units)));
        if next == 0 {
            break;
        }
        off += next;
    }
    out
}

/// Fold one parsed event into the state. `rel` is relative to `root`.
fn record(state: &mut State, root: &Path, noisy: &[String], max_dirty: usize, rel: &str) {
    if state.root_dirty {
        return; // everything is dirty already; stay bounded
    }
    let comps: Vec<&str> = rel.split('\\').filter(|c| !c.is_empty()).collect();
    if comps.is_empty() {
        return;
    }
    // A noisy folder above the changed entry: mark that folder's subtree and stop.
    // (The noisy folder itself being created or removed is a change to its parent.)
    if let Some(i) = comps[..comps.len() - 1]
        .iter()
        .position(|c| noisy.iter().any(|n| n.eq_ignore_ascii_case(c)))
    {
        let mut p = root.to_path_buf();
        p.extend(&comps[..=i]);
        state.subtrees.insert(p);
        return;
    }
    let mut parent = root.to_path_buf();
    parent.extend(&comps[..comps.len() - 1]);
    state.dirty.insert(parent);
    if state.dirty.len() > max_dirty {
        state.dirty.clear();
        state.subtrees.clear();
        state.root_dirty = true;
    }
}

fn worker_loop(rx: &Receiver<Vec<u8>>, state: &Mutex<State>, root: &Path, opts: &WatchOptions) {
    let noisy: Vec<String> = opts.noisy_folders.clone();
    while let Ok(batch) = rx.recv() {
        let events = parse_batch(&batch);
        if let Ok(mut s) = state.lock() {
            s.events += events.len() as u64;
            if s.root_dirty {
                continue; // everything is dirty already; still counting events
            }
            for (_, rel) in &events {
                record(&mut s, root, &noisy, opts.max_dirty_dirs, rel);
            }
        }
    }
}

fn reader_loop(
    raw: isize,
    buffer_bytes: u32,
    stall: Duration,
    tx: &SyncSender<Vec<u8>>,
    state: &Mutex<State>,
    stopping: &AtomicBool,
) {
    let handle = HANDLE(raw as *mut c_void);
    let mut size = buffer_bytes;
    // u64 elements keep the buffer aligned for FILE_NOTIFY_INFORMATION.
    let mut buf = vec![0u64; size as usize / 8];
    let mut first = true;
    loop {
        if stopping.load(Ordering::SeqCst) {
            return;
        }
        let mut got = 0u32;
        // SAFETY: `buf` and `got` outlive this synchronous call; the handle stays open
        // until the owner has cancelled the call and joined this thread.
        let r = unsafe {
            ReadDirectoryChangesW(
                handle,
                buf.as_mut_ptr().cast::<c_void>(),
                size,
                true,
                FILTER,
                Some(&mut got),
                None,
                None,
            )
        };
        if let Err(e) = r {
            if stopping.load(Ordering::SeqCst) {
                return;
            }
            // Network paths refuse buffers over 64 KB: retry once with the smaller size.
            if first && size > NETWORK_BUFFER {
                size = NETWORK_BUFFER;
                buf.truncate(size as usize / 8);
                first = false;
                continue;
            }
            let overflow = e.code() == ERROR_NOTIFY_ENUM_DIR.to_hresult();
            if let Ok(mut s) = state.lock() {
                s.root_dirty = true;
                if overflow {
                    s.overflows += 1;
                } else {
                    s.broken = Some(e.message());
                }
            }
            if overflow {
                continue;
            }
            return;
        }
        first = false;
        if got == 0 {
            // The kernel buffer overflowed: a whole interval of events is gone.
            if let Ok(mut s) = state.lock() {
                s.overflows += 1;
                s.root_dirty = true;
            }
            continue;
        }
        // SAFETY: the kernel wrote `got` initialized bytes at the start of `buf`.
        let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), got as usize) };
        if tx.try_send(bytes.to_vec()).is_err()
            && let Ok(mut s) = state.lock()
        {
            // The consumer fell behind: treat it like an overflow, and keep reading.
            s.dropped += 1;
            s.root_dirty = true;
        }
        if !stall.is_zero() {
            thread::sleep(stall);
        }
    }
}

impl Watcher {
    /// Start watching `root` and everything beneath it.
    pub fn start(root: &Path, opts: &WatchOptions) -> io::Result<Self> {
        if !root.is_absolute() || !root.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{} is not a folder that can be watched", root.display()),
            ));
        }
        let w = wide(root);
        // SAFETY: `w` is a NUL-terminated UTF-16 string that outlives the call.
        let handle = unsafe {
            CreateFileW(
                PCWSTR(w.as_ptr()),
                FILE_LIST_DIRECTORY.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                None,
            )
        }
        .map_err(|e| io::Error::other(e.message()))?;
        let raw = handle.0 as isize;
        let state = Arc::new(Mutex::new(State::default()));
        let stopping = Arc::new(AtomicBool::new(false));
        let (tx, rx) = sync_channel::<Vec<u8>>(QUEUE_BATCHES);
        let buffer = opts.buffer_bytes.clamp(MIN_BUFFER, MAX_BUFFER) & !7;
        let stall = opts.reader_stall;

        let reader = {
            let (state, stopping) = (state.clone(), stopping.clone());
            thread::Builder::new()
                .name("sentinel-watch-read".into())
                .spawn(move || reader_loop(raw, buffer, stall, &tx, &state, &stopping))?
        };
        let worker = {
            let (state, root, opts) = (state.clone(), root.to_path_buf(), opts.clone());
            thread::Builder::new()
                .name("sentinel-watch-fold".into())
                .spawn(move || worker_loop(&rx, &state, &root, &opts))?
        };
        tracing::info!(root = %root.display(), buffer, "folder watch started");
        Ok(Self {
            root: root.to_path_buf(),
            handle: raw,
            stopping,
            state,
            reader: Some(reader),
            worker: Some(worker),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Take everything recorded since the last call. After a [`Changes::root_dirty`]
    /// result the watcher keeps running; the caller rescans the root and carries on.
    pub fn drain(&self) -> Changes {
        let Ok(mut s) = self.state.lock() else {
            return Changes {
                root_dirty: true,
                broken: Some("the watcher's state was poisoned".into()),
                ..Changes::default()
            };
        };
        let st = std::mem::take(&mut *s);
        // Keep the break reason visible until the watcher is dropped.
        s.broken.clone_from(&st.broken);
        // A dirty directory under a dirty subtree is covered by the subtree.
        let dirty_dirs = st
            .dirty
            .into_iter()
            .filter(|d| !st.subtrees.iter().any(|t| d.starts_with(t)))
            .collect();
        Changes {
            dirty_dirs,
            dirty_subtrees: st.subtrees.into_iter().collect(),
            root_dirty: st.root_dirty,
            events: st.events,
            overflows: st.overflows,
            dropped_batches: st.dropped,
            broken: st.broken,
        }
    }

    /// Stop watching and wait for the threads. Also done on drop.
    pub fn stop(&mut self) {
        if self.reader.is_none() {
            return;
        }
        self.stopping.store(true, Ordering::SeqCst);
        let handle = HANDLE(self.handle as *mut c_void);
        if let Some(r) = self.reader.take() {
            // The reader may be between calls when the cancel arrives, with nothing to
            // cancel yet; it checks the flag before its next call, so keep cancelling
            // until it has exited.
            while !r.is_finished() {
                // SAFETY: cancels pending calls on a handle this struct owns.
                unsafe {
                    let _ = CancelIoEx(handle, None);
                }
                thread::sleep(Duration::from_millis(5));
            }
            let _ = r.join();
        }
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
        // SAFETY: both threads have exited, so nothing uses the handle; closed once.
        unsafe {
            let _ = CloseHandle(HANDLE(self.handle as *mut c_void));
        }
        tracing::info!(root = %self.root.display(), "folder watch stopped");
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noisy() -> Vec<String> {
        NOISY_FOLDERS.iter().map(|s| (*s).to_owned()).collect()
    }

    fn rec(s: &mut State, rel: &str) {
        record(s, Path::new(r"C:\root"), &noisy(), 100, rel);
    }

    #[test]
    fn parses_notify_records_and_ignores_truncated_ones() {
        // Two records: "a.txt" (ADDED) then "sub\b" (RENAMED_NEW).
        let mut b = Vec::new();
        let name = |s: &str| {
            s.encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<u8>>()
        };
        let n1 = name("a.txt");
        let first_len = 12 + n1.len(); // already 4-aligned? 10 bytes + 12 = 22: pad to 24
        let padded = (first_len + 3) & !3;
        b.extend_from_slice(&(padded as u32).to_le_bytes());
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&(n1.len() as u32).to_le_bytes());
        b.extend_from_slice(&n1);
        b.resize(padded, 0);
        let n2 = name(r"sub\b");
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&5u32.to_le_bytes());
        b.extend_from_slice(&(n2.len() as u32).to_le_bytes());
        b.extend_from_slice(&n2);
        let ev = parse_batch(&b);
        assert_eq!(ev, vec![(1, "a.txt".to_owned()), (5, r"sub\b".to_owned())]);
        // Truncated: the name runs past the end.
        assert_eq!(parse_batch(&b[..b.len() - 4]).len(), 1);
        assert!(parse_batch(&b[..5]).is_empty());
        assert!(parse_batch(&[]).is_empty());
    }

    #[test]
    fn an_event_marks_the_parent_directory() {
        let mut s = State::default();
        rec(&mut s, r"a\b\file.txt");
        rec(&mut s, "top.txt");
        let dirs: Vec<_> = s.dirty.iter().map(|p| p.display().to_string()).collect();
        assert_eq!(dirs, [r"C:\root", r"C:\root\a\b"]);
        assert!(s.subtrees.is_empty() && !s.root_dirty);
    }

    #[test]
    fn noisy_folders_collapse_to_their_subtree() {
        let mut s = State::default();
        rec(&mut s, r"AppData\Local\Temp\x\y\z.tmp");
        rec(&mut s, r"AppData\Local\Temp\other.tmp");
        rec(&mut s, r"proj\NODE_MODULES\pkg\index.js");
        let subs: Vec<_> = s.subtrees.iter().map(|p| p.display().to_string()).collect();
        assert_eq!(
            subs,
            [r"C:\root\AppData\Local\Temp", r"C:\root\proj\NODE_MODULES"]
        );
        assert!(
            s.dirty.is_empty(),
            "nothing inside a noisy folder is tracked singly"
        );
    }

    #[test]
    fn creating_or_removing_a_noisy_folder_itself_dirties_its_parent() {
        let mut s = State::default();
        rec(&mut s, r"proj\node_modules");
        assert!(s.subtrees.is_empty());
        assert_eq!(s.dirty.len(), 1);
        assert!(s.dirty.contains(Path::new(r"C:\root\proj")));
    }

    #[test]
    fn too_many_dirty_directories_collapse_to_the_root() {
        let mut s = State::default();
        for i in 0..150 {
            rec(&mut s, &format!(r"d{i}\f.txt"));
        }
        assert!(s.root_dirty);
        assert!(s.dirty.is_empty(), "memory is bounded once collapsed");
    }
}
