//! Owns the background directory scan: at most one runs at a time, and the most recent
//! finished result is kept in memory for drill-down. The full tree never crosses IPC;
//! the frontend asks for one folder level at a time.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use sentinel_scanner::scan::{
    LargeFile, NodeId, NodeStatus, ScanControl, ScanOptions, ScanProgress, ScanStats, ScanTree,
    scan,
};
use serde::Serialize;
use ts_rs::TS;

use crate::commands::error::{CommandError, ErrorKind};

pub(crate) const PROGRESS_EVENT: &str = "scan-progress";
pub(crate) const FINISHED_EVENT: &str = "scan-finished";
pub(crate) const FAILED_EVENT: &str = "scan-failed";
const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);
/// Children beyond this many (smallest first) are summarized, not listed.
const MAX_CHILDREN: usize = 200;

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ScanProgressEvent {
    #[ts(type = "number")]
    pub id: u64,
    pub root: String,
    pub progress: ScanProgress,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ScanFinishedEvent {
    #[ts(type = "number")]
    pub id: u64,
    pub root: String,
    pub stats: ScanStats,
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
    pub last: Option<ScanFinishedEvent>,
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
}

/// One level of a finished scan.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DirListing {
    #[ts(type = "number")]
    pub scan_id: u64,
    pub node: NodeId,
    pub path: String,
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
    /// Subfolders not listed because of [`MAX_CHILDREN`].
    #[ts(type = "number")]
    pub hidden_children: u64,
    #[ts(type = "number")]
    pub hidden_bytes: u64,
}

struct Running {
    id: u64,
    root: String,
    control: Arc<ScanControl>,
}

struct Finished {
    id: u64,
    tree: ScanTree,
}

#[derive(Default)]
struct Slot {
    next_id: u64,
    running: Option<Running>,
    last: Option<Arc<Finished>>,
}

/// Something that can receive scan events. Implemented for the Tauri app handle; tests
/// use a recorder.
pub(crate) trait ScanEvents: Send + Sync + 'static {
    fn progress(&self, e: ScanProgressEvent);
    fn finished(&self, e: ScanFinishedEvent);
    fn failed(&self, e: ScanFailedEvent);
}

#[derive(Default)]
pub(crate) struct ScanManager {
    slot: Arc<Mutex<Slot>>,
}

fn lock(slot: &Mutex<Slot>) -> MutexGuard<'_, Slot> {
    // A panic while holding the lock cannot leave `Slot` logically broken (each field is
    // replaced wholesale), so recover rather than propagate poison.
    slot.lock().unwrap_or_else(|p| p.into_inner())
}

