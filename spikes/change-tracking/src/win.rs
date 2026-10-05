//! Windows side of the spike.

use std::collections::HashMap;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{CloseHandle, FILETIME, GENERIC_READ, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_LIST_DIRECTORY, FILE_NOTIFY_CHANGE_ATTRIBUTES,
    FILE_NOTIFY_CHANGE_CREATION, FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME,
    FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE, FILE_READ_ATTRIBUTES,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, ReadDirectoryChangesW,
};
use windows::Win32::System::IO::{CancelIoEx, DeviceIoControl};
use windows::Win32::System::Ioctl::{
    FSCTL_ENUM_USN_DATA, FSCTL_QUERY_USN_JOURNAL, FSCTL_READ_FILE_USN_DATA, FSCTL_READ_USN_JOURNAL,
};
use windows::Win32::System::Threading::{GetCurrentThread, GetThreadTimes};
use windows::core::PCWSTR;

fn wide(p: &str) -> Vec<u16> {
    p.encode_utf16().chain(std::iter::once(0)).collect()
}

fn filetime_ms(ft: FILETIME) -> f64 {
    let v = (u64::from(ft.dwHighDateTime) << 32) | u64::from(ft.dwLowDateTime);
    v as f64 / 10_000.0
}

pub fn run() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("usn") => usn(),
        Some("burst") => burst(args.get(1).and_then(|s| s.parse().ok()).unwrap_or(20_000)),
        Some("ambient") => match args.get(1) {
            Some(dir) => ambient(
                Path::new(dir),
                args.get(2).and_then(|s| s.parse().ok()).unwrap_or(15),
            ),
            None => eprintln!("usage: ambient <folder> [seconds]"),
        },
        _ => eprintln!("usage: usn | burst [files] | ambient <folder> [seconds]"),
    }
}

// ---------------------------------------------------------------------------------
// Experiment 1: what can an unelevated process do with the USN journal?
// ---------------------------------------------------------------------------------

