//! Running a structured query against a saved analysis. Read-only.

use sentinel_store::ScanId;
use tauri::State;

use super::error::CommandError;
use crate::AppState;
use crate::query::{self, QueryResult};

/// Run a query (as JSON, validated here) against a saved analysis, the latest by default.
/// Whoever wrote the query, a person or a model, it passes through the same validation.
#[tauri::command]
pub(crate) fn query_saved_analysis(
    state: State<'_, AppState>,
    scan_id: Option<ScanId>,
    query_json: String,
) -> Result<QueryResult, CommandError> {
    query::run_json(
        &state.db,
        &|p| state.scans.classify(p),
        scan_id,
        &query_json,
    )
}
