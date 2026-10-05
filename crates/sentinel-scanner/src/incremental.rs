//! Incremental rescan: bring an existing [`ScanTree`] up to date by re-reading only the
//! directories a watcher reported as changed.
//!
//! The result is meant to equal what a fresh [`scan`](crate::scan::scan) would produce
//! now, for everything the dirty set covers. Two things are *not* recoverable from a
//! partial rescan and are reported instead of guessed:
//! - [`Rescan::largest_files_may_be_incomplete`]: the largest-files list is rebuilt from
//!   the entries that were not touched plus what the rescanned directories contain; when
//!   files left the list and nothing known can take their place, the list may be missing
//!   a file that only a full scan would find.
//! - A watcher that lost events ([`Changes::root_dirty`]) means "rescan the root": this
//!   function refuses and returns [`RescanError::FullScanNeeded`].
//!
//! The tree is only modified when the whole rescan succeeds; a cancelled or failed
//! rescan leaves it exactly as it was.
//!
//! Cost: the arena is rebuilt, which is linear in the tree's size (milliseconds per
//! million nodes) but independent of how little changed; the directory reads, which
//! dominate a scan, are limited to the dirty set.

use std::cmp::Reverse;
use std::path::{Path, PathBuf};

use crate::scan::{
    DirNode, LargeFile, NodeStatus, NotScannedReason, ScanControl, ScanError, ScanOptions,
    ScanStats, ScanTree, Walked, flatten, run_walks,
};
use crate::watch::Changes;

#[derive(Debug, thiserror::Error)]
pub enum RescanError {
    #[error("events were lost, so the whole root must be scanned again")]
    FullScanNeeded,
    #[error("the rescan was cancelled; the tree was not changed")]
    Cancelled,
    #[error(transparent)]
    Scan(#[from] ScanError),
}

/// What a rescan did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rescan {
    /// Directories re-read (one level each).
    pub dirs_refreshed: u64,
    /// Noisy subtrees re-read in full.
    pub subtrees_rewalked: u64,
    /// Reported paths the tree has no node for (not scanned, excluded, or inside a link).
    pub skipped_unknown: u64,
    /// See the module docs.
    pub largest_files_may_be_incomplete: bool,
}

fn key(s: &str) -> String {
    s.to_lowercase()
}

fn strip_verbatim(p: &Path) -> String {
    let s = p.to_string_lossy();
    s.strip_prefix(r"\\?\").unwrap_or(&s).to_owned()
}

/// Components of `path` below `root`, compared case-insensitively. `None` if outside.
fn relative(root: &Path, path: &Path) -> Option<Vec<String>> {
    let r: Vec<String> = strip_verbatim(root)
        .split('\\')
        .filter(|c| !c.is_empty())
        .map(str::to_owned)
        .collect();
    let p: Vec<String> = strip_verbatim(path)
        .split('\\')
        .filter(|c| !c.is_empty())
        .map(str::to_owned)
        .collect();
    (p.len() >= r.len() && r.iter().zip(&p).all(|(a, b)| key(a) == key(b)))
        .then(|| p[r.len()..].to_vec())
}

/// Rebuild the walked tree from the arena. File counts and logical sizes are stored for
/// whole subtrees, so a directory's own share is its total minus its children's.
fn to_walked(nodes: &[DirNode], id: usize) -> Walked {
    let n = &nodes[id];
    let children: Vec<Walked> = n
        .children
        .iter()
        .map(|&c| to_walked(nodes, c as usize))
        .collect();
    let (cf, cl) = n.children.iter().fold((0u64, 0u64), |(f, l), &c| {
        let c = &nodes[c as usize];
        (f + c.file_count, l + c.total_logical_bytes)
    });
    Walked {
        name: n.name.clone(),
        own_bytes: n.own_bytes,
        own_logical: n.total_logical_bytes.saturating_sub(cl),
        own_files: n.file_count.saturating_sub(cf),
        status: n.status.clone(),
        children,
    }
}

fn find_mut<'a>(root: &'a mut Walked, rel: &[String]) -> Option<&'a mut Walked> {
    let mut cur = root;
    for part in rel {
        let i = cur
            .children
            .iter()
            .position(|c| key(&c.name) == key(part))?;
        cur = &mut cur.children[i];
    }
    Some(cur)
}

/// A real directory that was read (as opposed to a link, an excluded or online-only
/// placeholder, or an unreadable one).
fn is_plain_dir(w: &Walked) -> bool {
    !matches!(
        w.status,
        NodeStatus::Link { .. }
            | NodeStatus::Excluded
            | NodeStatus::NotScanned {
                reason: NotScannedReason::OnlineOnly
            }
    )
}

fn count_statuses(nodes: &[DirNode]) -> (u64, u64, u64, u64, u64) {
    let (mut dirs, mut denied, mut errors, mut links, mut excluded) = (0, 0, 0, 0, 0);
    for n in nodes {
        match n.status {
            NodeStatus::Complete => dirs += 1,
            NodeStatus::AccessDenied => denied += 1,
            NodeStatus::Error { .. } => errors += 1,
            NodeStatus::Link { .. } => links += 1,
            NodeStatus::Excluded => excluded += 1,
            NodeStatus::NotScanned { .. } => {}
        }
    }
    (dirs, denied, errors, links, excluded)
}

