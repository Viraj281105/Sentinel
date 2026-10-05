//! Phase 7 spike: can Sentinel track file changes with the NTFS change journal (USN), or
//! should it use `ReadDirectoryChangesW`? Throwaway measurement code; never shipped.
//!
//! ```text
//! cargo run --release -p spike-change-tracking -- usn
//! cargo run --release -p spike-change-tracking -- burst [files]
//! cargo run --release -p spike-change-tracking -- ambient <folder> [seconds]
//! ```
//!
//! Nothing is written outside a temporary folder created for `burst`.

// Throwaway measurement code: failing loudly is the right behavior here.
#![allow(clippy::expect_used, clippy::unwrap_used)]

#[cfg(windows)]
mod win;

#[cfg(windows)]
fn main() {
    win::run();
}

#[cfg(not(windows))]
fn main() {
    eprintln!("This spike only runs on Windows.");
}
