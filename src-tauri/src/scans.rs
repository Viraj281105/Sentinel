//! Owns background directory scans: at most one runs at a time. Finished scans are saved
//! to the local database and all drill-down is served from there, so results survive
//! restarts and the in-memory tree is released as soon as it is saved. The full tree
//! never crosses IPC; the frontend asks for one folder level at a time.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use sentinel_classify::{Category, CategoryBytes, Classification, Classifier};
use sentinel_scanner::incremental::{RescanError, rescan};
use sentinel_scanner::persist::{self, Snapshot};
use sentinel_scanner::scan::{
    LargeFile, NodeId, NodeStatus, ScanControl, ScanOptions, ScanProgress, ScanStats, ScanTree,
    scan,
};
use sentinel_scanner::usn::{JournalChange, JournalPosition, journal_position};
use sentinel_scanner::watch::{Changes, WatchOptions, Watcher};
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
/// The last tree is kept in memory for incremental updates only if it is this small
/// (roughly 100 MB at most). Larger trees are updated by a full analysis.
const RETAIN_MAX_NODES: usize = 400_000;

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
    /// Bytes per category, largest first. Empty for analyses saved before
    /// classification existed.
    pub categories: Vec<CategoryBytes>,
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
    pub classification: Option<Classification>,
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
    pub classification: Option<Classification>,
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

/// One of a saved analysis's largest files, with its classification.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LargeFileView {
    pub path: String,
    #[ts(type = "number")]
    pub bytes: u64,
    #[ts(type = "number")]
    pub logical_bytes: u64,
    pub classification: Option<Classification>,
}

/// What was learned about the time Sentinel was closed, from the volume's change journal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "state"
)]
#[ts(export)]
pub enum ResumeStatus {
    /// The drive's change journal did not move while Sentinel was closed: nothing on it
    /// changed, so the saved analysis should still be current and updates stay incremental.
    Verified,
    /// Something on the drive changed while Sentinel was closed. The journal cannot say
    /// where without administrator rights, so a full analysis is needed.
    ChangedWhileClosed {
        /// How far the journal advanced: a rough size of the activity, not a file count.
        #[ts(type = "number")]
        journal_bytes: u64,
    },
    /// The drive's journal was recreated, so what happened meanwhile cannot be known.
    JournalReset,
}

/// Whether the saved analysis is still current, according to the folder watcher.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WatchStatus {
    /// The folder being watched (the last analyzed root).
    pub root: Option<String>,
    pub watching: bool,
    /// When watching began. Changes before that are unknown.
    #[ts(type = "number | null")]
    pub since_ms: Option<i64>,
    /// Folders whose contents changed since the analysis.
    #[ts(type = "number")]
    pub changed_dirs: u64,
    /// Busy folders (temp files, `node_modules`, ...) with changes somewhere beneath them.
    #[ts(type = "number")]
    pub changed_subtrees: u64,
    /// Events were lost (or the watch broke): only a full analysis is reliable.
    pub lost_events: bool,
    pub broken: Option<String>,
    /// An update can re-read just the changed folders (otherwise it analyzes everything).
    pub can_update_incrementally: bool,
    /// What the drive's change journal said about the time Sentinel was closed. `None` when
    /// it could not be checked (no saved tree, or a drive without a journal) or after a new
    /// analysis.
    pub after_restart: Option<ResumeStatus>,
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

/// The last full tree, kept so changed folders can be re-read without a full analysis.
struct Retained {
    scan_id: ScanId,
    tree: ScanTree,
}

/// Changes reported by the watcher and not yet applied to an analysis.
#[derive(Default)]
struct Accum {
    dirs: BTreeSet<PathBuf>,
    subtrees: BTreeSet<PathBuf>,
    root_dirty: bool,
    broken: Option<String>,
}

impl Accum {
    fn merge(&mut self, c: Changes) {
        self.dirs.extend(c.dirty_dirs);
        self.subtrees.extend(c.dirty_subtrees);
        self.root_dirty |= c.root_dirty;
        if c.broken.is_some() {
            self.broken = c.broken;
        }
    }

    fn take(&mut self) -> Changes {
        let a = std::mem::take(self);
        Changes {
            dirty_dirs: a.dirs.into_iter().collect(),
            dirty_subtrees: a.subtrees.into_iter().collect(),
            root_dirty: a.root_dirty,
            broken: a.broken,
            ..Changes::default()
        }
    }
}

struct ActiveWatch {
    root: PathBuf,
    watcher: Watcher,
    since_ms: i64,
    acc: Accum,
}

impl ActiveWatch {
    fn pump(&mut self) {
        let c = self.watcher.drain();
        self.acc.merge(c);
    }
}

