//! The recovery planner command. Read-only: it previews and plans, and changes nothing.

use sentinel_cleanup::{Risk, providers};
use tauri::State;

use super::error::CommandError;
use super::projects::saved_searches;
use crate::AppState;
use crate::db::now_ms;
use crate::recovery::{self, RecoveryPlan};

/// Propose which eligible items to remove to free about `target_bytes`, never above
/// `max_risk` (at most medium). The plan is a proposal; items are validated again when
/// someone approves and runs them.
#[tauri::command]
pub(crate) async fn plan_recovery(
    state: State<'_, AppState>,
    target_bytes: u64,
    max_risk: Risk,
) -> Result<RecoveryPlan, CommandError> {
    let policy = state.policy.clone();
    let searches: Vec<_> = saved_searches(&state.db)?
        .into_iter()
        .map(|s| s.detection)
        .collect();
    tauri::async_runtime::spawn_blocking(move || {
        // Always included, so what exists above the ceiling can be reported; the planner
        // enforces the ceiling.
        let project_paths = recovery::project_paths(&searches);
        recovery::build(
            &policy,
            &providers::builtin(),
            &project_paths,
            now_ms(),
            target_bytes,
            max_risk,
        )
    })
    .await
    .map_err(|e| CommandError::internal("planning recovery", e))?
}