fn usn() {
    println!("Unelevated USN journal access, per volume and access mask:");
    for letter in ['C', 'D'] {
        let path = format!(r"\\.\{letter}:");
        if !Path::new(&format!(r"{letter}:\")).exists() {
            continue;
        }
        for (label, access) in [
            ("access 0 (query only)", 0u32),
            ("FILE_READ_ATTRIBUTES", FILE_READ_ATTRIBUTES.0),
            ("GENERIC_READ", GENERIC_READ.0),
        ] {
            let w = wide(&path);
            // SAFETY: `w` is a NUL-terminated UTF-16 string that outlives the call.
            let handle = unsafe {
                CreateFileW(
                    PCWSTR(w.as_ptr()),
                    access,
                    FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                    None,
                    OPEN_EXISTING,
                    Default::default(),
                    None,
                )
            };
            match handle {
                Err(e) => println!("  {path} {label:<24} open FAILED: {e}"),
                Ok(h) => {
                    let mut out = [0u8; 256];
                    let mut got = 0u32;
                    // SAFETY: `out` and `got` are valid for the call; `h` is an open handle.
                    let q = unsafe {
                        DeviceIoControl(
                            h,
                            FSCTL_QUERY_USN_JOURNAL,
                            None,
                            0,
                            Some(out.as_mut_ptr().cast::<c_void>()),
                            out.len() as u32,
                            Some(&mut got),
                            None,
                        )
                    };
                    println!(
                        "  {path} {label:<24} open ok; QUERY_USN_JOURNAL: {}",
                        match q {
                            Ok(()) => format!("ok ({got} bytes)"),
                            Err(e) => format!("FAILED: {e}"),
                        }
                    );
                    // Reading records is what a monitor needs; try it with this handle.
                    let req = [0u8; 40]; // READ_USN_JOURNAL_DATA_V0, all zero: start at 0
                    let mut buf = vec![0u8; 4096];
                    // SAFETY: as above.
                    let r = unsafe {
                        DeviceIoControl(
                            h,
                            FSCTL_READ_USN_JOURNAL,
                            Some(req.as_ptr().cast::<c_void>()),
                            req.len() as u32,
                            Some(buf.as_mut_ptr().cast::<c_void>()),
                            buf.len() as u32,
                            Some(&mut got),
                            None,
                        )
                    };
                    println!(
                        "  {path} {label:<24}                     READ_USN_JOURNAL:  {}",
                        match r {
                            Ok(()) => format!("ok ({got} bytes)"),
                            Err(e) => format!("FAILED: {e}"),
                        }
                    );
                    // SAFETY: `h` was opened above and is closed once.
                    unsafe {
                        let _ = CloseHandle(h);
                    }
                }
            }
        }
    }
    usn_other_handles();
}

fn ioctl_try(h: HANDLE, code: u32, input: &[u8]) -> String {
    match ioctl_raw(h, code, input) {
        Ok(out) => format!("ok ({} bytes)", out.len()),
        Err(e) => format!("FAILED: {e}"),
    }
}

fn ioctl_raw(h: HANDLE, code: u32, input: &[u8]) -> Result<Vec<u8>, windows::core::Error> {
    let mut out = vec![0u8; 4096];
    let mut got = 0u32;
    // SAFETY: buffers are valid for the call and `h` is an open handle.
    let r = unsafe {
        DeviceIoControl(
            h,
            code,
            (!input.is_empty()).then(|| input.as_ptr().cast::<c_void>()),
            input.len() as u32,
            Some(out.as_mut_ptr().cast::<c_void>()),
            out.len() as u32,
            Some(&mut got),
            None,
        )
    };
    r.map(|()| {
        out.truncate(got as usize);
        out
    })
}

fn le_u64(b: &[u8], at: usize) -> u64 {
    b.get(at..at + 8)
        .and_then(|s| s.try_into().ok())
        .map_or(0, u64::from_le_bytes)
}

/// Journal id, next USN and maximum size from a FSCTL_QUERY_USN_JOURNAL answer.
fn journal_state(h: HANDLE) -> Option<(u64, u64, u64)> {
    let b = ioctl_raw(h, FSCTL_QUERY_USN_JOURNAL, &[]).ok()?;
    Some((le_u64(&b, 0), le_u64(&b, 16), le_u64(&b, 40)))
}

fn open_dir_or_file(path: &str, access: u32) -> Result<HANDLE, windows::core::Error> {
    let w = wide(path);
    // SAFETY: `w` is a NUL-terminated UTF-16 string that outlives the call.
    unsafe {
        CreateFileW(
            PCWSTR(w.as_ptr()),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            None,
        )
    }
}

/// Handles other than the volume device: the root folder and an ordinary file.
fn usn_other_handles() {
    println!(
        "
Other handle types (what an unelevated process can reach):"
    );
    let enum_in = {
        let mut v = Vec::new();
        v.extend_from_slice(&0u64.to_le_bytes()); // StartFileReferenceNumber
        v.extend_from_slice(&0i64.to_le_bytes()); // LowUsn
        v.extend_from_slice(&i64::MAX.to_le_bytes()); // HighUsn
        v
    };
    for letter in ['C', 'D'] {
        let root = format!(r"{letter}:\");
        if !Path::new(&root).exists() {
            continue;
        }
        for (label, access) in [
            ("FILE_READ_ATTRIBUTES", FILE_READ_ATTRIBUTES.0),
            ("FILE_LIST_DIRECTORY", FILE_LIST_DIRECTORY.0),
        ] {
            match open_dir_or_file(&root, access) {
                Err(e) => println!("  {root} dir, {label:<22} open FAILED: {e}"),
                Ok(h) => {
                    println!(
                        "  {root} dir, {label:<22} QUERY: {}",
                        ioctl_try(h, FSCTL_QUERY_USN_JOURNAL, &[])
                    );
                    println!(
                        "  {root} dir, {label:<22} ENUM_USN_DATA (every file on the volume): {}",
                        ioctl_try(h, FSCTL_ENUM_USN_DATA, &enum_in)
                    );
                    if let Some((id, next, max)) = journal_state(h) {
                        let mut req = Vec::new();
                        req.extend_from_slice(&(next as i64 - 4096).max(0).to_le_bytes()); // StartUsn
                        req.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes()); // ReasonMask: everything
                        req.extend_from_slice(&0u32.to_le_bytes()); // ReturnOnlyOnClose
                        req.extend_from_slice(&0u64.to_le_bytes()); // Timeout
                        req.extend_from_slice(&0u64.to_le_bytes()); // BytesToWaitFor
                        req.extend_from_slice(&id.to_le_bytes()); // UsnJournalID
                        println!(
                            "  {root} dir, {label:<22} READ_USN_JOURNAL (the change records): {}",
                            ioctl_try(h, FSCTL_READ_USN_JOURNAL, &req)
                        );
                        if label == "FILE_READ_ATTRIBUTES" {
                            thread::sleep(Duration::from_secs(5));
                            if let Some((_, next2, _)) = journal_state(h) {
                                println!(
                                    "  {root} journal id {id:#x}, size cap {} MB, NextUsn advanced {} bytes in 5 s",
                                    max / (1024 * 1024),
                                    next2.saturating_sub(next)
                                );
                            }
                        }
                    }
                    // SAFETY: `h` was opened above and is closed once.
                    unsafe {
                        let _ = CloseHandle(h);
                    }
                }
            }
        }
    }
    // Per-file USN data: the last change record of one file the caller can open.
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    match open_dir_or_file(&exe, FILE_READ_ATTRIBUTES.0) {
        Err(e) => println!("  own exe, FILE_READ_ATTRIBUTES open FAILED: {e}"),
        Ok(h) => {
            let input = [0u8; 4]; // READ_FILE_USN_DATA: MinMajorVersion 0, MaxMajorVersion 0 (two u16)
            println!(
                "  one ordinary file, READ_FILE_USN_DATA (per-file only): {}",
                ioctl_try(h, FSCTL_READ_FILE_USN_DATA, &input)
            );
            // SAFETY: closed once.
            unsafe {
                let _ = CloseHandle(h);
            }
        }
    }
}

// ---------------------------------------------------------------------------------
// ReadDirectoryChangesW watcher
// ---------------------------------------------------------------------------------

#[derive(Default)]
struct Stats {
    events: AtomicU64,
    batches: AtomicU64,
    overflows: AtomicU64,
    bytes: AtomicU64,
}

struct Watch {
    handle: isize,
    thread: Option<thread::JoinHandle<f64>>,
    stats: Arc<Stats>,
    /// Event paths, relative to the watched folder, when `collect` is on.
    names: Arc<std::sync::Mutex<Vec<String>>>,
}

/// Watch `dir` recursively with a buffer of `buffer_bytes`. Each batch is followed by
/// `consumer_delay`, standing in for the work a real consumer does per batch.
fn watch(dir: &Path, buffer_bytes: u32, consumer_delay: Duration, collect: bool) -> Watch {
    let w = wide(&dir.display().to_string());
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
    .expect("open watched folder");
    let raw = handle.0 as isize;
    let stats = Arc::new(Stats::default());
    let names = Arc::new(std::sync::Mutex::new(Vec::new()));
    let (s, n) = (stats.clone(), names.clone());
    let thread = thread::spawn(move || {
        let handle = HANDLE(raw as *mut c_void);
        let filter = FILE_NOTIFY_CHANGE_FILE_NAME
            | FILE_NOTIFY_CHANGE_DIR_NAME
            | FILE_NOTIFY_CHANGE_ATTRIBUTES
            | FILE_NOTIFY_CHANGE_SIZE
            | FILE_NOTIFY_CHANGE_LAST_WRITE
            | FILE_NOTIFY_CHANGE_CREATION;
        // u64 elements keep the buffer aligned for FILE_NOTIFY_INFORMATION (DWORD).
        let mut buf = vec![0u64; buffer_bytes as usize / 8];
        loop {
            let mut got = 0u32;
            // SAFETY: `buf` and `got` outlive this synchronous call; `handle` stays open
            // until the owner cancels and joins this thread.
            let r = unsafe {
                ReadDirectoryChangesW(
                    handle,
                    buf.as_mut_ptr().cast::<c_void>(),
                    buffer_bytes,
                    true,
                    filter,
                    Some(&mut got),
                    None,
                    None,
                )
            };
            if r.is_err() {
                break; // cancelled
            }
            if got == 0 {
                // The buffer overflowed: events were lost and a rescan would be needed.
                s.overflows.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            s.batches.fetch_add(1, Ordering::Relaxed);
            s.bytes.fetch_add(u64::from(got), Ordering::Relaxed);
            let bytes: &[u8] =
                // SAFETY: the kernel wrote `got` initialized bytes at the start of `buf`.
                unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), got as usize) };
            let mut off = 0usize;
            loop {
                let next =
                    u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap_or([0; 4])) as usize;
                let len = u32::from_le_bytes(bytes[off + 8..off + 12].try_into().unwrap_or([0; 4]))
                    as usize;
                s.events.fetch_add(1, Ordering::Relaxed);
                if collect {
                    let units: Vec<u16> = bytes[off + 12..off + 12 + len]
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|c| u16::from_le_bytes(*c))
                        .collect();
                    if let Ok(mut v) = n.lock() {
                        v.push(String::from_utf16_lossy(&units));
                    }
                }
                if next == 0 {
                    break;
                }
                off += next;
            }
            if !consumer_delay.is_zero() {
                thread::sleep(consumer_delay);
            }
        }
        // CPU time this thread used: what watching costs.
        let (mut c, mut e, mut k, mut u) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        // SAFETY: pointers are to locals; the pseudo-handle is valid for this thread.
        unsafe {
            let _ = GetThreadTimes(GetCurrentThread(), &mut c, &mut e, &mut k, &mut u);
        }
        filetime_ms(k) + filetime_ms(u)
    });
    Watch {
        handle: raw,
        thread: Some(thread),
        stats,
        names,
    }
}