/// Whether two spellings name the same folder. Both are resolved first, because the analysis
/// stores its root in canonical form (links followed, 8.3 short names expanded) while a watch
/// keeps the spelling it was started with; comparing the text alone would call them different
/// and silently turn off incremental updates. A path that cannot be resolved is compared as
/// written.
fn same_folder(a: &Path, b: &Path) -> bool {
    let resolved = |p: &Path| {
        sentinel_safety::CanonicalPath::resolve(p)
            .map(|c| c.as_path().to_path_buf())
            .unwrap_or_else(|_| p.to_path_buf())
    };
    let norm = |p: PathBuf| {
        let s = p.to_string_lossy().to_lowercase();
        s.trim_start_matches(r"\\?\")
            .trim_end_matches('\\')
            .to_owned()
    };
    norm(resolved(a)) == norm(resolved(b))
}

/// Something that can receive scan events. Implemented for the Tauri app handle; tests
/// use a recorder.
pub(crate) trait ScanEvents: Send + Sync + 'static {
    fn progress(&self, e: ScanProgressEvent);
    fn finished(&self, e: ScanFinishedEvent);
    fn failed(&self, e: ScanFailedEvent);
}

/// Reads a volume's journal position; replaceable so tests do not depend on other programs
/// writing to the same drive.
type Probe = Arc<dyn Fn(&Path) -> std::io::Result<JournalPosition> + Send + Sync>;

pub(crate) struct ScanManager {
    slot: Arc<Mutex<Slot>>,
    retained: Arc<Mutex<Option<Retained>>>,
    watch: Arc<Mutex<Option<ActiveWatch>>>,
    db: Db,
    classifier: Arc<Classifier>,
    retention: Retention,
    /// Where the last full tree is saved so it survives a restart.
    snapshot_file: Option<PathBuf>,
    probe: Probe,
    /// Bumped by every new analysis, so a slow restore can tell it has been overtaken.
    epoch: Arc<AtomicU64>,
    resume: Arc<Mutex<Option<ResumeStatus>>>,
}

/// A snapshot larger than this is not read back (the app keeps at most ~400,000 folders).
const SNAPSHOT_MAX_BYTES: u64 = 1 << 30;

/// Save the tree atomically. Best effort: failing only means a full analysis after a restart.
fn write_snapshot(file: &Path, scan_id: ScanId, journal: JournalPosition, tree: &ScanTree) {
    let snap = Snapshot {
        scan_id,
        journal,
        saved_at_ms: now_ms(),
        tree: tree.clone(),
    };
    let result = persist::encode(&snap)
        .map_err(|e| e.to_string())
        .and_then(|bytes| {
            let tmp = file.with_extension("tmp");
            if let Some(dir) = file.parent() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            std::fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
            std::fs::rename(&tmp, file).map_err(|e| e.to_string())
        });
    if let Err(err) = result {
        tracing::warn!(file = %file.display(), error = %err, "could not save the scan tree");
    }
}

fn remove_snapshot(file: &Path) {
    if let Err(err) = std::fs::remove_file(file)
        && err.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(file = %file.display(), error = %err, "could not remove the saved scan tree");
    }
}

/// Everything the startup restore needs, so it can run on its own thread.
struct RestoreCtx {
    db: Db,
    file: PathBuf,
    probe: Probe,
    retained: Arc<Mutex<Option<Retained>>>,
    resume: Arc<Mutex<Option<ResumeStatus>>>,
    epoch: Arc<AtomicU64>,
}

/// Bring the last full tree back after a restart, but only when the drive's change journal
/// shows nothing at all changed since the tree was saved. Otherwise the file is discarded.
fn restore(ctx: &RestoreCtx, epoch_seen: u64) {
    let Ok(meta) = std::fs::metadata(&ctx.file) else {
        return; // nothing saved
    };
    let discard = || remove_snapshot(&ctx.file);
    if meta.len() > SNAPSHOT_MAX_BYTES {
        return discard();
    }
    let snap = match std::fs::read(&ctx.file)
        .map_err(|e| e.to_string())
        .and_then(|b| persist::decode(&b).map_err(|e| e.to_string()))
    {
        Ok(s) => s,
        Err(err) => {
            tracing::warn!(error = %err, "the saved scan tree is unusable; discarded");
            return discard();
        }
    };
    let latest = ctx.db.lock().latest_scan().ok().flatten();
    let current = latest
        .is_some_and(|l| l.id == snap.scan_id && same_folder(Path::new(&l.root), &snap.tree.root));
    if !current {
        return discard(); // a newer analysis exists
    }
    let now = match (ctx.probe)(&snap.tree.root) {
        Ok(p) => p,
        Err(err) => {
            tracing::info!(error = %err, "the drive's change journal could not be read");
            return discard();
        }
    };
    let status = match snap.journal.compare(now) {
        JournalChange::Unchanged => ResumeStatus::Verified,
        JournalChange::Changed { bytes } => ResumeStatus::ChangedWhileClosed {
            journal_bytes: bytes,
        },
        JournalChange::Reset => ResumeStatus::JournalReset,
    };
    // The check and the install happen under the same lock `new_baseline` takes to bump the
    // epoch, so an analysis begun meanwhile can never be followed by this older tree.
    let mut resume = lock_any(&ctx.resume);
    if ctx.epoch.load(Ordering::SeqCst) != epoch_seen {
        return;
    }
    if status == ResumeStatus::Verified {
        let mut kept = lock_any(&ctx.retained);
        if kept.is_some() {
            return;
        }
        *kept = Some(Retained {
            scan_id: snap.scan_id,
            tree: snap.tree,
        });
        tracing::info!(
            scan = snap.scan_id,
            "scan tree restored; the drive had no changes"
        );
    } else {
        discard();
        tracing::info!(?status, "the drive changed while Sentinel was closed");
    }
    *resume = Some(status);
}

