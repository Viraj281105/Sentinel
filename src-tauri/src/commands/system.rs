use serde::Serialize;
use tauri::State;
use ts_rs::TS;

use crate::AppState;

/// Basic facts about the running application.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AppInfo {
    pub version: String,
    pub debug_build: bool,
    pub log_dir: String,
}

#[tauri::command]
pub(crate) fn app_info(state: State<'_, AppState>) -> AppInfo {
    build_app_info(&state.log_dir)
}

fn build_app_info(log_dir: &std::path::Path) -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        debug_build: cfg!(debug_assertions),
        log_dir: log_dir.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_crate_version_and_log_dir() {
        let info = build_app_info(std::path::Path::new(r"C:\logs"));
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(info.log_dir, r"C:\logs");
    }
}