impl Watch {
    /// Stop watching; returns the watcher thread's CPU time in milliseconds.
    fn stop(mut self) -> f64 {
        let handle = HANDLE(self.handle as *mut c_void);
        // SAFETY: cancels the pending call on a handle this struct owns.
        unsafe {
            let _ = CancelIoEx(handle, None);
        }
        let cpu = self.thread.take().map_or(0.0, |t| t.join().unwrap_or(0.0));
        // SAFETY: the thread has exited, so nothing uses the handle; closed once.
        unsafe {
            let _ = CloseHandle(handle);
        }
        cpu
    }
}

// ---------------------------------------------------------------------------------
// Experiment 2: a heavy burst of file operations against different buffers/consumers
// ---------------------------------------------------------------------------------

/// Create, rewrite and delete `files` small files under `dir`, in 50 subfolders.
fn churn(dir: &Path, files: u32) -> u64 {
    let mut ops = 0u64;
    for d in 0..50 {
        let _ = std::fs::create_dir_all(dir.join(format!("d{d}")));
        ops += 1;
    }
    for i in 0..files {
        let p = dir.join(format!("d{}", i % 50)).join(format!("f{i}.tmp"));
        if std::fs::write(&p, b"hello").is_ok() {
            ops += 1;
        }
    }
    for i in 0..files {
        let p = dir.join(format!("d{}", i % 50)).join(format!("f{i}.tmp"));
        if std::fs::write(&p, b"hello again, longer").is_ok() {
            ops += 1;
        }
    }
    for i in 0..files {
        let p = dir.join(format!("d{}", i % 50)).join(format!("f{i}.tmp"));
        if std::fs::remove_file(&p).is_ok() {
            ops += 1;
        }
    }
    ops
}