impl ScanManager {
    /// Start scanning `root` in the background. Returns the scan id.
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
        let root_s = root.to_owned();
        let spawned = std::thread::Builder::new()
            .name(format!("sentinel-scan-{id}"))
            .spawn(move || {
                let result = scan(&path, &options, &control);
                done.store(true, Ordering::Relaxed);
                let mut guard = lock(&slot);
                guard.running = None;
                match result {
                    Ok(tree) => {
                        let stats = tree.stats.clone();
                        guard.last = Some(Arc::new(Finished { id, tree }));
                        drop(guard);
                        events.finished(ScanFinishedEvent {
                            id,
                            root: root_s,
                            stats,
                        });
                    }
                    Err(err) => {
                        drop(guard);
                        tracing::warn!(id, error = %err, "scan failed");
                        events.failed(ScanFailedEvent {
                            id,
                            root: root_s,
                            message: err.to_string(),
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

    pub(crate) fn status(&self) -> ScanStatus {
        let slot = lock(&self.slot);
        ScanStatus {
            running: slot.running.as_ref().map(|r| ScanProgressEvent {
                id: r.id,
                root: r.root.clone(),
                progress: r.control.progress(),
            }),
            last: slot.last.as_ref().map(|f| ScanFinishedEvent {
                id: f.id,
                root: f.tree.root.display().to_string(),
                stats: f.tree.stats.clone(),
            }),
        }
    }

    fn last(&self) -> Result<Arc<Finished>, CommandError> {
        lock(&self.slot)
            .last
            .clone()
            .ok_or_else(|| CommandError::new(ErrorKind::NotFound, "No finished analysis yet."))
    }

    pub(crate) fn listing(&self, node: NodeId) -> Result<DirListing, CommandError> {
        let last = self.last()?;
        listing_of(last.id, &last.tree, node)
    }

    pub(crate) fn largest_files(&self) -> Result<Vec<LargeFile>, CommandError> {
        Ok(self.last()?.tree.largest_files.clone())
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

fn listing_of(scan_id: u64, tree: &ScanTree, node: NodeId) -> Result<DirListing, CommandError> {
    let n = tree.node(node).ok_or_else(|| {
        CommandError::new(
            ErrorKind::NotFound,
            "That folder is not part of the latest analysis.",
        )
    })?;
    let mut crumbs = Vec::new();
    let mut cur = Some(node);
    while let Some(i) = cur {
        let c = &tree.nodes[i as usize];
        crumbs.push(Crumb {
            id: i,
            name: c.name.clone(),
        });
        cur = c.parent;
    }
    crumbs.reverse();

    let ordered = tree.children_by_size(node);
    let (shown, hidden) = ordered.split_at(ordered.len().min(MAX_CHILDREN));
    let children = shown
        .iter()
        .map(|&c| {
            let k = &tree.nodes[c as usize];
            DirChild {
                id: c,
                name: k.name.clone(),
                total_bytes: k.total_bytes,
                file_count: k.file_count,
                has_children: !k.children.is_empty(),
                status: k.status.clone(),
            }
        })
        .collect();
    let path = tree
        .path_of(node)
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    Ok(DirListing {
        scan_id,
        node,
        path,
        crumbs,
        total_bytes: n.total_bytes,
        files_bytes: n.own_bytes,
        files_here: n.file_count
            - n.children
                .iter()
                .map(|&c| tree.nodes[c as usize].file_count)
                .sum::<u64>(),
        status: n.status.clone(),
        children,
        hidden_children: hidden.len() as u64,
        hidden_bytes: hidden
            .iter()
            .map(|&c| tree.nodes[c as usize].total_bytes)
            .sum(),
    })
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

    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("big").join("inner")).unwrap();
        std::fs::write(
            dir.path().join("big").join("inner").join("f"),
            vec![0u8; 9000],
        )
        .unwrap();
        std::fs::create_dir(dir.path().join("small")).unwrap();
        std::fs::write(dir.path().join("small").join("f"), vec![0u8; 100]).unwrap();
        std::fs::write(dir.path().join("top.bin"), vec![0u8; 50]).unwrap();
        dir
    }

    #[test]
    fn runs_in_background_and_supports_drill_down() {
        let dir = fixture();
        let mgr = ScanManager::default();
        let (rec, rx) = recorder();
        let root = dir.path().to_str().unwrap();
        let id = mgr.start(root, ScanOptions::default(), rec).unwrap();
        let done = wait_done(&rx).unwrap();
        assert_eq!(done.id, id);
        assert_eq!(done.stats.files, 3);
        assert!(mgr.status().running.is_none());
        assert_eq!(mgr.status().last.unwrap().id, id);

        let top = mgr.listing(ScanTree::ROOT).unwrap();
        let names: Vec<_> = top.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["big", "small"]);
        assert_eq!(top.files_here, 1);
        assert_eq!(top.crumbs.len(), 1);

        let big = mgr.listing(top.children[0].id).unwrap();
        assert_eq!(big.crumbs.last().unwrap().name, "big");
        assert!(big.path.ends_with("big"));
        assert!(!big.children[0].has_children);
        assert!(mgr.listing(9999).is_err());
        assert_eq!(mgr.largest_files().unwrap().len(), 3);
    }

    #[test]
    fn rejects_bad_roots_and_concurrent_scans() {
        let dir = fixture();
        let mgr = ScanManager::default();
        let err = mgr
            .start("relative", ScanOptions::default(), recorder().0)
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::InvalidInput);
        let file = dir.path().join("top.bin");
        let err = mgr
            .start(file.to_str().unwrap(), ScanOptions::default(), recorder().0)
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::InvalidInput);

        // Hold the slot as if a scan were running.
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
    fn listing_without_a_scan_is_not_found() {
        let mgr = ScanManager::default();
        assert_eq!(mgr.listing(0).unwrap_err().kind, ErrorKind::NotFound);
    }
}
