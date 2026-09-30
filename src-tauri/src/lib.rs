//! Sentinel desktop application.
//!
//! This crate is deliberately thin: it wires Tauri to the core crates, owns
//! application state and exposes typed IPC commands. Business logic lives in the
//! `sentinel-*` crates.

mod commands;
mod logging;

use std::path::PathBuf;

use sentinel_safety::Policy;
use tauri::Manager;

/// Process-wide state shared with IPC commands.
pub(crate) struct AppState {
    pub log_dir: PathBuf,
    pub policy: Policy,
}

pub fn run() {
    let result = tauri::Builder::default()
        .setup(|app| {
            let log_dir = app.path().app_log_dir()?;
            let guard = logging::init(&log_dir)?;
            app.manage(guard);
            app.manage(AppState {
                log_dir,
                policy: Policy::for_system(),
            });
            tracing::info!(version = env!("CARGO_PKG_VERSION"), "Sentinel started");
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::system::app_info,
            commands::safety::protected_locations,
        ])
        .run(tauri::generate_context!());

    if let Err(err) = result {
        tracing::error!(error = %err, "Sentinel failed to run");
        eprintln!("Sentinel failed to run: {err}");
        std::process::exit(1);
    }
}
