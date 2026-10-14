//! Preferences commands.

use std::sync::atomic::Ordering;

use tauri::{AppHandle, State};

use super::error::{CommandError, ErrorKind};
use crate::prefs::{self, Preferences};
use crate::{AppState, background, startup};

fn system_error(message: String) -> CommandError {
    CommandError::new(ErrorKind::System, message)
}

/// The stored preferences, with the sign-in entry read from Windows itself.
#[tauri::command]
pub(crate) fn preferences(state: State<'_, AppState>) -> Result<Preferences, CommandError> {
    let stored = prefs::load(&state.db)?;
    Ok(Preferences {
        start_with_windows: stored.run_in_background && startup::is_enabled(),
        ..stored
    }
    .normalized())
}

/// Save preferences and apply them now: the tray icon, the sign-in entry and the close
/// behaviour. If any part cannot be applied the whole change is refused, so what is saved is
/// always what is in effect.
#[tauri::command]
pub(crate) fn set_preferences(
    app: AppHandle,
    state: State<'_, AppState>,
    preferences: Preferences,
) -> Result<Preferences, CommandError> {
    let wanted = preferences.normalized();
    let before = prefs::load(&state.db)?;

    // The tray first: if Windows cannot create it, closing the window must not start hiding it.
    background::apply(&app, wanted.run_in_background).map_err(system_error)?;
    if let Err(message) = startup::set_enabled(wanted.start_with_windows) {
        // Put the tray back as it was before reporting the failure.
        let _ = background::apply(&app, before.run_in_background);
        return Err(system_error(message));
    }
    prefs::save(&state.db, wanted)?;
    state
        .background
        .store(wanted.run_in_background, Ordering::SeqCst);
    tracing::info!(
        background = wanted.run_in_background,
        sign_in = wanted.start_with_windows,
        alerts = wanted.low_space_alerts,
        "preferences saved"
    );
    Ok(wanted)
}
