//! Owns background directory scans: at most one runs at a time. Finished scans are saved
//! to the local database and all drill-down is served from there, so results survive
//! restarts and the in-memory tree is released as soon as it is saved. The full tree
//! never crosses IPC; the frontend asks for one folder level at a time.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use sentinel_scanner::scan::{
    LargeFile, NodeId, NodeStatus, ScanControl, ScanOptions, ScanProgress, ScanStats, scan,
};
use sentinel_store::{Retention, ScanId, ScanRecord};
use serde::Serialize;
use ts_rs::TS;

use crate::commands::error::{CommandError, ErrorKind};
use crate::db::{Db, now_ms};

pub(crate) const PROGRESS_EVENT: &str = "scan-progress";
pub(crate) const FINISHED_EVENT: &str = "scan-finished";
pub(crate) const FAILED_EVENT: &str = "scan-failed";
const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);
/// Children beyond this many (largest first) are summarized, not listed.
const MAX_CHILDREN: u32 = 200;

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ScanProgressEvent {
    /// Id of the running job (used to cancel it).
    #[ts(type = "number")]
    pub id: u64,
    pub root: String,
    pub progress: ScanProgress,
}

/// A reference to an earlier saved scan, for comparison.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ScanRef {
    #[ts(type = "number")]
    pub scan_id: ScanId,
    #[ts(type = "number")]
    pub finished_at_ms: i64,
    #[ts(type = "number")]
    pub total_bytes: u64,
}

