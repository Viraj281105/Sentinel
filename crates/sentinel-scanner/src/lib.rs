//! Read-only storage observation for Sentinel: drive discovery and directory scanning.
//! Nothing in this crate modifies the filesystem.

#[cfg(not(windows))]
compile_error!("sentinel-scanner only supports Windows");

pub mod dirent;
pub mod drives;
pub mod scan;
