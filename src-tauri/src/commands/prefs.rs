//! Preferences commands.

use tauri::State;

use super::error::CommandError;
use crate::AppState;
use crate::prefs::{self, Preferences};

#[tauri::command]
pub(crate) fn preferences(state: State<'_, AppState>) -> Result<Preferences, CommandError> {
    prefs::load(&state.db)
}

#[tauri::command]
pub(crate) fn set_preferences(
    state: State<'_, AppState>,
    preferences: Preferences,
) -> Result<Preferences, CommandError> {
    prefs::save(&state.db, preferences)
}
