//! Parallel, cancellable, read-only directory scanner.
//!
//! Guarantees:
//! - Never follows a junction, symlink or mount point; each one becomes a
//!   [`NodeStatus::Link`] node. Cloud-files directories (OneDrive) are real directories
//!   on the same volume and *are* descended, unless they are online-only, in which case
//!   opening them would make the sync provider fetch data.
//! - Bounded: a fixed-size thread pool, a maximum depth and a maximum entry count.
//! - Cancellable at directory granularity; partial results are returned and flagged.
//! - Sizes are *allocated* bytes (space actually used on disk), with logical sizes kept
//!   alongside.
//!
//! Known limitation: hard-linked files are counted once per link (as Explorer does),
//! which over-reports folders such as `C:\Windows\WinSxS`.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::time::Instant;

use rayon::prelude::*;
use sentinel_safety::{CanonicalPath, SafetyError, is_within};
use serde::Serialize;
use ts_rs::TS;

use crate::dirent::{RawEntry, read_dir};

pub type NodeId = u32;

const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;
const IO_REPARSE_TAG_SYMLINK: u32 = 0xA000_000C;
const FILE_ATTRIBUTE_RECALL_ON_OPEN: u32 = 0x0004_0000;

fn is_cloud_tag(tag: u32) -> bool {
    tag & 0xFFFF_0FFF == 0x9000_001A
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum LinkKind {
    /// Directory junction or volume mount point.
    Junction,
    Symlink,
    /// Any other reparse point Sentinel does not traverse.
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum NotScannedReason {
    Cancelled,
    DepthLimit,
    EntryLimit,
    /// Cloud directory whose contents are not stored on this computer.
    OnlineOnly,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase", tag = "state")]
#[ts(export)]
pub enum NodeStatus {
    Complete,
    AccessDenied,
    Error { message: String },
    Link { kind: LinkKind },
    Excluded,
    NotScanned { reason: NotScannedReason },
}

#[derive(Debug, Clone)]
pub struct DirNode {
    pub name: String,
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
    /// Allocated bytes of files directly in this directory.
    pub own_bytes: u64,
    /// Allocated bytes of the whole subtree.
    pub total_bytes: u64,
    pub total_logical_bytes: u64,
    /// Files in the whole subtree.
    pub file_count: u64,
    /// Directories in the subtree, excluding this one.
    pub dir_count: u64,
    pub status: NodeStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LargeFile {
    pub path: String,
    #[ts(type = "number")]
    pub bytes: u64,
    #[ts(type = "number")]
    pub logical_bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ScanStats {
    #[ts(type = "number")]
    pub dirs: u64,
    #[ts(type = "number")]
    pub files: u64,
    #[ts(type = "number")]
    pub total_bytes: u64,
    #[ts(type = "number")]
    pub total_logical_bytes: u64,
    #[ts(type = "number")]
    pub access_denied: u64,
    #[ts(type = "number")]
    pub errors: u64,
    #[ts(type = "number")]
    pub links_skipped: u64,
    #[ts(type = "number")]
    pub excluded: u64,
    /// The user cancelled; totals are a lower bound.
    pub cancelled: bool,
    /// A depth or entry budget was hit; totals are a lower bound.
    pub truncated: bool,
    #[ts(type = "number")]
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone)]
pub struct ScanOptions {
    /// Worker threads. Directory listing is I/O-bound; a few threads saturate an SSD.
    pub threads: usize,
    pub max_depth: u32,
    pub max_entries: u64,
    pub exclude: Vec<PathBuf>,
    pub largest_files: usize,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            threads: 4,
            max_depth: 256,
            max_entries: 50_000_000,
            exclude: Vec::new(),
            largest_files: 50,
        }
    }
}

/// Shared between the scan and its observers: cancellation plus live counters.
#[derive(Debug, Default)]
pub struct ScanControl {
    cancelled: AtomicBool,
    dirs: AtomicU64,
    files: AtomicU64,
    bytes: AtomicU64,
    problems: AtomicU64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ScanProgress {
    #[ts(type = "number")]
    pub dirs: u64,
    #[ts(type = "number")]
    pub files: u64,
    #[ts(type = "number")]
    pub bytes: u64,
    /// Directories that could not be read.
    #[ts(type = "number")]
    pub problems: u64,
}

impl ScanControl {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Relaxed)
    }

    pub fn progress(&self) -> ScanProgress {
        ScanProgress {
            dirs: self.dirs.load(Relaxed),
            files: self.files.load(Relaxed),
            bytes: self.bytes.load(Relaxed),
            problems: self.problems.load(Relaxed),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ScanError {
    #[error("cannot scan this location: {0}")]
    InvalidRoot(#[from] SafetyError),
    #[error("{0} is not a folder")]
    NotADirectory(PathBuf),
    #[error("could not start scanner threads: {0}")]
    ThreadPool(String),
}

/// Result of a scan: a directory tree in an arena (node 0 is the root).
#[derive(Debug, Clone)]
pub struct ScanTree {
    pub root: PathBuf,
    pub nodes: Vec<DirNode>,
    pub largest_files: Vec<LargeFile>,
    pub stats: ScanStats,
}

impl ScanTree {
    pub const ROOT: NodeId = 0;

    pub fn node(&self, id: NodeId) -> Option<&DirNode> {
        self.nodes.get(id as usize)
    }

    /// Full path of a node, rebuilt from its ancestors.
    pub fn path_of(&self, id: NodeId) -> Option<PathBuf> {
        let mut names = Vec::new();
        let mut cur = Some(id);
        while let Some(i) = cur {
            let n = self.node(i)?;
            if i == Self::ROOT {
                let mut p = self.root.clone();
                p.extend(names.iter().rev());
                return Some(p);
            }
            names.push(n.name.as_str());
            cur = n.parent;
        }
        None
    }

    /// Children of `id`, largest first.
    pub fn children_by_size(&self, id: NodeId) -> Vec<NodeId> {
        let Some(n) = self.node(id) else {
            return Vec::new();
        };
        let mut kids = n.children.clone();
        kids.sort_by_key(|&k| Reverse(self.nodes[k as usize].total_bytes));
        kids
    }
}

struct Ctx<'a> {
    ctl: &'a ScanControl,
    opts: &'a ScanOptions,
    exclude: Vec<PathBuf>,
    entries: AtomicU64,
    denied: AtomicU64,
    errors: AtomicU64,
    links: AtomicU64,
    excluded: AtomicU64,
    truncated: AtomicBool,
    largest: Mutex<BinaryHeap<Reverse<(u64, u64, PathBuf)>>>,
    /// Smallest size currently in a full `largest` heap; lets most files skip the lock.
    largest_floor: AtomicU64,
}

struct Walked {
    name: String,
    own_bytes: u64,
    own_logical: u64,
    own_files: u64,
    status: NodeStatus,
    children: Vec<Walked>,
}

impl Walked {
    fn leaf(name: String, status: NodeStatus) -> Self {
        Self {
            name,
            own_bytes: 0,
            own_logical: 0,
            own_files: 0,
            status,
            children: Vec::new(),
        }
    }
}

impl Ctx<'_> {
    fn is_excluded(&self, path: &Path) -> bool {
        self.exclude.iter().any(|e| is_within(path, e))
    }

    fn consider_largest(&self, dir: &Path, e: &RawEntry) {
        let limit = self.opts.largest_files;
        if limit == 0 || e.allocated_bytes <= self.largest_floor.load(Relaxed) {
            return;
        }
        let Ok(mut heap) = self.largest.lock() else {
            return;
        };
        heap.push(Reverse((
            e.allocated_bytes,
            e.logical_bytes,
            dir.join(&e.name),
        )));
        if heap.len() > limit {
            heap.pop();
        }
        if heap.len() == limit
            && let Some(Reverse((min, _, _))) = heap.peek()
        {
            self.largest_floor.store(*min, Relaxed);
        }
    }

    fn walk(&self, path: &Path, name: String, depth: u32) -> Walked {
        if self.ctl.is_cancelled() {
            return Walked::leaf(
                name,
                NodeStatus::NotScanned {
                    reason: NotScannedReason::Cancelled,
                },
            );
        }
        let entries = match read_dir(path) {
            Ok(e) => e,
            Err(err) => {
                self.ctl.problems.fetch_add(1, Relaxed);
                let status = if err.kind() == io::ErrorKind::PermissionDenied {
                    self.denied.fetch_add(1, Relaxed);
                    NodeStatus::AccessDenied
                } else {
                    self.errors.fetch_add(1, Relaxed);
                    tracing::debug!(path = %path.display(), error = %err, "directory unreadable");
                    NodeStatus::Error {
                        message: err.to_string(),
                    }
                };
                return Walked::leaf(name, status);
            }
        };
        self.ctl.dirs.fetch_add(1, Relaxed);
        let over_budget = self.entries.fetch_add(entries.len() as u64, Relaxed)
            + entries.len() as u64
            > self.opts.max_entries;

        let mut node = Walked::leaf(name, NodeStatus::Complete);
        let mut subdirs = Vec::new();
        for e in entries {
            if !e.is_dir() {
                node.own_files += 1;
                node.own_bytes += e.allocated_bytes;
                node.own_logical += e.logical_bytes;
                self.consider_largest(path, &e);
                continue;
            }
            if e.is_reparse_point() && !is_cloud_tag(e.reparse_tag) {
                self.links.fetch_add(1, Relaxed);
                let kind = match e.reparse_tag {
                    IO_REPARSE_TAG_MOUNT_POINT => LinkKind::Junction,
                    IO_REPARSE_TAG_SYMLINK => LinkKind::Symlink,
                    _ => LinkKind::Other,
                };
                node.children
                    .push(Walked::leaf(e.name, NodeStatus::Link { kind }));
                continue;
            }
            if e.attributes & FILE_ATTRIBUTE_RECALL_ON_OPEN != 0 {
                node.children.push(Walked::leaf(
                    e.name,
                    NodeStatus::NotScanned {
                        reason: NotScannedReason::OnlineOnly,
                    },
                ));
                continue;
            }
            let child = path.join(&e.name);
            if self.is_excluded(&child) {
                self.excluded.fetch_add(1, Relaxed);
                node.children
                    .push(Walked::leaf(e.name, NodeStatus::Excluded));
                continue;
            }
            subdirs.push((child, e.name));
        }
        self.ctl.files.fetch_add(node.own_files, Relaxed);
        self.ctl.bytes.fetch_add(node.own_bytes, Relaxed);

        let limit = if over_budget {
            Some(NotScannedReason::EntryLimit)
        } else if depth >= self.opts.max_depth {
            Some(NotScannedReason::DepthLimit)
        } else {
            None
        };
        if let Some(reason) = limit {
            if !subdirs.is_empty() {
                self.truncated.store(true, Relaxed);
            }
            node.children.extend(
                subdirs
                    .into_iter()
                    .map(|(_, n)| Walked::leaf(n, NodeStatus::NotScanned { reason })),
            );
        } else {
            let walked: Vec<Walked> = subdirs
                .into_par_iter()
                .map(|(p, n)| self.walk(&p, n, depth + 1))
                .collect();
            node.children.extend(walked);
        }
        node
    }
}

/// Scan `root` and everything beneath it. Read-only.
pub fn scan(root: &Path, opts: &ScanOptions, ctl: &ScanControl) -> Result<ScanTree, ScanError> {
    let started = Instant::now();
    let root = CanonicalPath::resolve(root)?.as_path().to_path_buf();
    if !root.is_dir() {
        return Err(ScanError::NotADirectory(root));
    }
    let exclude = opts
        .exclude
        .iter()
        .map(|e| {
            CanonicalPath::resolve(e)
                .map(|c| c.as_path().to_path_buf())
                .unwrap_or_else(|_| e.clone())
        })
        .collect();
    let ctx = Ctx {
        ctl,
        opts,
        exclude,
        entries: AtomicU64::new(0),
        denied: AtomicU64::new(0),
        errors: AtomicU64::new(0),
        links: AtomicU64::new(0),
        excluded: AtomicU64::new(0),
        truncated: AtomicBool::new(false),
        largest: Mutex::new(BinaryHeap::new()),
        largest_floor: AtomicU64::new(0),
    };
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(opts.threads.max(1))
        .stack_size(8 * 1024 * 1024)
        .thread_name(|i| format!("sentinel-scan-{i}"))
        .build()
        .map_err(|e| ScanError::ThreadPool(e.to_string()))?;
    let walked = pool.install(|| ctx.walk(&root, root.display().to_string(), 0));

    let mut nodes = Vec::new();
    flatten(walked, None, &mut nodes);

    let mut largest: Vec<_> = ctx
        .largest
        .into_inner()
        .unwrap_or_default()
        .into_iter()
        .map(|Reverse((bytes, logical_bytes, path))| LargeFile {
            path: path.display().to_string(),
            bytes,
            logical_bytes,
        })
        .collect();
    // Same order the heap keeps: allocated size, then logical size as tie-breaker.
    largest.sort_by_key(|f| Reverse((f.bytes, f.logical_bytes)));

    let r = &nodes[0];
    let stats = ScanStats {
        dirs: ctl.dirs.load(Relaxed),
        files: r.file_count,
        total_bytes: r.total_bytes,
        total_logical_bytes: r.total_logical_bytes,
        access_denied: ctx.denied.load(Relaxed),
        errors: ctx.errors.load(Relaxed),
        links_skipped: ctx.links.load(Relaxed),
        excluded: ctx.excluded.load(Relaxed),
        cancelled: ctl.is_cancelled(),
        truncated: ctx.truncated.load(Relaxed),
        elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    };
    tracing::info!(
        root = %root.display(),
        dirs = stats.dirs,
        files = stats.files,
        bytes = stats.total_bytes,
        cancelled = stats.cancelled,
        truncated = stats.truncated,
        elapsed_ms = stats.elapsed_ms,
        "scan finished"
    );
    Ok(ScanTree {
        root,
        nodes,
        largest_files: largest,
        stats,
    })
}

/// Move a walked subtree into the arena, computing subtree totals. Returns its id.
fn flatten(w: Walked, parent: Option<NodeId>, nodes: &mut Vec<DirNode>) -> NodeId {
    let id = nodes.len() as NodeId;
    nodes.push(DirNode {
        name: w.name,
        parent,
        children: Vec::with_capacity(w.children.len()),
        own_bytes: w.own_bytes,
        total_bytes: w.own_bytes,
        total_logical_bytes: w.own_logical,
        file_count: w.own_files,
        dir_count: 0,
        status: w.status,
    });
    for child in w.children {
        let cid = flatten(child, Some(id), nodes);
        let (tb, tl, fc, dc, scanned) = {
            let c = &nodes[cid as usize];
            (
                c.total_bytes,
                c.total_logical_bytes,
                c.file_count,
                c.dir_count,
                matches!(c.status, NodeStatus::Complete),
            )
        };
        let n = &mut nodes[id as usize];
        n.children.push(cid);
        n.total_bytes += tb;
        n.total_logical_bytes += tl;
        n.file_count += fc;
        n.dir_count += dc + u64::from(scanned);
    }
    id
}
