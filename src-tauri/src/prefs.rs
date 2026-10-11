//! Small user preferences, stored in the local database. Everything here is off until a
//! person turns it on.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::commands::error::CommandError;
use crate::db::Db;

const KEY_LOW_SPACE_ALERTS: &str = "notify.lowSpace";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Preferences {
    /// Show a Windows notification when a drive becomes low on space (while Sentinel runs).
    pub low_space_alerts: bool,
}

pub(crate) fn load(db: &Db) -> Result<Preferences, CommandError> {
    let store = db.lock();
    Ok(Preferences {
        low_space_alerts: store.setting(KEY_LOW_SPACE_ALERTS)?.as_deref() == Some("true"),
    })
}

pub(crate) fn save(db: &Db, prefs: Preferences) -> Result<Preferences, CommandError> {
    db.lock().set_setting(
        KEY_LOW_SPACE_ALERTS,
        if prefs.low_space_alerts {
            "true"
        } else {
            "false"
        },
    )?;
    Ok(prefs)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn everything_is_off_by_default_and_changes_are_kept() {
        let db = Db::in_memory();
        assert_eq!(load(&db).unwrap(), Preferences::default());
        assert!(!load(&db).unwrap().low_space_alerts);
        save(
            &db,
            Preferences {
                low_space_alerts: true,
            },
        )
        .unwrap();
        assert!(load(&db).unwrap().low_space_alerts);
        save(&db, Preferences::default()).unwrap();
        assert!(!load(&db).unwrap().low_space_alerts);
    }
}
