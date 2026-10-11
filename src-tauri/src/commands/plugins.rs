//! Plugin commands. A plugin can only describe cleanup candidates (docs/PLUGINS.md); these
//! commands inspect, approve, switch and remove them. Nothing here moves a file.

use std::path::PathBuf;

use sentinel_plugins::AnchorPaths;
use tauri::State;

use super::error::{CommandError, ErrorKind};
use crate::AppState;
use crate::db::now_ms;
use crate::plugins::{self, Inspection, PluginSummary};

#[tauri::command]
pub(crate) fn plugins_list(state: State<'_, AppState>) -> Result<Vec<PluginSummary>, CommandError> {
    plugins::list(&state.db, &AnchorPaths::for_system())
}

/// Read a plugin file and show what it would do right now. Stores nothing.
#[tauri::command]
pub(crate) async fn plugin_inspect(
    state: State<'_, AppState>,
    file: String,
) -> Result<Inspection, CommandError> {
    let path = PathBuf::from(&file);
    if !path.is_absolute() {
        return Err(CommandError::new(
            ErrorKind::InvalidInput,
            "Choose a plugin file with the file picker.",
        ));
    }
    let (db, pending, policy) = (
        state.db.clone(),
        state.pending_plugins.clone(),
        state.policy.clone(),
    );
    tauri::async_runtime::spawn_blocking(move || {
        plugins::inspect(
            &db,
            &pending,
            &policy,
            &AnchorPaths::for_system(),
            &path,
            now_ms(),
        )
    })
    .await
    .map_err(|e| CommandError::internal("inspecting the plugin", e))?
}

/// Approve a plugin that was inspected in this session, by its fingerprint.
#[tauri::command]
pub(crate) fn plugin_install(
    state: State<'_, AppState>,
    sha256: String,
) -> Result<PluginSummary, CommandError> {
    plugins::install(
        &state.db,
        &state.pending_plugins,
        &AnchorPaths::for_system(),
        &state.user,
        &sha256,
        now_ms(),
    )
}

#[tauri::command]
pub(crate) fn plugin_set_enabled(
    state: State<'_, AppState>,
    id: String,
    enabled: bool,
) -> Result<(), CommandError> {
    plugins::set_enabled(&state.db, &state.user, &id, enabled, now_ms())
}

#[tauri::command]
pub(crate) fn plugin_remove(state: State<'_, AppState>, id: String) -> Result<(), CommandError> {
    plugins::remove(&state.db, &state.user, &id, now_ms())
}
