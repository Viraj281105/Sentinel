//! The app's local database handle.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use sentinel_store::Store;

use crate::commands::error::{CommandError, ErrorKind};

pub(crate) const DB_FILE: &str = "sentinel.db";

/// The store plus where it lives. Shared by commands and the scan thread.
#[derive(Clone)]
pub(crate) struct Db {
    store: Arc<Mutex<Store>>,
    /// `None` when running on the in-memory fallback.
    pub path: Option<PathBuf>,
    /// Why the on-disk database could not be used, if it could not.
    pub error: Option<String>,
}

impl Db {
    /// Open the database in `dir`. If that fails, fall back to an in-memory database so
    /// the app keeps working, and record why history will not be kept.
    pub(crate) fn open(dir: &Path) -> Result<Self, sentinel_store::StoreError> {
        let path = dir.join(DB_FILE);
        match Store::open(&path) {
            Ok(store) => Ok(Self::wrap(store, Some(path), None)),
            Err(err) => {
                tracing::error!(path = %path.display(), error = %err, "database unavailable; using memory");
                Ok(Self::wrap(
                    Store::open_in_memory()?,
                    None,
                    Some(err.to_string()),
                ))
            }
        }
    }

    pub(crate) fn wrap(store: Store, path: Option<PathBuf>, error: Option<String>) -> Self {
        Self {
            store: Arc::new(Mutex::new(store)),
            path,
            error,
        }
    }

    #[cfg(test)]
    pub(crate) fn in_memory() -> Self {
        #[allow(clippy::expect_used)]
        Self::wrap(
            Store::open_in_memory().expect("in-memory database"),
            None,
            None,
        )
    }

    /// Lock the store. A panic while holding the lock cannot corrupt SQLite (every write
    /// is transactional), so a poisoned lock is recovered.
    pub(crate) fn lock(&self) -> MutexGuard<'_, Store> {
        self.store.lock().unwrap_or_else(|p| p.into_inner())
    }
}

impl From<sentinel_store::StoreError> for CommandError {
    fn from(err: sentinel_store::StoreError) -> Self {
        CommandError::new(
            ErrorKind::System,
            format!("Sentinel's local database failed: {err}"),
        )
    }
}

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}
