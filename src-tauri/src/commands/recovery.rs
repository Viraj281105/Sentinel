//! The recovery planner command. Read-only: it previews and plans, and changes nothing.

use sentinel_cleanup::Risk;
use tauri::State;

use super::error::CommandError;
use super::projects::saved_searches;
use super::quarantine::quarantine;
use crate::AppState;
use crate::db::now_ms;
use crate::recovery::{self, ApprovedItem, PlanRunResponse, RecoveryPlan};

/// Propose which eligible items to remove to free about `target_bytes`, never above
/// `max_risk` (at most medium). The plan is a proposal; items are validated again when
/// someone approves and runs them.
#[tauri::command]
pub(crate) async fn plan_recovery(
    state: State<'_, AppState>,
    target_bytes: u64,
    max_risk: Risk,
) -> Result<RecoveryPlan, CommandError> {
    let (policy, db) = (state.policy.clone(), state.db.clone());
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
            &crate::plugins::all_providers(&db),
            &project_paths,
            now_ms(),
            target_bytes,
            max_risk,
        )
    })
    .await
    .map_err(|e| CommandError::internal("planning recovery", e))?
}

/// Move the approved items of a plan into quarantine (never anywhere else). Each item is
/// re-checked just before it moves; anything that changed, gained a protected file or is in
/// use is left in place and reported.
#[tauri::command]
pub(crate) async fn run_recovery_plan(
    state: State<'_, AppState>,
    approved: Vec<ApprovedItem>,
) -> Result<PlanRunResponse, CommandError> {
    let q = quarantine(&state)?;
    let (policy, db, user) = (state.policy.clone(), state.db.clone(), state.user.clone());
    let searches: Vec<_> = saved_searches(&db)?
        .into_iter()
        .map(|s| s.detection)
        .collect();
    tauri::async_runtime::spawn_blocking(move || {
        // The project list is rebuilt from the saved searches here rather than taken from
        // the caller; the provider re-inspects each project from disk.
        let project_paths = recovery::project_paths(&searches);
        recovery::execute(
            &q,
            &policy,
            &db,
            &user,
            &crate::plugins::all_providers(&db),
            &project_paths,
            &approved,
        )
    })
    .await
    .map_err(|e| CommandError::internal("running the plan", e))?
}