fn lock(slot: &Mutex<Slot>) -> MutexGuard<'_, Slot> {
    // Each field of `Slot` is replaced wholesale, so a poisoned lock is still coherent.
    slot.lock().unwrap_or_else(|p| p.into_inner())
}

fn lock_any<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// Save a finished tree and describe it. The caller decides whether to keep the tree.
fn save_tree(
    db: &Db,
    classifier: &Classifier,
    retention: Retention,
    tree: &ScanTree,
    started: i64,
) -> Result<SavedScan, String> {
    let finished = now_ms();
    let breakdown = classifier.breakdown(tree);
    let categories: Vec<(&str, u64)> = breakdown
        .iter()
        .map(|c| (c.category.key(), c.bytes))
        .collect();
    let scan_id = db
        .lock()
        .save_scan(tree, &categories, started, finished, retention)
        .map_err(|e| format!("the analysis finished but could not be saved: {e}"))?;
    let rec = db
        .lock()
        .scan(scan_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "the saved analysis disappeared".to_owned())?;
    saved(db, rec).map_err(|e| e.message)
}

/// Apply `changes` to the retained tree and save the result as a new analysis.
fn apply_update(
    kept: &mut Option<Retained>,
    changes: &Changes,
    control: &ScanControl,
    (db, classifier, retention): (&Db, &Classifier, Retention),
    started: i64,
) -> Result<SavedScan, String> {
    let r = kept
        .as_mut()
        .ok_or_else(|| "There is no saved tree to update; run a full analysis.".to_owned())?;
    rescan(&mut r.tree, changes, &ScanOptions::default(), control).map_err(|e| match e {
        RescanError::Cancelled => "The update was cancelled.".to_owned(),
        RescanError::FullScanNeeded => {
            "Changes were missed, so a full analysis is needed.".to_owned()
        }
        RescanError::Scan(e) => e.to_string(),
    })?;
    let scan = save_tree(db, classifier, retention, &r.tree, started)?;
    r.scan_id = scan.scan_id;
    Ok(scan)
}

fn scan_ref(r: &ScanRecord) -> ScanRef {
    ScanRef {
        scan_id: r.id,
        finished_at_ms: r.finished_at_ms,
        total_bytes: r.stats.total_bytes,
    }
}

fn saved(db: &Db, rec: ScanRecord) -> Result<SavedScan, CommandError> {
    let (previous, categories) = {
        let store = db.lock();
        (store.previous_scan(rec.id)?, store.scan_categories(rec.id)?)
    };
    let categories = categories
        .into_iter()
        .filter_map(|(key, bytes)| match Category::from_key(&key) {
            Some(category) => Some(CategoryBytes { category, bytes }),
            None => {
                tracing::warn!(key, "unknown category in database; ignored");
                None
            }
        })
        .collect();
    Ok(SavedScan {
        categories,
        scan_id: rec.id,
        root: rec.root,
        finished_at_ms: rec.finished_at_ms,
        stats: rec.stats,
        min_folder_bytes: rec.min_node_bytes,
        previous: previous.as_ref().map(scan_ref),
    })
}

impl ScanManager {
    pub(crate) fn new(db: Db, classifier: Arc<Classifier>) -> Self {
        Self {
            slot: Arc::default(),
            retained: Arc::default(),
            watch: Arc::default(),
            db,
            classifier,
            retention: Retention::default(),
            snapshot_file: None,
            probe: Arc::new(journal_position),
            epoch: Arc::default(),
            resume: Arc::default(),
        }
    }

    /// Keep the last full tree in `file` so incremental updates survive a restart.
    pub(crate) fn with_snapshot_file(mut self, file: PathBuf) -> Self {
        self.snapshot_file = Some(file);
        self
    }

    #[cfg(test)]
    fn with_probe(mut self, probe: Probe) -> Self {
        self.probe = probe;
        self
    }

    fn restore_ctx(&self) -> Option<RestoreCtx> {
        Some(RestoreCtx {
            db: self.db.clone(),
            file: self.snapshot_file.clone()?,
            probe: Arc::clone(&self.probe),
            retained: Arc::clone(&self.retained),
            resume: Arc::clone(&self.resume),
            epoch: Arc::clone(&self.epoch),
        })
    }

