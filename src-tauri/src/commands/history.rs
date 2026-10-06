//! Growth history of analyzed drives and folders. Read-only: reads saved analyses only.

use sentinel_store::ScanId;
use tauri::State;

use super::error::CommandError;
use crate::AppState;
use crate::anomaly::{self, AnomalyReport};
use crate::history::{self, FolderHistory, GrowthHistory};

/// How the root of an analysis (the latest one by default) changed across its analyses.
#[tauri::command]
pub(crate) fn growth_history(
    state: State<'_, AppState>,
    scan_id: Option<ScanId>,
) -> Result<GrowthHistory, CommandError> {
    history::growth_history(&state.db, scan_id)
}

/// How one folder of a saved analysis changed across the analyses of its root.
#[tauri::command]
pub(crate) fn folder_history(
    state: State<'_, AppState>,
    scan_id: ScanId,
    node: u32,
) -> Result<FolderHistory, CommandError> {
    history::folder_history(&state.db, scan_id, node)
}

/// What grew or shrank most between two analyses of a drive, and the folders responsible.
/// Compares the latest analysis with the previous complete one unless told otherwise.
#[tauri::command]
pub(crate) fn anomaly_report(
    state: State<'_, AppState>,
    scan_id: Option<ScanId>,
    baseline_id: Option<ScanId>,
) -> Result<AnomalyReport, CommandError> {
    anomaly::attribute(&state.db, scan_id, baseline_id, &|p| {
        state.scans.classify(p)
    })
}