fn burst(files: u32) {
    println!("Burst: {files} files created, rewritten, then deleted (3 operations each).");
    println!(
        "{:<10} {:<14} {:>8} {:>9} {:>10} {:>11} {:>9}",
        "buffer", "consumer", "events", "batches", "overflows", "reader CPU", "wall"
    );
    for (buf_kb, delay_ms) in [
        (4u32, 20u64),
        (64, 200),
        (64, 1000),
        (512, 200),
        (512, 1000),
        (1024, 1000),
    ] {
        let tmp = tempdir();
        let w = watch(&tmp, buf_kb * 1024, Duration::from_millis(delay_ms), false);
        let t = Instant::now();
        let ops = churn(&tmp, files);
        // Let the watcher drain.
        thread::sleep(Duration::from_millis(1500));
        let wall = t.elapsed();
        let (events, batches, overflows) = (
            w.stats.events.load(Ordering::Relaxed),
            w.stats.batches.load(Ordering::Relaxed),
            w.stats.overflows.load(Ordering::Relaxed),
        );
        let cpu = w.stop();
        let _ = std::fs::remove_dir_all(&tmp);
        println!(
            "{:<10} {:<14} {:>8} {:>9} {:>10} {:>9.0} ms {:>7.1} s   ({ops} fs operations)",
            format!("{buf_kb} KB"),
            if delay_ms == 0 {
                "none".into()
            } else {
                format!("{delay_ms} ms/batch")
            },
            events,
            batches,
            overflows,
            cpu,
            wall.as_secs_f64()
        );
    }
}

fn tempdir() -> PathBuf {
    let p = std::env::temp_dir().join(format!("sentinel-spike-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).expect("create temp folder");
    p
}

// ---------------------------------------------------------------------------------
// Experiment 3: how noisy is a real folder when nothing special is happening?
// ---------------------------------------------------------------------------------

fn ambient(dir: &Path, seconds: u64) {
    println!(
        "Ambient: watching {} for {seconds} s with a 64 KB buffer...",
        dir.display()
    );
    let w = watch(dir, 64 * 1024, Duration::ZERO, true);
    let stop = Arc::new(AtomicBool::new(false));
    let _ = &stop;
    thread::sleep(Duration::from_secs(seconds));
    let (events, overflows) = (
        w.stats.events.load(Ordering::Relaxed),
        w.stats.overflows.load(Ordering::Relaxed),
    );
    let names: Vec<String> = w.names.lock().map(|v| v.clone()).unwrap_or_default();
    let cpu = w.stop();
    println!(
        "{events} events ({:.1}/s), {overflows} overflows, watcher CPU {cpu:.0} ms",
        events as f64 / seconds as f64
    );
    let mut by_dir: HashMap<String, u64> = HashMap::new();
    for n in &names {
        let top: Vec<&str> = n.split('\\').take(3).collect();
        *by_dir.entry(top.join("\\")).or_default() += 1;
    }
    let mut v: Vec<_> = by_dir.into_iter().collect();
    v.sort_by_key(|e| std::cmp::Reverse(e.1));
    println!("Noisiest places (first three path components):");
    for (d, c) in v.iter().take(8) {
        println!("  {c:>6}  {d}");
    }
}