    /// Claim the single scan slot for `root`.
    fn claim(&self, root: &str) -> Result<(u64, Arc<ScanControl>), CommandError> {
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
        Ok((id, control))
    }

    /// Watch `path` for changes. Started *before* a scan so changes made during it are
    /// reported as pending rather than missed. A failure only means no staleness hints.
    fn begin_watch(&self, path: &Path) {
        let mut w = lock_any(&self.watch);
        *w = None; // stop the previous watch first
        match Watcher::start(path, &WatchOptions::default()) {
            Ok(watcher) => {
                *w = Some(ActiveWatch {
                    root: path.to_path_buf(),
                    watcher,
                    since_ms: now_ms(),
                    acc: Accum::default(),
                });
            }
            Err(err) => {
                tracing::warn!(root = %path.display(), error = %err, "folder watch not started");
            }
        }
    }

    /// Resume watching the last analyzed folder after a restart. Changes made while the
    /// app was closed are unknown, so no incremental update is possible until the next
    /// full analysis.
    pub(crate) fn resume_watch(&self) {
        let Ok(Some(latest)) = self.db.lock().latest_scan() else {
            return;
        };
        let path = PathBuf::from(&latest.root);
        if path.is_absolute() && path.is_dir() {
            // Watch first, then check the journal: anything that changes after the watch
            // began is seen by the watcher, anything before it by the journal.
            self.begin_watch(&path);
            if let Some(ctx) = self.restore_ctx() {
                let epoch = self.epoch.load(Ordering::SeqCst);
                let spawned = std::thread::Builder::new()
                    .name("sentinel-restore".into())
                    .spawn(move || restore(&ctx, epoch));
                if let Err(err) = spawned {
                    tracing::warn!(error = %err, "could not start restoring the scan tree");
                }
            }
        }
    }

    /// [`Self::resume_watch`] with the restore done before returning, for tests.
    #[cfg(test)]
    fn resume_watch_now(&self) {
        let Ok(Some(latest)) = self.db.lock().latest_scan() else {
            return;
        };
        self.begin_watch(Path::new(&latest.root));
        if let Some(ctx) = self.restore_ctx() {
            restore(&ctx, self.epoch.load(Ordering::SeqCst));
        }
    }

