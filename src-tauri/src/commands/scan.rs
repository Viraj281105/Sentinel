use sentinel_scanner::scan::{NodeId, ScanOptions};
use sentinel_store::ScanId;
use tauri::{AppHandle, Emitter, State};

use super::error::CommandError;
use crate::AppState;
use crate::scans::{
    DirListing, FAILED_EVENT, FINISHED_EVENT, LargeFileView, PROGRESS_EVENT, ScanEvents,
    ScanFailedEvent, ScanFinishedEvent, ScanProgressEvent, ScanStatus,
};

struct TauriEvents(AppHandle);

impl TauriEvents {
    fn emit(&self, name: &str, payload: impl serde::Serialize + Clone) {
        if let Err(err) = self.0.emit(name, payload) {
            tracing::warn!(event = name, error = %err, "could not emit event");
        }
    }
}

impl ScanEvents for TauriEvents {
    fn progress(&self, e: ScanProgressEvent) {
        self.emit(PROGRESS_EVENT, e);
    }
    fn finished(&self, e: ScanFinishedEvent) {
        self.emit(FINISHED_EVENT, e);
    }
    fn failed(&self, e: ScanFailedEvent) {
        self.emit(FAILED_EVENT, e);
    }
}

/// Start analyzing a folder (usually a drive root). Progress and completion arrive as
/// `scan-progress`, `scan-finished` and `scan-failed` events.
#[tauri::command]
pub(crate) fn start_scan(
    app: AppHandle,
    state: State<'_, AppState>,
    root: String,
) -> Result<u64, CommandError> {
    state
        .scans
        .start(&root, ScanOptions::default(), TauriEvents(app))
}

#[tauri::command]
pub(crate) fn cancel_scan(state: State<'_, AppState>, id: u64) -> bool {
    state.scans.cancel(id)
}

/// The running scan, if any, and the most recent saved analysis.
#[tauri::command]
pub(crate) fn scan_status(state: State<'_, AppState>) -> Result<ScanStatus, CommandError> {
    state.scans.status()
}

#[tauri::command]
pub(crate) fn scan_listing(
    state: State<'_, AppState>,
    scan_id: ScanId,
    node: NodeId,
) -> Result<DirListing, CommandError> {
    state.scans.listing(scan_id, node)
}

#[tauri::command]
pub(crate) fn scan_largest_files(
    state: State<'_, AppState>,
    scan_id: ScanId,
) -> Result<Vec<LargeFileView>, CommandError> {
    state.scans.largest_files(scan_id)
}