/// A scan saved in the local database.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SavedScan {
    #[ts(type = "number")]
    pub scan_id: ScanId,
    pub root: String,
    #[ts(type = "number")]
    pub finished_at_ms: i64,
    pub stats: ScanStats,
    /// Folders smaller than this were not stored individually.
    #[ts(type = "number")]
    pub min_folder_bytes: u64,
    pub previous: Option<ScanRef>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ScanFinishedEvent {
    #[ts(type = "number")]
    pub id: u64,
    pub scan: SavedScan,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ScanFailedEvent {
    #[ts(type = "number")]
    pub id: u64,
    pub root: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ScanStatus {
    pub running: Option<ScanProgressEvent>,
    pub last: Option<SavedScan>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Crumb {
    pub id: NodeId,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DirChild {
    pub id: NodeId,
    pub name: String,
    #[ts(type = "number")]
    pub total_bytes: u64,
    #[ts(type = "number")]
    pub file_count: u64,
    pub has_children: bool,
    pub status: NodeStatus,
    /// Size in the previous analysis, if that analysis stored this folder.
    #[ts(type = "number | null")]
    pub previous_bytes: Option<u64>,
}

/// One level of a saved scan.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DirListing {
    #[ts(type = "number")]
    pub scan_id: ScanId,
    pub node: NodeId,
    pub crumbs: Vec<Crumb>,
    #[ts(type = "number")]
    pub total_bytes: u64,
    /// Bytes in files directly inside this folder (not in subfolders).
    #[ts(type = "number")]
    pub files_bytes: u64,
    #[ts(type = "number")]
    pub files_here: u64,
    pub status: NodeStatus,
    pub children: Vec<DirChild>,
    /// Subfolders not listed: too small to store, or beyond the listing limit.
    #[ts(type = "number")]
    pub hidden_children: u64,
    #[ts(type = "number")]
    pub hidden_bytes: u64,
    /// The analysis this listing is compared with, if any.
    pub compared_to: Option<ScanRef>,
    /// This folder's size in that analysis, if it was stored there.
    #[ts(type = "number | null")]
    pub previous_total_bytes: Option<u64>,
}

struct Running {
    id: u64,
    root: String,
    control: Arc<ScanControl>,
}

#[derive(Default)]
struct Slot {
    next_id: u64,
    running: Option<Running>,
}

/// Something that can receive scan events. Implemented for the Tauri app handle; tests
/// use a recorder.
pub(crate) trait ScanEvents: Send + Sync + 'static {
    fn progress(&self, e: ScanProgressEvent);
    fn finished(&self, e: ScanFinishedEvent);
    fn failed(&self, e: ScanFailedEvent);
}

pub(crate) struct ScanManager {
    slot: Arc<Mutex<Slot>>,
    db: Db,
    retention: Retention,
}

fn lock(slot: &Mutex<Slot>) -> MutexGuard<'_, Slot> {
    // Each field of `Slot` is replaced wholesale, so a poisoned lock is still coherent.
    slot.lock().unwrap_or_else(|p| p.into_inner())
}

fn scan_ref(r: &ScanRecord) -> ScanRef {
    ScanRef {
        scan_id: r.id,
        finished_at_ms: r.finished_at_ms,
        total_bytes: r.stats.total_bytes,
    }
}

fn saved(db: &Db, rec: ScanRecord) -> Result<SavedScan, CommandError> {
    let previous = db.lock().previous_scan(rec.id)?;
    Ok(SavedScan {
        scan_id: rec.id,
        root: rec.root,
        finished_at_ms: rec.finished_at_ms,
        stats: rec.stats,
        min_folder_bytes: rec.min_node_bytes,
        previous: previous.as_ref().map(scan_ref),
    })
}

impl ScanManager {
    pub(crate) fn new(db: Db) -> Self {
        Self {
            slot: Arc::default(),
            db,
            retention: Retention::default(),
        }
    }

    /// Start scanning `root` in the background. Returns the job id.
    pub(crate) fn start(
        &self,
        root: &str,
        options: ScanOptions,
        events: impl ScanEvents,
    ) -> Result<u64, CommandError> {
        let path = PathBuf::from(root);
        if !path.is_absolute() || !path.is_dir() {
            return Err(CommandError::new(
                ErrorKind::InvalidInput,
                format!("{root} is not a folder that can be analyzed."),
            ));
        }
        let mut slot = lock(&self.slot);
        if let Some(r) = &slot.running {
            return Err(CommandError::new(
                ErrorKind::Busy,
                format!("{} is already being analyzed. Cancel it first.", r.root),
            ));
        }
        slot.next_id += 1;
        let id = slot.next_id;
        let control = Arc::new(ScanControl::new());
        slot.running = Some(Running {
            id,
            root: root.to_owned(),
            control: Arc::clone(&control),
        });
        drop(slot);

        tracing::info!(id, root, "scan started");
        let events = Arc::new(events);
        let done = Arc::new(AtomicBool::new(false));
        spawn_progress_reporter(id, root, &control, &events, &done);

        let slot = Arc::clone(&self.slot);
        let db = self.db.clone();
        let retention = self.retention;
        let root_s = root.to_owned();
        let spawned = std::thread::Builder::new()
            .name(format!("sentinel-scan-{id}"))
            .spawn(move || {
                let started = now_ms();
                let outcome = scan(&path, &options, &control)
                    .map_err(|e| e.to_string())
                    .and_then(|tree| {
                        let finished = now_ms();
                        let scan_id = db
                            .lock()
                            .save_scan(&tree, started, finished, retention)
                            .map_err(|e| {
                                format!("the analysis finished but could not be saved: {e}")
                            })?;
                        drop(tree);
                        let rec = db
                            .lock()
                            .scan(scan_id)
                            .map_err(|e| e.to_string())?
                            .ok_or_else(|| "the saved analysis disappeared".to_owned())?;
                        saved(&db, rec).map_err(|e| e.message)
                    });
                done.store(true, Ordering::Relaxed);
                lock(&slot).running = None;
                match outcome {
                    Ok(scan) => events.finished(ScanFinishedEvent { id, scan }),
                    Err(message) => {
                        tracing::warn!(id, error = %message, "scan failed");
                        events.failed(ScanFailedEvent {
                            id,
                            root: root_s,
                            message,
                        });
                    }
                }
            });
        if let Err(err) = spawned {
            lock(&self.slot).running = None;
            return Err(CommandError::internal("starting the scan", err));
        }
        Ok(id)
    }

    /// Request cancellation. Returns false if `id` is not the running scan.
    pub(crate) fn cancel(&self, id: u64) -> bool {
        match &lock(&self.slot).running {
            Some(r) if r.id == id => {
                r.control.cancel();
                tracing::info!(id, "scan cancellation requested");
                true
            }
            _ => false,
        }
    }

    pub(crate) fn status(&self) -> Result<ScanStatus, CommandError> {
        let running = lock(&self.slot)
            .running
            .as_ref()
            .map(|r| ScanProgressEvent {
                id: r.id,
                root: r.root.clone(),
                progress: r.control.progress(),
            });
        let latest = self.db.lock().latest_scan()?;
        Ok(ScanStatus {
            running,
            last: latest.map(|r| saved(&self.db, r)).transpose()?,
        })
    }

    pub(crate) fn listing(
        &self,
        scan_id: ScanId,
        node: NodeId,
    ) -> Result<DirListing, CommandError> {
        let store = self.db.lock();
        let not_found = || {
            CommandError::new(
                ErrorKind::NotFound,
                "That folder is not part of a saved analysis.",
            )
        };
        let n = store.node(scan_id, node)?.ok_or_else(not_found)?;
        let crumbs = store.crumbs(scan_id, node)?;
        let children = store.children(scan_id, node, MAX_CHILDREN)?;
        let (beyond, beyond_bytes) = store.children_beyond(scan_id, node, MAX_CHILDREN)?;

        // Compare with the previous analysis of the same root by folder path.
        let previous = store.previous_scan(scan_id)?;
        let mut previous_total = None;
        let mut previous_children: HashMap<String, u64> = HashMap::new();
        if let Some(prev) = &previous {
            let names: Vec<&str> = crumbs.iter().skip(1).map(|c| c.name.as_str()).collect();
            if let Some(pn) = store.find_by_names(prev.id, &names)? {
                previous_total = store.node(prev.id, pn)?.map(|p| p.total_bytes);
                for c in store.children(prev.id, pn, u32::MAX)? {
                    previous_children.insert(c.name.to_lowercase(), c.total_bytes);
                }
            }
        }

        Ok(DirListing {
            scan_id,
            node,
            crumbs: crumbs
                .into_iter()
                .map(|c| Crumb {
                    id: c.id,
                    name: c.name,
                })
                .collect(),
            total_bytes: n.total_bytes,
            files_bytes: n.own_bytes,
            files_here: n.own_files,
            status: n.status,
            children: children
                .into_iter()
                .map(|c| DirChild {
                    previous_bytes: previous_children.get(&c.name.to_lowercase()).copied(),
                    id: c.id,
                    name: c.name,
                    total_bytes: c.total_bytes,
                    file_count: c.file_count,
                    has_children: c.child_count > 0,
                    status: c.status,
                })
                .collect(),
            hidden_children: n.pruned_children + beyond,
            hidden_bytes: n.pruned_bytes + beyond_bytes,
            compared_to: previous.as_ref().map(scan_ref),
            previous_total_bytes: previous_total,
        })
    }

    pub(crate) fn largest_files(&self, scan_id: ScanId) -> Result<Vec<LargeFile>, CommandError> {
        Ok(self.db.lock().largest_files(scan_id)?)
    }
}

fn spawn_progress_reporter(
    id: u64,
    root: &str,
    control: &Arc<ScanControl>,
    events: &Arc<impl ScanEvents>,
    done: &Arc<AtomicBool>,
) {
    let (control, events, done, root) = (
        Arc::clone(control),
        Arc::clone(events),
        Arc::clone(done),
        root.to_owned(),
    );
    let spawned = std::thread::Builder::new()
        .name(format!("sentinel-scan-progress-{id}"))
        .spawn(move || {
            while !done.load(Ordering::Relaxed) {
                events.progress(ScanProgressEvent {
                    id,
                    root: root.clone(),
                    progress: control.progress(),
                });
                std::thread::sleep(PROGRESS_INTERVAL);
            }
        });
    if let Err(err) = spawned {
        // Progress is cosmetic; the scan itself still runs and reports completion.
        tracing::warn!(id, error = %err, "could not start progress reporter");
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::sync::mpsc;

    use super::*;

    enum Ev {
        Progress,
        Finished(ScanFinishedEvent),
        Failed(ScanFailedEvent),
    }

    struct Recorder(Mutex<mpsc::Sender<Ev>>);

    impl ScanEvents for Recorder {
        fn progress(&self, _: ScanProgressEvent) {
            let _ = self.0.lock().unwrap().send(Ev::Progress);
        }
        fn finished(&self, e: ScanFinishedEvent) {
            let _ = self.0.lock().unwrap().send(Ev::Finished(e));
        }
        fn failed(&self, e: ScanFailedEvent) {
            let _ = self.0.lock().unwrap().send(Ev::Failed(e));
        }
    }

    fn recorder() -> (Recorder, mpsc::Receiver<Ev>) {
        let (tx, rx) = mpsc::channel();
        (Recorder(Mutex::new(tx)), rx)
    }

    fn wait_done(rx: &mpsc::Receiver<Ev>) -> Result<ScanFinishedEvent, ScanFailedEvent> {
        loop {
            match rx
                .recv_timeout(Duration::from_secs(30))
                .expect("scan event")
            {
                Ev::Progress => {}
                Ev::Finished(e) => return Ok(e),
                Ev::Failed(e) => return Err(e),
            }
        }
    }

    const MB: usize = 1024 * 1024;

    fn put(path: &std::path::Path, bytes: usize) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, vec![0u8; bytes]).unwrap();
    }

    /// big/inner/f (3 MB), small/f (100 B, pruned), top.bin
    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        put(&dir.path().join("big").join("inner").join("f"), 3 * MB);
        put(&dir.path().join("small").join("f"), 100);
        put(&dir.path().join("top.bin"), 50);
        dir
    }

    fn run(mgr: &ScanManager, root: &std::path::Path) -> SavedScan {
        let (rec, rx) = recorder();
        let id = mgr
            .start(root.to_str().unwrap(), ScanOptions::default(), rec)
            .unwrap();
        let done = wait_done(&rx).unwrap_or_else(|e| panic!("scan failed: {}", e.message));
        assert_eq!(done.id, id);
        done.scan
    }

    #[test]
    fn saves_scans_and_serves_drill_down_from_the_database() {
        let dir = fixture();
        let mgr = ScanManager::new(Db::in_memory());
        let s = run(&mgr, dir.path());
        assert_eq!(s.stats.files, 3);
        assert!(s.previous.is_none());
        let status = mgr.status().unwrap();
        assert!(status.running.is_none());
        assert_eq!(status.last.unwrap().scan_id, s.scan_id);

        let top = mgr.listing(s.scan_id, 0).unwrap();
        let names: Vec<_> = top.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["big"], "small folder is pruned");
        assert_eq!((top.hidden_children, top.hidden_bytes > 0), (1, true));
        assert_eq!(top.files_here, 1);
        assert!(top.compared_to.is_none());

        let big = mgr.listing(s.scan_id, top.children[0].id).unwrap();
        assert_eq!(big.crumbs.last().unwrap().name, "big");
        assert!(!big.children[0].has_children);
        assert_eq!(
            mgr.listing(s.scan_id, 9999).unwrap_err().kind,
            ErrorKind::NotFound
        );
        assert_eq!(mgr.largest_files(s.scan_id).unwrap().len(), 3);
    }

    #[test]
    fn compares_with_the_previous_analysis_by_path() {
        let dir = fixture();
        let mgr = ScanManager::new(Db::in_memory());
        let first = run(&mgr, dir.path());
        put(&dir.path().join("big").join("inner").join("g"), 2 * MB);
        put(&dir.path().join("fresh").join("f"), 2 * MB);
        let second = run(&mgr, dir.path());
        assert_eq!(second.previous.as_ref().unwrap().scan_id, first.scan_id);

        let top = mgr.listing(second.scan_id, 0).unwrap();
        assert_eq!(top.compared_to.unwrap().scan_id, first.scan_id);
        let by_name = |n: &str| top.children.iter().find(|c| c.name == n).unwrap();
        let big = by_name("big");
        assert!(big.previous_bytes.unwrap() + (2 * MB) as u64 <= big.total_bytes + 4096);
        assert!(by_name("fresh").previous_bytes.is_none());
        assert!(top.previous_total_bytes.unwrap() < top.total_bytes);

        let inner_id = mgr.listing(second.scan_id, big.id).unwrap().children[0].id;
        let inner = mgr.listing(second.scan_id, inner_id).unwrap();
        assert!(inner.previous_total_bytes.is_some());
    }

    #[test]
    fn rejects_bad_roots_and_concurrent_scans() {
        let dir = fixture();
        let mgr = ScanManager::new(Db::in_memory());
        let err = mgr
            .start("relative", ScanOptions::default(), recorder().0)
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::InvalidInput);
        let file = dir.path().join("top.bin");
        let err = mgr
            .start(file.to_str().unwrap(), ScanOptions::default(), recorder().0)
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::InvalidInput);

        lock(&mgr.slot).running = Some(Running {
            id: 42,
            root: "X".into(),
            control: Arc::new(ScanControl::new()),
        });
        let err = mgr
            .start(
                dir.path().to_str().unwrap(),
                ScanOptions::default(),
                recorder().0,
            )
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::Busy);
        assert!(!mgr.cancel(7));
        assert!(mgr.cancel(42));
    }

    #[test]
    fn empty_database_has_no_last_scan() {
        let mgr = ScanManager::new(Db::in_memory());
        assert!(mgr.status().unwrap().last.is_none());
        assert_eq!(mgr.listing(1, 0).unwrap_err().kind, ErrorKind::NotFound);
    }
}