    /// A new baseline: forget what the last restart learned and any saved tree.
    fn new_baseline(&self) {
        let mut resume = lock_any(&self.resume);
        self.epoch.fetch_add(1, Ordering::SeqCst);
        *resume = None;
        drop(resume);
        if let Some(file) = &self.snapshot_file {
            remove_snapshot(file);
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
        let (id, control) = self.claim(root)?;
        // The old tree describes an older state; the watch restarts with this scan.
        *lock_any(&self.retained) = None;
        self.new_baseline();
        self.begin_watch(&path);
        // Where the journal stands *before* the scan: any change after this, including one
        // made during the scan, makes a later comparison say "changed", never "unchanged".
        let journal = (self.probe)(&path).ok();
        let snapshot_file = self.snapshot_file.clone();

        tracing::info!(id, root, "scan started");
        let events = Arc::new(events);
        let done = Arc::new(AtomicBool::new(false));
        spawn_progress_reporter(id, root, &control, &events, &done);

        let slot = Arc::clone(&self.slot);
        let retained = Arc::clone(&self.retained);
        let db = self.db.clone();
        let classifier = Arc::clone(&self.classifier);
        let retention = self.retention;
        let root_s = root.to_owned();
        let spawned = std::thread::Builder::new()
            .name(format!("sentinel-scan-{id}"))
            .spawn(move || {
                let started = now_ms();
                let outcome = scan(&path, &options, &control)
                    .map_err(|e| e.to_string())
                    .and_then(|tree| {
                        let scan = save_tree(&db, &classifier, retention, &tree, started)?;
                        // Keep a complete, small tree for incremental updates.
                        let keep = tree.nodes.len() <= RETAIN_MAX_NODES
                            && !tree.stats.cancelled
                            && !tree.stats.truncated;
                        if keep && let (Some(file), Some(journal)) = (&snapshot_file, journal) {
                            write_snapshot(file, scan.scan_id, journal, &tree);
                        }
                        *lock_any(&retained) = keep.then_some(Retained {
                            scan_id: scan.scan_id,
                            tree,
                        });
                        Ok(scan)
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

    /// Where the saved analysis stands relative to the filesystem.
    pub(crate) fn watch_status(&self) -> Result<WatchStatus, CommandError> {
        let latest = self.db.lock().latest_scan()?;
        let mut w = lock_any(&self.watch);
        let (Some(active), Some(latest)) = (w.as_mut(), latest) else {
            return Ok(WatchStatus {
                root: None,
                watching: false,
                since_ms: None,
                changed_dirs: 0,
                changed_subtrees: 0,
                lost_events: false,
                broken: None,
                can_update_incrementally: false,
                after_restart: None,
            });
        };
        active.pump();
        let retained_ok = lock_any(&self.retained)
            .as_ref()
            .is_some_and(|r| r.scan_id == latest.id);
        let lost = active.acc.root_dirty || active.acc.broken.is_some();
        Ok(WatchStatus {
            root: Some(active.root.display().to_string()),
            watching: true,
            since_ms: Some(active.since_ms),
            changed_dirs: active.acc.dirs.len() as u64,
            changed_subtrees: active.acc.subtrees.len() as u64,
            lost_events: lost,
            broken: active.acc.broken.clone(),
            can_update_incrementally: retained_ok
                && !lost
                && same_folder(&active.root, Path::new(&latest.root)),
            after_restart: lock_any(&self.resume).clone(),
        })
    }

    /// Bring the latest analysis up to date: re-read only the changed folders when that is
    /// reliable, otherwise analyze the whole root again. Returns the job id.
    pub(crate) fn update(&self, events: impl ScanEvents) -> Result<u64, CommandError> {
        let latest = self.db.lock().latest_scan()?.ok_or_else(|| {
            CommandError::new(ErrorKind::NotFound, "There is no analysis to update yet.")
        })?;
        if self.watch_status()?.can_update_incrementally {
            self.start_incremental(&latest.root, events)
        } else {
            self.start(&latest.root, ScanOptions::default(), events)
        }
    }

    fn start_incremental(&self, root: &str, events: impl ScanEvents) -> Result<u64, CommandError> {
        let (id, control) = self.claim(root)?;
        tracing::info!(id, root, "incremental update started");
        // Captured before the changes are taken, for the same reason as in `start`.
        let journal = (self.probe)(Path::new(root)).ok();
        let snapshot_file = self.snapshot_file.clone();
        let events = Arc::new(events);
        let done = Arc::new(AtomicBool::new(false));
        spawn_progress_reporter(id, root, &control, &events, &done);

        let slot = Arc::clone(&self.slot);
        let retained = Arc::clone(&self.retained);
        let watch = Arc::clone(&self.watch);
        let db = self.db.clone();
        let classifier = Arc::clone(&self.classifier);
        let retention = self.retention;
        let root_s = root.to_owned();
        let spawned = std::thread::Builder::new()
            .name(format!("sentinel-update-{id}"))
            .spawn(move || {
                let started = now_ms();
                // Take the pending changes and the tree; both are put back on failure.
                let changes = {
                    let mut w = lock_any(&watch);
                    w.as_mut().map_or_else(Changes::default, |a| {
                        a.pump();
                        a.acc.take()
                    })
                };
                let mut kept = lock_any(&retained).take();
                let outcome = apply_update(
                    &mut kept,
                    &changes,
                    &control,
                    (&db, &classifier, retention),
                    started,
                );
                if let (Ok(scan), Some(r), Some(file), Some(journal)) =
                    (&outcome, &kept, &snapshot_file, journal)
                {
                    write_snapshot(file, scan.scan_id, journal, &r.tree);
                }
                *lock_any(&retained) = kept;
                if outcome.is_err()
                    && let Some(a) = lock_any(&watch).as_mut()
                {
                    a.acc.merge(changes);
                }
                done.store(true, Ordering::Relaxed);
                lock(&slot).running = None;
                match outcome {
                    Ok(scan) => events.finished(ScanFinishedEvent { id, scan }),
                    Err(message) => {
                        tracing::warn!(id, error = %message, "incremental update failed");
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
            return Err(CommandError::internal("starting the update", err));
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
        let mut path = PathBuf::from(store.scan(scan_id)?.ok_or_else(not_found)?.root);
        path.extend(crumbs.iter().skip(1).map(|c| c.name.as_str()));
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
            classification: self.classifier.classify(&path),
            children: children
                .into_iter()
                .map(|c| DirChild {
                    classification: self.classifier.classify(&path.join(&c.name)),
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

    /// What Sentinel knows about a path's kind, by the same rules as the folder listing.
    pub(crate) fn classify(&self, path: &Path) -> Option<Classification> {
        self.classifier.classify(path)
    }

    pub(crate) fn largest_files(
        &self,
        scan_id: ScanId,
    ) -> Result<Vec<LargeFileView>, CommandError> {
        let files: Vec<LargeFile> = self.db.lock().largest_files(scan_id)?;
        Ok(files
            .into_iter()
            .map(|f| LargeFileView {
                classification: self.classifier.classify(std::path::Path::new(&f.path)),
                path: f.path,
                bytes: f.bytes,
                logical_bytes: f.logical_bytes,
            })
            .collect())
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

    /// A manager whose classifier has no known-folder anchors, so only name and
    /// drive-root rules apply: results do not depend on where the temp dir lives.
    fn manager() -> ScanManager {
        ScanManager::new(Db::in_memory(), Arc::new(Classifier::new(&HashMap::new())))
    }

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
        let mgr = manager();
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
        let mgr = manager();
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
        let mgr = manager();
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
        let mgr = manager();
        assert!(mgr.status().unwrap().last.is_none());
        assert_eq!(mgr.listing(1, 0).unwrap_err().kind, ErrorKind::NotFound);
    }

    #[test]
    fn classifies_saved_scans_folders_and_files() {
        let dir = fixture();
        put(
            &dir.path()
                .join("web")
                .join("node_modules")
                .join("pkg")
                .join("f"),
            2 * MB,
        );
        let mgr = manager();
        let saved = run(&mgr, dir.path());
        let deps = saved
            .categories
            .iter()
            .find(|c| c.category == Category::DeveloperDependencies)
            .expect("developer dependencies counted");
        assert!(deps.bytes >= (2 * MB) as u64);
        let total: u64 = saved.categories.iter().map(|c| c.bytes).sum();
        assert_eq!(total, saved.stats.total_bytes);

        let top = mgr.listing(saved.scan_id, 0).unwrap();
        let web = top.children.iter().find(|c| c.name == "web").unwrap();
        assert!(web.classification.is_none(), "unknown stays unknown");
        let inside = mgr.listing(saved.scan_id, web.id).unwrap();
        let nm = &inside.children[0];
        assert_eq!(nm.name, "node_modules");
        let class = nm.classification.unwrap();
        assert_eq!(class.category, Category::DeveloperDependencies);
        assert_eq!(class.rule, "name.node-modules");

        let files = mgr.largest_files(saved.scan_id).unwrap();
        let f = files
            .iter()
            .find(|f| f.path.contains("node_modules"))
            .unwrap();
        assert_eq!(
            f.classification.map(|c| c.category),
            Some(Category::DeveloperDependencies)
        );
    }

    #[test]
    fn watches_after_a_scan_and_updates_only_what_changed() {
        let dir = fixture();
        let mgr = manager();
        let first = run(&mgr, dir.path());
        let st = mgr.watch_status().unwrap();
        assert!(st.watching && st.can_update_incrementally);
        // (Stray hints from antivirus or indexing touching the fixture may already be
        // pending; they are harmless, so the test waits for its own directories.)

        let inner = dir.path().join("big").join("inner");
        put(&inner.join("g"), 2 * MB);
        std::fs::remove_file(dir.path().join("top.bin")).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let seen = {
                let mut w = lock_any(&mgr.watch);
                let a = w.as_mut().unwrap();
                a.pump();
                a.acc.dirs.contains(&inner) && a.acc.dirs.contains(dir.path())
            };
            if seen || std::time::Instant::now() > deadline {
                assert!(seen, "the watcher never reported the changed folders");
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        let st = mgr.watch_status().unwrap();
        assert!(st.changed_dirs >= 2 && st.can_update_incrementally && !st.lost_events);

        let (rec, rx) = recorder();
        let id = mgr.update(rec).unwrap();
        let done = wait_done(&rx).unwrap_or_else(|e| panic!("update failed: {}", e.message));
        assert_eq!(done.id, id);
        assert_eq!(done.scan.previous.as_ref().unwrap().scan_id, first.scan_id);

        // The updated analysis equals a fresh one.
        let fresh = run(&manager(), dir.path());
        assert_eq!(done.scan.stats.total_bytes, fresh.stats.total_bytes);
        assert_eq!(done.scan.stats.files, fresh.stats.files);
        assert_eq!(done.scan.stats.dirs, fresh.stats.dirs);
        assert!(mgr.watch_status().unwrap().can_update_incrementally);
    }

    /// The 8.3 short spelling of `path`, if the volume has short names enabled.
    fn short_path(path: &std::path::Path) -> Option<std::path::PathBuf> {
        let out = std::process::Command::new("cmd")
            .arg("/C")
            .arg(format!("for %I in (\"{}\") do @echo %~sI", path.display()))
            .output()
            .ok()?;
        let p = std::path::PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
        (p.exists() && p != path).then_some(p)
    }

    #[test]
    fn a_root_given_through_a_junction_still_updates_incrementally() {
        // Same class of problem, reproducible on any volume: the analysis saves the canonical
        // root (links resolved), the watch keeps the spelling it was given.
        let dir = fixture();
        let link = dir.path().join("link-to-fixture-root");
        let real = dir.path().join("real-root");
        put(&real.join("big").join("f"), 3 * MB);
        let out = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&real)
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        let mgr = manager();
        run(&mgr, &link);
        let st = mgr.watch_status().unwrap();
        assert!(st.watching && st.can_update_incrementally, "{st:?}");
    }

    #[test]
    fn a_root_spelled_with_short_names_still_updates_incrementally() {
        // The analysis stores the canonical spelling of its root; the watch was started on
        // the spelling it was given. They are the same folder, so updates must stay incremental
        // (CI runners spell their temp folders with short names, which exposed this).
        let dir = fixture();
        let long = dir
            .path()
            .join("a long folder name that needs a short alias");
        put(&long.join("big").join("f"), 3 * MB);
        let Some(short) = short_path(&long) else {
            eprintln!("SKIPPED: 8.3 names are disabled on this volume");
            return;
        };
        let mgr = manager();
        run(&mgr, &short);
        let st = mgr.watch_status().unwrap();
        assert!(st.watching && st.can_update_incrementally, "{st:?}");
    }

    #[test]
    fn lost_events_fall_back_to_a_full_analysis() {
        let dir = fixture();
        let mgr = manager();
        run(&mgr, dir.path());
        lock_any(&mgr.watch).as_mut().unwrap().acc.root_dirty = true;
        let st = mgr.watch_status().unwrap();
        assert!(st.lost_events && !st.can_update_incrementally);

        put(&dir.path().join("late.bin"), 1000);
        let (rec, rx) = recorder();
        mgr.update(rec).unwrap();
        let done = wait_done(&rx).unwrap_or_else(|e| panic!("update failed: {}", e.message));
        assert_eq!(
            done.scan.stats.files, 4,
            "the full analysis saw the new file"
        );
        // A full analysis starts the watch afresh.
        let st = mgr.watch_status().unwrap();
        assert!(!st.lost_events && st.can_update_incrementally);
    }

    #[test]
    fn nothing_to_update_without_an_analysis() {
        let err = manager().update(recorder().0).unwrap_err();
        assert_eq!(err.kind, ErrorKind::NotFound);
        assert!(!manager().watch_status().unwrap().watching);
    }

    // ---- surviving a restart -------------------------------------------------------

    /// A journal whose position the test controls: (id, next USN), shared with the manager.
    fn fake_journal(initial: (u64, u64)) -> (Probe, Arc<Mutex<(u64, u64)>>) {
        let pos = Arc::new(Mutex::new(initial));
        let p = Arc::clone(&pos);
        let probe: Probe = Arc::new(move |_| {
            let (journal_id, next_usn) = *p.lock().unwrap();
            Ok(JournalPosition {
                journal_id,
                next_usn,
            })
        });
        (probe, pos)
    }

    /// A manager over `db` that saves its tree to `file`, as the app does. Creating a second
    /// one over the same database and file stands in for restarting the app.
    fn persistent(db: &Db, file: &std::path::Path, probe: Probe) -> ScanManager {
        ScanManager::new(db.clone(), Arc::new(Classifier::new(&HashMap::new())))
            .with_snapshot_file(file.to_path_buf())
            .with_probe(probe)
    }

    #[test]
    fn a_restart_over_an_untouched_drive_keeps_incremental_updates() {
        let dir = fixture();
        let state = tempfile::tempdir().unwrap();
        let file = state.path().join("scan-tree.bin");
        let db = Db::in_memory();
        let (probe, _pos) = fake_journal((7, 1000));

        let first = run(&persistent(&db, &file, probe.clone()), dir.path());
        assert!(file.exists(), "the tree was saved after the analysis");

        let after = persistent(&db, &file, probe);
        assert!(!after.watch_status().unwrap().can_update_incrementally);
        after.resume_watch_now();
        let st = after.watch_status().unwrap();
        assert_eq!(st.after_restart, Some(ResumeStatus::Verified));
        assert!(st.watching && st.can_update_incrementally && !st.lost_events);

        // And the restored tree really is usable: a change is picked up by an update that
        // re-reads only that folder, and the result is saved again.
        put(&dir.path().join("big").join("inner").join("g"), 2 * MB);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while after.watch_status().unwrap().changed_dirs == 0 {
            assert!(std::time::Instant::now() < deadline, "no change seen");
            std::thread::sleep(Duration::from_millis(50));
        }
        let (rec, rx) = recorder();
        after.update(rec).unwrap();
        let done = wait_done(&rx).unwrap_or_else(|e| panic!("update failed: {}", e.message));
        assert!(done.scan.scan_id > first.scan_id);
        assert_eq!(done.scan.stats.files, first.stats.files + 1);
        assert!(file.exists());
    }

    #[test]
    fn a_drive_that_changed_while_closed_is_reported_and_needs_a_full_analysis() {
        let dir = fixture();
        let state = tempfile::tempdir().unwrap();
        let file = state.path().join("scan-tree.bin");
        let db = Db::in_memory();
        let (probe, pos) = fake_journal((7, 1000));
        run(&persistent(&db, &file, probe.clone()), dir.path());

        *pos.lock().unwrap() = (7, 5000); // something changed while closed
        let after = persistent(&db, &file, probe);
        after.resume_watch_now();
        let st = after.watch_status().unwrap();
        assert_eq!(
            st.after_restart,
            Some(ResumeStatus::ChangedWhileClosed {
                journal_bytes: 4000
            })
        );
        assert!(!st.can_update_incrementally, "the tree is not trusted");
        assert!(!file.exists(), "and it is thrown away");
    }

    #[test]
    fn a_recreated_journal_is_reported_and_nothing_is_trusted() {
        let dir = fixture();
        let state = tempfile::tempdir().unwrap();
        let file = state.path().join("scan-tree.bin");
        let db = Db::in_memory();
        let (probe, pos) = fake_journal((7, 1000));
        run(&persistent(&db, &file, probe.clone()), dir.path());
        *pos.lock().unwrap() = (8, 1000);
        let after = persistent(&db, &file, probe);
        after.resume_watch_now();
        let st = after.watch_status().unwrap();
        assert_eq!(st.after_restart, Some(ResumeStatus::JournalReset));
        assert!(!st.can_update_incrementally);
    }

    #[test]
    fn an_unreadable_journal_means_nothing_is_saved_and_a_restart_is_a_full_analysis() {
        let dir = fixture();
        let state = tempfile::tempdir().unwrap();
        let file = state.path().join("scan-tree.bin");
        let db = Db::in_memory();
        let none: Probe = Arc::new(|_| Err(std::io::Error::other("no journal on this drive")));
        run(&persistent(&db, &file, none.clone()), dir.path());
        assert!(
            !file.exists(),
            "a tree that cannot be checked later is not kept"
        );
        let after = persistent(&db, &file, none);
        after.resume_watch_now();
        let st = after.watch_status().unwrap();
        assert!(st.after_restart.is_none() && !st.can_update_incrementally);
    }

    #[test]
    fn a_snapshot_of_an_older_analysis_or_a_damaged_file_is_discarded() {
        let dir = fixture();
        let state = tempfile::tempdir().unwrap();
        let file = state.path().join("scan-tree.bin");
        let db = Db::in_memory();
        let (probe, _pos) = fake_journal((7, 1000));
        run(&persistent(&db, &file, probe.clone()), dir.path());
        let good = std::fs::read(&file).unwrap();

        // Another analysis (by a manager that does not save a tree) supersedes it.
        run(&manager_over(&db), dir.path());
        let after = persistent(&db, &file, probe.clone());
        after.resume_watch_now();
        assert!(!after.watch_status().unwrap().can_update_incrementally);
        assert!(!file.exists());

        // Damage: flipped bytes, and a truncated file.
        for damaged in [
            {
                let mut b = good.clone();
                let mid = b.len() / 2;
                b[mid] ^= 0xFF;
                b
            },
            good[..good.len() / 2].to_vec(),
            b"garbage".to_vec(),
        ] {
            std::fs::write(&file, damaged).unwrap();
            let again = persistent(&db, &file, probe.clone());
            again.resume_watch_now();
            assert!(!again.watch_status().unwrap().can_update_incrementally);
            assert!(!file.exists(), "a damaged file is removed");
        }
    }

    fn manager_over(db: &Db) -> ScanManager {
        ScanManager::new(db.clone(), Arc::new(Classifier::new(&HashMap::new())))
    }

    #[test]
    fn a_new_analysis_replaces_the_saved_tree_and_forgets_the_restart_note() {
        let dir = fixture();
        let state = tempfile::tempdir().unwrap();
        let file = state.path().join("scan-tree.bin");
        let db = Db::in_memory();
        let (probe, pos) = fake_journal((7, 1000));
        run(&persistent(&db, &file, probe.clone()), dir.path());
        *pos.lock().unwrap() = (7, 2000);
        let after = persistent(&db, &file, probe);
        after.resume_watch_now();
        assert!(after.watch_status().unwrap().after_restart.is_some());

        let second = run(&after, dir.path());
        assert!(after.watch_status().unwrap().after_restart.is_none());
        assert!(file.exists(), "the new analysis wrote its own tree");
        let snap = persist::decode(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(snap.scan_id, second.scan_id);
        assert_eq!(
            snap.journal.next_usn, 2000,
            "the position before the new scan"
        );
    }

    #[test]
    fn a_restore_overtaken_by_a_new_analysis_installs_nothing() {
        let dir = fixture();
        let state = tempfile::tempdir().unwrap();
        let file = state.path().join("scan-tree.bin");
        let db = Db::in_memory();
        let (probe, _pos) = fake_journal((7, 1000));
        run(&persistent(&db, &file, probe.clone()), dir.path());
        let after = persistent(&db, &file, probe);
        let ctx = after.restore_ctx().unwrap();
        let stale_epoch = after.epoch.load(Ordering::SeqCst);
        after.new_baseline(); // an analysis begins while the restore is still reading
        restore(&ctx, stale_epoch);
        assert!(lock_any(&after.retained).is_none());
        assert!(lock_any(&after.resume).is_none());
    }
}
