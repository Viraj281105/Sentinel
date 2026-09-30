//! Read-only storage observation for Sentinel: drive discovery today, directory
//! scanning later. Nothing in this crate modifies the filesystem.

#[cfg(not(windows))]
compile_error!("sentinel-scanner only supports Windows");

pub mod drives;
