//! The optional AI assistant. Off by default; local models only (docs/AI_POLICY.md).

use sentinel_ai::{RedactionContext, Redactor};
use sentinel_store::ScanId;
use tauri::{AppHandle, Manager, State};

use super::error::CommandError;
use crate::AppState;
use crate::ai::{self, AiSettings, AskResponse};

#[tauri::command]
pub(crate) fn ai_settings(state: State<'_, AppState>) -> Result<AiSettings, CommandError> {
    ai::load(&state.db)
}

/// Save the settings. A model address that is not on this computer is refused.
#[tauri::command]
pub(crate) fn set_ai_settings(
    state: State<'_, AppState>,
    settings: AiSettings,
) -> Result<AiSettings, CommandError> {
    ai::save(&state.db, &settings)
}

/// Translate a typed question into a query with the local model, then run that query
/// read-only against a saved analysis (the latest by default).
#[tauri::command]
pub(crate) async fn ask_assistant(
    app: AppHandle,
    question: String,
    scan_id: Option<ScanId>,
) -> Result<AskResponse, CommandError> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let settings = ai::load(&state.db)?;
        let provider = ai::provider(&settings)?;
        let mut redactor = Redactor::new(RedactionContext::from_env());
        ai::ask(
            &state.db,
            &|p| state.scans.classify(p),
            &settings,
            &provider,
            &mut redactor,
            scan_id,
            &question,
        )
    })
    .await
    .map_err(|e| CommandError::internal("asking the assistant", e))?
}
