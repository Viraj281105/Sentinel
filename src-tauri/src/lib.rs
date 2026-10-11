//! Sentinel desktop application.
//!
//! This crate is deliberately thin: it wires Tauri to the core crates, owns
//! application state and exposes typed IPC commands. Business logic lives in the
//! `sentinel-*` crates.

mod ai;
mod anomaly;
mod audit;
mod commands;
mod db;
mod history;
mod logging;
mod lowspace;
mod plugins;
mod prefs;
mod query;
mod recovery;
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
    /// Plugin files inspected this session, waiting for approval.
    pub pending_plugins: std::sync::Arc<plugins::Pending>,
    /// The user's quarantine; `None` if the profile folder cannot be located.
    pub quarantine: Option<sentinel_quarantine::Quarantine>,
}

/// How often drives are checked for low space while Sentinel is running.
const LOW_SPACE_CHECK_EVERY: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// Check the drives now and then and show a notification when one becomes low, if the
/// person turned that on. Reads only drive capacity; the text names no folder or file.
fn start_low_space_alerts(app: tauri::AppHandle) {
    use tauri_winrt_notification::Toast;
    // An installed app is identified by its bundle identifier. A development build is not
    // registered with Windows, so it borrows PowerShell's identity to be allowed to toast.
    let app_id = if cfg!(debug_assertions) {
        Toast::POWERSHELL_APP_ID.to_owned()
    } else {
        app.config().identifier.clone()
    };
    let spawned = std::thread::Builder::new()
        .name("sentinel-low-space".into())
        .spawn(move || {
            let mut tracker = lowspace::LowSpaceTracker::default();
            loop {
                let state = app.state::<AppState>();
                let enabled = prefs::load(&state.db).is_ok_and(|p| p.low_space_alerts);
                if enabled {
                    match sentinel_scanner::drives::list_drives() {
                        Ok(drives) => lowspace::check(true, &drives, &mut tracker, &mut |title, body| {
                            if let Err(err) = Toast::new(&app_id).title(title).text1(body).show() {
                                tracing::warn!(error = %err, "could not show the low-space notification");
                            }
                        }),
                        Err(err) => tracing::warn!(error = %err, "low-space check could not list drives"),
                    }
                }
                std::thread::sleep(LOW_SPACE_CHECK_EVERY);
            }
        });
    if let Err(err) = spawned {
        tracing::warn!(error = %err, "could not start the low-space check");
    }
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
            let snapshot_file = app.path().app_local_data_dir()?.join("scan-tree.bin");
            app.manage(AppState {
                log_dir,
                policy: Policy::for_system(),
                scans: scans::ScanManager::new(
                    db.clone(),
                    std::sync::Arc::new(sentinel_classify::Classifier::for_system()),
                )
                .with_snapshot_file(snapshot_file),
                db,
                user,
                quarantine,
                pending_plugins: std::sync::Arc::default(),
            });
            app.state::<AppState>().scans.resume_watch();
            start_low_space_alerts(app.handle().clone());
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
            commands::scan::update_scan,
            commands::scan::watch_status,
            commands::history::growth_history,
            commands::history::folder_history,
            commands::history::anomaly_report,
            commands::query::query_saved_analysis,
            commands::plugins::plugins_list,
            commands::plugins::plugin_inspect,
            commands::plugins::plugin_install,
            commands::plugins::plugin_set_enabled,
            commands::plugins::plugin_remove,
            commands::prefs::preferences,
            commands::prefs::set_preferences,
            commands::ai::ai_settings,
            commands::ai::set_ai_settings,
            commands::ai::ask_assistant,
            commands::scan::scan_listing,
            commands::scan::scan_largest_files,
            commands::cleanup::cleanup_providers,
            commands::cleanup::cleanup_preview,
            commands::activity::audit_log,
            commands::activity::audit_verify,
            commands::projects::project_searches,
            commands::projects::find_projects,
            commands::projects::remove_project_search,
            commands::projects::jvm_caches,
            commands::projects::runtime_inventory,
            commands::projects::dependency_graph,
            commands::software::software_inventory,
            commands::containers::docker_overview,
            commands::containers::wsl_distros,
            commands::quarantine::cleanup_run,
            commands::quarantine::project_cleanup_run,
            commands::cleanup::project_cleanup_preview,
            commands::recovery::plan_recovery,
            commands::recovery::run_recovery_plan,
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