/// Apply `changes` to `tree`. Read-only with respect to the file system.
///
/// Returns [`RescanError::FullScanNeeded`] when `changes.root_dirty` is set.
pub fn rescan(
    tree: &mut ScanTree,
    changes: &Changes,
    opts: &ScanOptions,
    ctl: &ScanControl,
) -> Result<Rescan, RescanError> {
    if changes.root_dirty {
        return Err(RescanError::FullScanNeeded);
    }
    let mut report = Rescan::default();
    let mut root = to_walked(&tree.nodes, 0);

    // Shorter paths first, so a new directory is created before anything inside it is
    // refreshed (the later refresh then simply finds it up to date).
    let mut dirs: Vec<&PathBuf> = changes.dirty_dirs.iter().collect();
    dirs.sort_by_key(|p| (p.components().count(), p.to_string_lossy().to_lowercase()));
    let mut subtrees: Vec<&PathBuf> = changes.dirty_subtrees.iter().collect();
    subtrees.sort_by_key(|p| p.components().count());

    let tree_root = tree.root.clone();
    let max_depth = opts.max_depth;
    let ((), extras) = run_walks(opts, ctl, |ctx| {
        for d in &dirs {
            let Some(rel) = relative(&tree_root, d) else {
                report.skipped_unknown += 1;
                continue;
            };
            let depth = rel.len() as u32;
            let Some(node) = find_mut(&mut root, &rel) else {
                report.skipped_unknown += 1;
                continue;
            };
            if !is_plain_dir(node) || depth > max_depth {
                report.skipped_unknown += 1;
                continue;
            }
            let path = d.as_path();
            let fresh = ctx.walk_with(path, node.name.clone(), depth, true);
            report.dirs_refreshed += 1;
            // The directory itself could not be read any more (deleted, access lost).
            if !matches!(fresh.status, NodeStatus::Complete) {
                *node = fresh;
                continue;
            }
            let mut old: Vec<Walked> = std::mem::take(&mut node.children);
            let mut kept = Vec::with_capacity(fresh.children.len());
            for c in fresh.children {
                let is_subdir = matches!(
                    c.status,
                    NodeStatus::NotScanned {
                        reason: NotScannedReason::DepthLimit
                    }
                );
                if !is_subdir {
                    kept.push(c); // link, excluded or online-only: as just read
                    continue;
                }
                match old.iter().position(|o| key(&o.name) == key(&c.name)) {
                    Some(i) if is_plain_dir(&old[i]) => kept.push(old.swap_remove(i)),
                    _ => {
                        // A directory that was not there before: read it completely.
                        let sub = path.join(&c.name);
                        kept.push(ctx.walk(&sub, c.name, depth + 1));
                    }
                }
            }
            node.own_bytes = fresh.own_bytes;
            node.own_logical = fresh.own_logical;
            node.own_files = fresh.own_files;
            node.status = fresh.status;
            node.children = kept;
        }
        for t in &subtrees {
            let Some(rel) = relative(&tree_root, t) else {
                report.skipped_unknown += 1;
                continue;
            };
            let depth = rel.len() as u32;
            match find_mut(&mut root, &rel) {
                Some(node) if is_plain_dir(node) && !rel.is_empty() => {
                    *node = ctx.walk(t.as_path(), node.name.clone(), depth);
                    report.subtrees_rewalked += 1;
                }
                _ => report.skipped_unknown += 1,
            }
        }
    })?;
    if ctl.is_cancelled() {
        return Err(RescanError::Cancelled);
    }

    let mut nodes = Vec::with_capacity(tree.nodes.len());
    flatten(root, None, &mut nodes);

    // Largest files: keep what the rescan did not touch, add what it saw.
    let limit = opts.largest_files;
    let touched = |p: &str| -> bool {
        let parent = Path::new(p).parent();
        dirs.iter()
            .any(|d| parent.is_some_and(|pp| relative(d, pp).is_some_and(|r| r.is_empty())))
            || subtrees.iter().any(|t| relative(t, Path::new(p)).is_some())
    };
    let old_full = limit > 0 && tree.largest_files.len() >= limit;
    let old_floor = tree.largest_files.last().map_or(0, |f| f.bytes);
    let before = tree.largest_files.len();
    let mut largest: Vec<LargeFile> = tree
        .largest_files
        .iter()
        .filter(|f| !touched(&f.path) && Path::new(&f.path).exists())
        .cloned()
        .collect();
    let removed = before - largest.len();
    largest.extend(extras.largest);
    largest.sort_by_key(|f| Reverse((f.bytes, f.logical_bytes)));
    largest.dedup_by(|a, b| a.path.eq_ignore_ascii_case(&b.path));
    if limit > 0 {
        largest.truncate(limit);
    }
    report.largest_files_may_be_incomplete = removed > 0
        && limit > 0
        && (largest.len() < limit
            || (old_full && largest.last().is_some_and(|f| f.bytes < old_floor)));

    let (dirs_count, denied, errors, links, excluded) = count_statuses(&nodes);
    let r = &nodes[0];
    let stats = ScanStats {
        dirs: dirs_count,
        files: r.file_count,
        total_bytes: r.total_bytes,
        total_logical_bytes: r.total_logical_bytes,
        access_denied: denied,
        errors,
        links_skipped: links,
        excluded,
        cancelled: false,
        truncated: tree.stats.truncated || extras.truncated,
        elapsed_ms: tree.stats.elapsed_ms,
    };
    tree.nodes = nodes;
    tree.largest_files = largest;
    tree.stats = stats;
    tracing::info!(
        root = %tree.root.display(),
        dirs = report.dirs_refreshed,
        subtrees = report.subtrees_rewalked,
        skipped = report.skipped_unknown,
        "incremental rescan finished"
    );
    Ok(report)
}
