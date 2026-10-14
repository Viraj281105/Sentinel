//! Small user preferences. Everything here is off until a person turns it on.
//!
//! Two are kept in the local database. `start_with_windows` is not: the registry's Run entry is
//! the source of truth for it (a person can also remove it in Windows' Startup apps list), so it
//! is read from there by the command layer.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::commands::error::CommandError;
use crate::db::Db;

const KEY_LOW_SPACE_ALERTS: &str = "notify.lowSpace";
const KEY_RUN_IN_BACKGROUND: &str = "background.run";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Preferences {
    /// Show a Windows notification when a drive becomes low on space (while Sentinel runs).
    pub low_space_alerts: bool,
    /// Keep running in the notification area: closing the window hides it instead of quitting.
    pub run_in_background: bool,
    /// Start Sentinel, hidden in the notification area, when this person signs in. Needs
    /// `run_in_background`, so Sentinel can never start with no way to open it.
    pub start_with_windows: bool,
}

impl Preferences {
    /// The combination that is actually allowed: starting at sign-in needs background mode.
    pub fn normalized(mut self) -> Self {
        if !self.run_in_background {
            self.start_with_windows = false;
        }
        self
    }
}

/// The stored preferences. `start_with_windows` is always false here; see the module note.
pub(crate) fn load(db: &Db) -> Result<Preferences, CommandError> {
    let store = db.lock();
    let on =
        |key| -> Result<bool, CommandError> { Ok(store.setting(key)?.as_deref() == Some("true")) };
    Ok(Preferences {
        low_space_alerts: on(KEY_LOW_SPACE_ALERTS)?,
        run_in_background: on(KEY_RUN_IN_BACKGROUND)?,
        start_with_windows: false,
    })
}

pub(crate) fn save(db: &Db, prefs: Preferences) -> Result<(), CommandError> {
    let text = |b: bool| if b { "true" } else { "false" };
    let store = db.lock();
    store.set_setting(KEY_LOW_SPACE_ALERTS, text(prefs.low_space_alerts))?;
    store.set_setting(KEY_RUN_IN_BACKGROUND, text(prefs.run_in_background))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn everything_is_off_by_default_and_changes_are_kept() {
        let db = Db::in_memory();
        assert_eq!(load(&db).unwrap(), Preferences::default());
        save(
            &db,
            Preferences {
                low_space_alerts: true,
                run_in_background: true,
                start_with_windows: true,
            },
        )
        .unwrap();
        let p = load(&db).unwrap();
        assert!(p.low_space_alerts && p.run_in_background);
        assert!(
            !p.start_with_windows,
            "that one lives in the registry, not here"
        );
        save(&db, Preferences::default()).unwrap();
        assert_eq!(load(&db).unwrap(), Preferences::default());
    }

    #[test]
    fn starting_at_sign_in_is_only_allowed_with_background_mode() {
        let wants = Preferences {
            low_space_alerts: false,
            run_in_background: false,
            start_with_windows: true,
        };
        assert!(!wants.normalized().start_with_windows);
        let ok = Preferences {
            run_in_background: true,
            ..wants
        };
        assert!(ok.normalized().start_with_windows);
    }
}
