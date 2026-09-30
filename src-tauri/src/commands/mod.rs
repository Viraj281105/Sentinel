//! Typed IPC commands. Each command is a thin wrapper over a plain function that is
//! unit-tested without a running Tauri app. Types shared with the frontend derive
//! `ts_rs::TS`; `cargo test -p sentinel-app` regenerates `src/bindings`.
//!
//! Commands are registered by full module path because `#[tauri::command]` generates
//! companion items that re-exports do not carry.

pub(crate) mod error;
pub(crate) mod safety;
pub(crate) mod storage;
pub(crate) mod system;
