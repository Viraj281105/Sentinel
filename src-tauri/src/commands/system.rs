use serde::Serialize;
use tauri::State;
use ts_rs::TS;

use crate::AppState;
use crate::db::Db;

/// Basic facts about the running application.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AppInfo {
    pub version: String,
    pub debug_build: bool,
    pub log_dir: String,
    /// Where analyses and history are stored; `null` when running without a database file.
    pub database_path: Option<String>,
    /// Why the database file could not be used; history is then lost on exit.
    pub database_error: Option<String>,
}

#[tauri::command]
pub(crate) fn app_info(state: State<'_, AppState>) -> AppInfo {
    build_app_info(&state.log_dir, &state.db)
}

fn build_app_info(log_dir: &std::path::Path, db: &Db) -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        debug_build: cfg!(debug_assertions),
        log_dir: log_dir.display().to_string(),
        database_path: db.path.as_ref().map(|p| p.display().to_string()),
        database_error: db.error.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_version_log_dir_and_database_state() {
        let info = build_app_info(std::path::Path::new(r"C:\logs"), &Db::in_memory());
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(info.log_dir, r"C:\logs");
        assert!(info.database_path.is_none());
        assert!(info.database_error.is_none());
    }

    #[test]
    fn falls_back_to_memory_when_the_database_file_is_unusable() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        // A directory where the database file should be makes opening it fail.
        std::fs::create_dir(dir.path().join(crate::db::DB_FILE)).unwrap_or_else(|e| panic!("{e}"));
        let db = Db::open(dir.path()).unwrap_or_else(|e| panic!("{e}"));
        let info = build_app_info(dir.path(), &db);
        assert!(info.database_path.is_none());
        assert!(info.database_error.is_some());
    }
}
