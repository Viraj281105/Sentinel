//! Sentinel desktop application.
//!
//! This crate is deliberately thin: it wires Tauri to the core crates, owns
//! application state and exposes typed IPC commands. Business logic lives in the
//! `sentinel-*` crates.

mod audit;
mod commands;
mod db;
mod logging;
mod scans;

use std::path::PathBuf;

use sentinel_safety::Policy;
use tauri::Manager;

/// Process-wide state shared with IPC commands.
pub(crate) struct AppState {
    pub log_dir: PathBuf,
    pub policy: Policy,
    pub db: db::Db,
    /// Windows account name, recorded in audit entries.
    pub user: String,
    pub scans: scans::ScanManager,
    /// The user's quarantine; `None` if the profile folder cannot be located.
    pub quarantine: Option<sentinel_quarantine::Quarantine>,
}

pub fn run() {
    let result = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let log_dir = app.path().app_log_dir()?;
            let guard = logging::init(&log_dir)?;
            app.manage(guard);
            let db = db::Db::open(&app.path().app_local_data_dir()?)?;
            let user = audit::current_user();
            let quarantine = sentinel_quarantine::Quarantine::for_current_user();
            if let Some(q) = &quarantine {
                commands::quarantine::purge_expired_in_background(
                    q.clone(),
                    db.clone(),
                    user.clone(),
                );
            }
            app.manage(AppState {
                log_dir,
                policy: Policy::for_system(),
                scans: scans::ScanManager::new(
                    db.clone(),
                    std::sync::Arc::new(sentinel_classify::Classifier::for_system()),
                ),
                db,
                user,
                quarantine,
            });
            tracing::info!(version = env!("CARGO_PKG_VERSION"), "Sentinel started");
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::system::app_info,
            commands::safety::protected_locations,
            commands::storage::list_drives,
            commands::storage::drive_trends,
            commands::scan::start_scan,
            commands::scan::cancel_scan,
            commands::scan::scan_status,
            commands::scan::scan_listing,
            commands::scan::scan_largest_files,
            commands::cleanup::cleanup_providers,
            commands::cleanup::cleanup_preview,
            commands::activity::audit_log,
            commands::activity::audit_verify,
            commands::projects::project_searches,
            commands::projects::find_projects,
            commands::projects::remove_project_search,
            commands::quarantine::cleanup_run,
            commands::quarantine::quarantine_contents,
            commands::quarantine::quarantine_restore,
        ])
        .run(tauri::generate_context!());

    if let Err(err) = result {
        tracing::error!(error = %err, "Sentinel failed to run");
        eprintln!("Sentinel failed to run: {err}");
        std::process::exit(1);
    }
}
