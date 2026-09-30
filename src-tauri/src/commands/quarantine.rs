//! Cleanup execution and quarantine commands: the only commands that change files.
//!
//! `cleanup_run` moves items the user approved from a preview into quarantine; the
//! executor re-checks every item before moving it and writes audit records first.

use sentinel_cleanup::providers;
use sentinel_quarantine::{Context, Manifest, ManifestEntry, Quarantine, QuarantineError};
use sentinel_safety::Policy;
use serde::Serialize;
use tauri::State;
use ts_rs::TS;

use super::error::{CommandError, ErrorKind};
use crate::db::{Db, now_ms};
use crate::{AppState, audit};

/// Most items one operation may move; far above any real preview.
const MAX_ITEMS: usize = 10_000;

impl From<QuarantineError> for CommandError {
    fn from(err: QuarantineError) -> Self {
        let kind = match err {
            QuarantineError::Refused(_) => ErrorKind::Refused,
            QuarantineError::NotFound(_) => ErrorKind::NotFound,
            _ => ErrorKind::System,
        };
        CommandError::new(kind, err.to_string())
    }
}

fn quarantine(state: &AppState) -> Result<Quarantine, CommandError> {
    state.quarantine.clone().ok_or_else(|| {
        CommandError::new(
            ErrorKind::System,
            "There is no quarantine folder for this account, so nothing can be moved.",
        )
    })
}

fn db_sink(db: &Db) -> impl FnMut(sentinel_store::NewAuditRecord) -> Result<i64, String> + '_ {
    move |r| audit::record(db, &r).map_err(|e| e.to_string())
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CleanupRunResponse {
    pub operation_id: String,
    pub manifest: Manifest,
}

fn run(
    q: &Quarantine,
    policy: &Policy,
    db: &Db,
    user: &str,
    provider_id: &str,
    approved: &[String],
) -> Result<CleanupRunResponse, CommandError> {
    if approved.is_empty() || approved.len() > MAX_ITEMS {
        return Err(CommandError::new(
            ErrorKind::InvalidInput,
            "Choose between 1 and 10,000 items to move.",
        ));
    }
    let all = providers::builtin();
    let provider = all
        .iter()
        .find(|p| p.info().id == provider_id)
        .ok_or_else(|| {
            CommandError::new(
                ErrorKind::NotFound,
                format!("There is no cleanup type called {provider_id}."),
            )
        })?;
    let paths: Vec<std::path::PathBuf> = approved.iter().map(std::path::PathBuf::from).collect();
    let operation_id = audit::new_operation_id();
    let ctx = Context {
        operation_id: &operation_id,
        user,
        now_ms: now_ms(),
    };
    let manifest = q.quarantine(policy, provider.as_ref(), &paths, &ctx, &mut db_sink(db))?;
    Ok(CleanupRunResponse {
        operation_id,
        manifest,
    })
}

/// Move items approved from a preview into quarantine.
#[tauri::command]
pub(crate) async fn cleanup_run(
    state: State<'_, AppState>,
    provider: String,
    approved: Vec<String>,
) -> Result<CleanupRunResponse, CommandError> {
    let q = quarantine(&state)?;
    let (policy, db, user) = (state.policy.clone(), state.db.clone(), state.user.clone());
    tauri::async_runtime::spawn_blocking(move || run(&q, &policy, &db, &user, &provider, &approved))
        .await
        .map_err(|e| CommandError::internal("moving items to quarantine", e))?
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct QuarantineContents {
    pub root: String,
    /// Newest first.
    pub operations: Vec<Manifest>,
    /// Operation folders whose record could not be read.
    pub problems: Vec<String>,
}

#[tauri::command]
pub(crate) fn quarantine_contents(
    state: State<'_, AppState>,
) -> Result<QuarantineContents, CommandError> {
    let q = quarantine(&state)?;
    let (operations, problems) = q.operations()?;
    Ok(QuarantineContents {
        root: q.root().display().to_string(),
        operations,
        problems,
    })
}

/// Put one quarantined item back. Never overwrites.
#[tauri::command]
pub(crate) async fn quarantine_restore(
    state: State<'_, AppState>,
    operation_id: String,
    index: u32,
) -> Result<ManifestEntry, CommandError> {
    let q = quarantine(&state)?;
    let (policy, db, user) = (state.policy.clone(), state.db.clone(), state.user.clone());
    tauri::async_runtime::spawn_blocking(move || {
        let op = audit::new_operation_id();
        let ctx = Context {
            operation_id: &op,
            user: &user,
            now_ms: now_ms(),
        };
        q.restore(&policy, &operation_id, index, &ctx, &mut db_sink(&db))
            .map_err(CommandError::from)
    })
    .await
    .map_err(|e| CommandError::internal("restoring from quarantine", e))?
}

/// Remove expired quarantine operations. Runs once in the background at startup.
pub(crate) fn purge_expired_in_background(q: Quarantine, db: Db, user: String) {
    let spawned = std::thread::Builder::new()
        .name("sentinel-quarantine-purge".into())
        .spawn(move || {
            let op = audit::new_operation_id();
            let ctx = Context {
                operation_id: &op,
                user: &user,
                now_ms: now_ms(),
            };
            match q.purge_expired(&ctx, &mut db_sink(&db)) {
                Ok(r) => tracing::info!(
                    operations = r.operations_purged,
                    entries = r.entries_purged,
                    bytes = r.bytes_purged,
                    errors = r.errors.len(),
                    "expired quarantine purged"
                ),
                Err(e) => tracing::error!(error = %e, "quarantine purge failed"),
            }
        });
    if let Err(e) = spawned {
        tracing::error!(error = %e, "could not start quarantine purge");
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn rejects_empty_oversized_and_unknown_requests_before_touching_anything() {
        let dir = tempfile::tempdir().unwrap();
        let q = Quarantine::at(dir.path().join("q"));
        let db = Db::in_memory();
        let policy = Policy::new(sentinel_safety::ProtectedSet::new());
        let err = run(&q, &policy, &db, "t", "user-temp", &[]).unwrap_err();
        assert_eq!(err.kind, ErrorKind::InvalidInput);
        let many = vec!["x".to_owned(); MAX_ITEMS + 1];
        assert_eq!(
            run(&q, &policy, &db, "t", "user-temp", &many)
                .unwrap_err()
                .kind,
            ErrorKind::InvalidInput
        );
        let err = run(&q, &policy, &db, "t", "nope", &["x".into()]).unwrap_err();
        assert_eq!(err.kind, ErrorKind::NotFound);
        assert!(!dir.path().join("q").exists());
        assert!(db.lock().audit_records(10, None).unwrap().is_empty());
    }

    #[test]
    fn records_started_and_outcome_for_a_run_that_moves_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let q = Quarantine::at(dir.path().join("q"));
        let db = Db::in_memory();
        let policy = Policy::new(sentinel_safety::ProtectedSet::new());
        // Not inside the user-temp provider's folder: skipped, never touched.
        let outside = dir.path().join("keep.txt");
        std::fs::write(&outside, b"x").unwrap();
        let r = run(
            &q,
            &policy,
            &db,
            "tester",
            "user-temp",
            &[outside.display().to_string()],
        )
        .unwrap();
        assert!(matches!(
            r.manifest.entries[0].status,
            sentinel_quarantine::EntryStatus::Skipped { .. }
        ));
        assert!(outside.exists());
        let recs = db.lock().audit_records(10, None).unwrap();
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[1].outcome, sentinel_store::Outcome::Started);
        assert_eq!(recs[0].outcome, sentinel_store::Outcome::NoChanges);
        assert_eq!(recs[0].user, "tester");
    }
}
