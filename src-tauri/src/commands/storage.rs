use sentinel_scanner::drives::{self, Drive};
use sentinel_store::DriveSnapshot;
use serde::Serialize;
use tauri::State;
use ts_rs::TS;

use super::error::CommandError;
use crate::AppState;
use crate::db::{Db, now_ms};

const SNAPSHOT_INTERVAL_MS: i64 = 60 * 60 * 1000;
const TREND_WINDOW_MS: i64 = 30 * 24 * 60 * 60 * 1000;

/// How a drive's free space changed over the recorded window.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DriveTrend {
    pub root: String,
    /// When the earliest snapshot in the window was taken.
    #[ts(type = "number")]
    pub since_ms: i64,
    /// Latest free space minus earliest; negative means the drive filled up.
    #[ts(type = "number")]
    pub free_change_bytes: i64,
    pub samples: u32,
}

/// List drives with capacity figures and record a capacity snapshot for each measured
/// drive (at most hourly). Runs on a blocking worker so a slow volume never stalls the UI.
#[tauri::command]
pub(crate) async fn list_drives(state: State<'_, AppState>) -> Result<Vec<Drive>, CommandError> {
    let db = state.db.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let drives = drives::list_drives()?;
        record_snapshots(&db, &drives, now_ms());
        Ok(drives)
    })
    .await
    .map_err(|e| CommandError::internal("listing drives", e))?
}

fn record_snapshots(db: &Db, drives: &[Drive], at: i64) {
    let store = db.lock();
    for d in drives {
        let Some(space) = d.space else { continue };
        let snap = DriveSnapshot {
            taken_at_ms: at,
            total_bytes: space.total_bytes,
            free_bytes: space.free_bytes,
        };
        // History is a convenience; a failed write must not hide the drive list.
        if let Err(err) = store.record_drive_snapshot(&d.root, snap, SNAPSHOT_INTERVAL_MS) {
            tracing::warn!(root = %d.root, error = %err, "could not record drive snapshot");
        }
    }
}

/// Free-space change per drive over the last 30 days, for drives with at least two
/// snapshots in that window.
#[tauri::command]
pub(crate) fn drive_trends(state: State<'_, AppState>) -> Result<Vec<DriveTrend>, CommandError> {
    trends(&state.db, now_ms())
}

fn trends(db: &Db, now: i64) -> Result<Vec<DriveTrend>, CommandError> {
    let store = db.lock();
    let mut out = Vec::new();
    for root in store.snapshot_roots()? {
        let h = store.drive_history(&root, now - TREND_WINDOW_MS)?;
        if let (Some(first), Some(last)) = (h.first(), h.last())
            && h.len() >= 2
        {
            out.push(DriveTrend {
                root,
                since_ms: first.taken_at_ms,
                free_change_bytes: i64::try_from(last.free_bytes).unwrap_or(i64::MAX)
                    - i64::try_from(first.free_bytes).unwrap_or(i64::MAX),
                samples: u32::try_from(h.len()).unwrap_or(u32::MAX),
            });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use sentinel_scanner::drives::{DriveKind, DriveStatus, Space};

    use super::*;

    fn drive(root: &str, free: u64) -> Drive {
        Drive {
            root: root.into(),
            kind: DriveKind::Fixed,
            label: None,
            file_system: Some("NTFS".into()),
            space: Some(Space::new(1000, free, free)),
            is_system: false,
            status: DriveStatus::Ready,
        }
    }

    #[test]
    fn records_hourly_snapshots_and_reports_change() {
        let db = Db::in_memory();
        let hour = SNAPSHOT_INTERVAL_MS;
        record_snapshots(&db, &[drive("C:\\", 500)], 0);
        record_snapshots(&db, &[drive("C:\\", 450)], hour / 2);
        assert!(
            trends(&db, hour).unwrap_or_default().is_empty(),
            "one sample only"
        );
        record_snapshots(&db, &[drive("C:\\", 300)], hour);
        let t = trends(&db, hour).unwrap_or_default();
        assert_eq!(t.len(), 1);
        assert_eq!((t[0].free_change_bytes, t[0].samples), (-200, 2));
    }

    #[test]
    fn skips_unmeasured_drives() {
        let db = Db::in_memory();
        let mut d = drive("E:\\", 0);
        d.space = None;
        record_snapshots(&db, &[d, drive("C:\\", 1)], 0);
        record_snapshots(&db, &[drive("C:\\", 2)], SNAPSHOT_INTERVAL_MS);
        let t = trends(&db, SNAPSHOT_INTERVAL_MS).unwrap_or_default();
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].root, "C:\\");
    }
}
