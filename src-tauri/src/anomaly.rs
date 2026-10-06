//! Anomaly attribution: what grew (or shrank) most between two analyses of a root, and
//! which folders account for it.
//!
//! Method: descend from the root, comparing each folder with the same path in the baseline
//! analysis, and go only into folders whose change is large enough to matter. A folder
//! where no child is individually responsible is a *leaf* and is reported with its change
//! and what drives it. What the leaves do not account for is reported as unattributed.
//!
//! What is and is not claimed:
//! - Only folders the analyses stored (1 MB and up) can be followed. A folder that was
//!   smaller than that in the baseline has no exact earlier size; its change is then a
//!   *lower bound* and says so.
//! - Descent is by magnitude, not sign, so growth in one place is not hidden by shrinkage
//!   in another.
//! - Incomplete (cancelled or truncated) analyses are not compared.
//! - A file is called new or grown only when the baseline's largest-files list makes that
//!   certain, since a list of the top 50 cannot show absence.
//! - These are changes in allocated size between two moments. Nothing here says why a
//!   folder grew or whether it should be cleaned.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use sentinel_classify::Classification;
use sentinel_store::{ScanId, ScanRecord, Store, StoredNode};
use serde::Serialize;
use ts_rs::TS;

use crate::commands::error::{CommandError, ErrorKind};
use crate::db::Db;
use crate::history::FolderState;

/// Changes smaller than this are never reported on their own.
const MIN_CHANGE_BYTES: u64 = 10 * 1024 * 1024;
/// A change must also be at least this share of the net change of the whole root.
const MIN_SHARE: f64 = 0.05;
const MAX_CONTRIBUTORS: usize = 8;
const MAX_DEPTH: usize = 64;

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AnalysisRef {
    #[ts(type = "number")]
    pub scan_id: ScanId,
    #[ts(type = "number")]
    pub finished_at_ms: i64,
    #[ts(type = "number")]
    pub total_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ChangeKind {
    /// The folder did not exist in the baseline.
    Added,
    Grew,
    Shrank,
    /// The folder no longer exists.
    Removed,
}

/// What a leaf folder's change is mostly made of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Driver {
    /// Files directly inside the folder.
    OwnFiles,
    /// Many small folders (each under the storage threshold) added together.
    SmallFolders,
    /// Several subfolders, none large enough to single out.
    ManyFolders,
    /// The folder as a whole (it was added or removed).
    WholeFolder,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Contributor {
    pub path: String,
    pub kind: ChangeKind,
    /// Positive for growth, negative for shrinkage. A lower bound when `baseline` is
    /// `belowThreshold` (growth) because the earlier size is only known to be small.
    #[ts(type = "number")]
    pub change_bytes: i64,
    #[ts(type = "number")]
    pub now_bytes: u64,
    /// Size in the baseline, when it was stored.
    #[ts(type = "number | null")]
    pub before_bytes: Option<u64>,
    pub baseline: FolderState,
    /// Share of the root's net change; can exceed 100 when other folders shrank.
    pub share_of_net_change: Option<f64>,
    pub driver: Driver,
    pub classification: Option<Classification>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FileChange {
    pub path: String,
    #[ts(type = "number")]
    pub bytes: u64,
    /// Size in the baseline's largest files, if it was listed there.
    #[ts(type = "number | null")]
    pub before_bytes: Option<u64>,
    pub classification: Option<Classification>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AnomalyReport {
    pub root: String,
    pub baseline: AnalysisRef,
    pub current: AnalysisRef,
    /// Net change of the whole root, positive or negative.
    #[ts(type = "number")]
    pub net_change_bytes: i64,
    /// Folders that grew, largest first.
    pub grew: Vec<Contributor>,
    /// Folders that shrank, largest first.
    pub shrank: Vec<Contributor>,
    /// Net change the listed folders do not account for: many small changes, files in
    /// folders that were not singled out, and anything below the reporting threshold.
    #[ts(type = "number")]
    pub unattributed_bytes: i64,
    /// Large files that are new or bigger than in the baseline.
    pub large_files: Vec<FileChange>,
    /// Smallest change reported on its own.
    #[ts(type = "number")]
    pub reporting_threshold_bytes: u64,
    pub caveats: Vec<String>,
}

fn invalid(msg: &str) -> CommandError {
    CommandError::new(ErrorKind::InvalidInput, msg)
}

fn partial(r: &ScanRecord) -> bool {
    r.stats.cancelled || r.stats.truncated
}

fn analysis_ref(r: &ScanRecord) -> AnalysisRef {
    AnalysisRef {
        scan_id: r.id,
        finished_at_ms: r.finished_at_ms,
        total_bytes: r.stats.total_bytes,
    }
}

/// Where a folder's earlier size comes from.
#[derive(Clone)]
enum Base {
    /// Stored in the baseline.
    Exact(StoredNode),
    /// Too small to have been stored: below this many bytes.
    Below(u64),
    /// Not in the baseline at all.
    Missing,
}

struct Walk<'a> {
    store: &'a Store,
    to: &'a ScanRecord,
    from: &'a ScanRecord,
    threshold: u64,
    leaves: Vec<Leaf>,
}

struct Leaf {
    names: Vec<String>,
    node: StoredNode,
    base_state: FolderState,
    before: Option<u64>,
    change: i64,
    driver: Driver,
    removed: bool,
}

fn signed(a: u64, b: u64) -> i64 {
    i64::try_from(i128::from(a) - i128::from(b)).unwrap_or(0)
}

/// Change of `now` against its baseline, and the baseline size when it is known. Against
/// a folder that was only known to be small, the smallest possible growth.
fn delta(now: &StoredNode, base: &Base) -> (i64, Option<u64>) {
    match base {
        Base::Exact(f) => (signed(now.total_bytes, f.total_bytes), Some(f.total_bytes)),
        Base::Below(max) => (signed(now.total_bytes, (*max).min(now.total_bytes)), None),
        Base::Missing => (signed(now.total_bytes, 0), Some(0)),
    }
}

fn driver_of(node: &StoredNode, base: &Base, change: i64) -> Driver {
    let Base::Exact(f) = base else {
        return Driver::WholeFolder;
    };
    let own = signed(node.own_bytes, f.own_bytes);
    let small = signed(node.pruned_bytes, f.pruned_bytes);
    let rest = change - own - small;
    let (o, s, r) = (
        own.unsigned_abs(),
        small.unsigned_abs(),
        rest.unsigned_abs(),
    );
    if o >= s && o >= r {
        Driver::OwnFiles
    } else if s >= r {
        Driver::SmallFolders
    } else {
        Driver::ManyFolders
    }
}

impl Walk<'_> {
    fn explore(
        &mut self,
        node: StoredNode,
        base: Base,
        names: Vec<String>,
    ) -> Result<(), CommandError> {
        let kids = self.store.children(self.to.id, node.id, u32::MAX)?;
        let (from_kids, had_pruned): (HashMap<String, StoredNode>, bool) = match &base {
            Base::Exact(f) => (
                self.store
                    .children(self.from.id, f.id, u32::MAX)?
                    .into_iter()
                    .map(|n| (n.name.to_lowercase(), n))
                    .collect(),
                f.pruned_children > 0,
            ),
            _ => (HashMap::new(), false),
        };
        let mut descended = false;
        if names.len() < MAX_DEPTH {
            for c in &kids {
                let cb = match &base {
                    Base::Exact(_) => match from_kids.get(&c.name.to_lowercase()) {
                        Some(f) => Base::Exact(f.clone()),
                        // Not stored: small (the parent dropped some folders) or absent.
                        None if had_pruned => Base::Below(self.from.min_node_bytes),
                        None => Base::Missing,
                    },
                    // Under a folder that was small, everything inside was small too.
                    other => other.clone(),
                };
                if delta(c, &cb).0.unsigned_abs() >= self.threshold {
                    descended = true;
                    let mut n = names.clone();
                    n.push(c.name.clone());
                    self.explore(c.clone(), cb, n)?;
                }
            }
        }
        // Folders that were in the baseline and are gone now.
        if matches!(base, Base::Exact(_)) {
            let now: std::collections::HashSet<String> =
                kids.iter().map(|k| k.name.to_lowercase()).collect();
            for (key, f) in &from_kids {
                if !now.contains(key) && f.total_bytes >= self.threshold {
                    descended = true;
                    let mut n = names.clone();
                    n.push(f.name.clone());
                    self.leaves.push(Leaf {
                        names: n,
                        node: StoredNode {
                            total_bytes: 0,
                            ..f.clone()
                        },
                        base_state: FolderState::Known,
                        before: Some(f.total_bytes),
                        change: -signed(f.total_bytes, 0),
                        driver: Driver::WholeFolder,
                        removed: true,
                    });
                }
            }
        }
        if descended {
            return Ok(());
        }
        let (change, before) = delta(&node, &base);
        if change.unsigned_abs() < self.threshold {
            return Ok(());
        }
        self.leaves.push(Leaf {
            names,
            base_state: match base {
                Base::Exact(_) => FolderState::Known,
                Base::Below(_) => FolderState::BelowThreshold,
                Base::Missing => FolderState::Missing,
            },
            before,
            change,
            driver: driver_of(&node, &base, change),
            node,
            removed: false,
        });
        Ok(())
    }
}

fn path_of(root: &str, names: &[String]) -> String {
    let mut p = PathBuf::from(root);
    p.extend(names);
    p.display().to_string()
}

/// What changed between `baseline` and `current`. Pure over the saved analyses.
pub(crate) fn attribute(
    db: &Db,
    current: Option<ScanId>,
    baseline: Option<ScanId>,
    classify: &dyn Fn(&Path) -> Option<Classification>,
) -> Result<AnomalyReport, CommandError> {
    let store = db.lock();
    let to = match current {
        Some(id) => store.scan(id)?,
        None => store.latest_scan()?,
    }
    .ok_or_else(|| CommandError::new(ErrorKind::NotFound, "There is no analysis yet."))?;
    let scans = store.scans_for_root(&to.root)?;
    let from = match baseline {
        Some(id) => store.scan(id)?.ok_or_else(|| {
            CommandError::new(ErrorKind::NotFound, "That analysis no longer exists.")
        })?,
        // The newest complete analysis before this one.
        None => scans
            .iter()
            .rev()
            .filter(|s| s.id != to.id && !partial(s))
            .find(|s| (s.finished_at_ms, s.id) < (to.finished_at_ms, to.id))
            .cloned()
            .ok_or_else(|| {
                CommandError::new(
                    ErrorKind::NotFound,
                    "There is no earlier complete analysis of this drive to compare with.",
                )
            })?,
    };
    if from.id == to.id {
        return Err(invalid("Choose two different analyses to compare."));
    }
    if !from.root.eq_ignore_ascii_case(&to.root) {
        return Err(invalid("Those analyses are of different folders."));
    }
    if partial(&from) || partial(&to) {
        return Err(invalid(
            "One of those analyses was cancelled or incomplete, so its totals are lower bounds and cannot be compared.",
        ));
    }

    let net = signed(to.stats.total_bytes, from.stats.total_bytes);
    // The folder-level threshold: absolute, and relative to the net change.
    let relative = (net.unsigned_abs() as f64 * MIN_SHARE) as u64;
    let threshold = MIN_CHANGE_BYTES.max(relative);

    let mut walk = Walk {
        store: &store,
        to: &to,
        from: &from,
        threshold,
        leaves: Vec::new(),
    };
    let root_to = store.node(to.id, 0)?.ok_or_else(|| {
        CommandError::new(ErrorKind::NotFound, "The analysis has no stored folders.")
    })?;
    let root_from = store.node(from.id, 0)?.ok_or_else(|| {
        CommandError::new(ErrorKind::NotFound, "The baseline has no stored folders.")
    })?;
    walk.explore(root_to, Base::Exact(root_from), Vec::new())?;

    let mut caveats = Vec::new();
    if from.min_node_bytes != to.min_node_bytes {
        caveats.push(
            "The two analyses stored folders down to different sizes, so small folders may be compared unevenly."
                .to_owned(),
        );
    }
    let mut grew = Vec::new();
    let mut shrank = Vec::new();
    for l in walk.leaves {
        let kind = if l.removed {
            ChangeKind::Removed
        } else if l.base_state == FolderState::Missing {
            ChangeKind::Added
        } else if l.change > 0 {
            ChangeKind::Grew
        } else {
            ChangeKind::Shrank
        };
        let path = path_of(&to.root, &l.names);
        let share =
            (net.unsigned_abs() >= 1024 * 1024).then(|| l.change as f64 / net as f64 * 100.0);
        let c = Contributor {
            classification: classify(Path::new(&path)),
            path,
            kind,
            change_bytes: l.change,
            now_bytes: l.node.total_bytes,
            before_bytes: l.before,
            baseline: l.base_state,
            share_of_net_change: share,
            driver: l.driver,
        };
        if c.change_bytes >= 0 {
            grew.push(c)
        } else {
            shrank.push(c)
        }
    }
    grew.sort_by_key(|c| std::cmp::Reverse(c.change_bytes));
    shrank.sort_by_key(|c| c.change_bytes);
    grew.truncate(MAX_CONTRIBUTORS);
    shrank.truncate(MAX_CONTRIBUTORS);
    if grew
        .iter()
        .any(|c| c.baseline == FolderState::BelowThreshold)
    {
        caveats.push("Growth marked \"at least\" is a lower bound: the folder was too small to be stored in the baseline.".to_owned());
    }
    let listed: i64 = grew.iter().chain(&shrank).map(|c| c.change_bytes).sum();

    // Large files: new or bigger, only where the baseline list makes that certain.
    let old = store.largest_files(from.id)?;
    let old_full = old.len() >= 50;
    let old_floor = old.last().map_or(0, |f| f.bytes);
    let before: HashMap<String, u64> = old
        .iter()
        .map(|f| (f.path.to_lowercase(), f.bytes))
        .collect();
    let mut large_files: Vec<FileChange> = store
        .largest_files(to.id)?
        .into_iter()
        .filter_map(|f| {
            let prev = before.get(&f.path.to_lowercase()).copied();
            let certain = match prev {
                Some(p) => f.bytes > p && f.bytes - p >= MIN_CHANGE_BYTES,
                // Not in an unfull list means it did not exist; in a full list it was at most the floor.
                None => (!old_full || f.bytes > old_floor) && f.bytes >= MIN_CHANGE_BYTES,
            };
            certain.then(|| FileChange {
                classification: classify(Path::new(&f.path)),
                path: f.path,
                bytes: f.bytes,
                before_bytes: prev,
            })
        })
        .collect();
    large_files.sort_by_key(|f| std::cmp::Reverse(f.bytes));
    large_files.truncate(MAX_CONTRIBUTORS);

    Ok(AnomalyReport {
        root: to.root.clone(),
        baseline: analysis_ref(&from),
        current: analysis_ref(&to),
        net_change_bytes: net,
        grew,
        shrank,
        unattributed_bytes: net - listed,
        large_files,
        reporting_threshold_bytes: threshold,
        caveats,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use sentinel_scanner::scan::{ScanControl, ScanOptions, scan};
    use sentinel_store::Retention;

    use super::*;

    const MB: usize = 1024 * 1024;
    const H: i64 = 3_600_000;

    fn put(path: &Path, bytes: usize) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, vec![7u8; bytes]).unwrap();
    }

    fn save_with(
        db: &Db,
        root: &Path,
        finished: i64,
        mutate: impl FnOnce(&mut sentinel_scanner::scan::ScanTree),
    ) -> ScanId {
        let mut tree = scan(root, &ScanOptions::default(), &ScanControl::new()).unwrap();
        mutate(&mut tree);
        db.lock()
            .save_scan(&tree, &[], finished - 5, finished, Retention::default())
            .unwrap()
    }

    fn save(db: &Db, root: &Path, finished: i64) -> ScanId {
        save_with(db, root, finished, |_| {})
    }

    fn none(_: &Path) -> Option<Classification> {
        None
    }

    fn report(
        db: &Db,
        current: Option<ScanId>,
        baseline: Option<ScanId>,
    ) -> Result<AnomalyReport, CommandError> {
        attribute(db, current, baseline, &none)
    }

    #[test]
    fn finds_the_folder_responsible_and_ignores_noise() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        put(&r.join("deep").join("inner").join("data.bin"), 2 * MB);
        put(&r.join("noise").join("n.bin"), 2 * MB);
        put(&r.join("stable").join("s.bin"), 3 * MB);
        let db = Db::in_memory();
        save(&db, r, 100 * H);

        put(&r.join("deep").join("inner").join("extra.bin"), 40 * MB);
        put(&r.join("noise").join("n2.bin"), MB);
        save(&db, r, 200 * H);

        let rep = report(&db, None, None).unwrap();
        assert_eq!(
            rep.grew.len(),
            1,
            "{:?}",
            rep.grew.iter().map(|c| &c.path).collect::<Vec<_>>()
        );
        let c = &rep.grew[0];
        assert!(c.path.ends_with(r"deep\inner"), "{}", c.path);
        assert_eq!(c.kind, ChangeKind::Grew);
        assert_eq!(c.driver, Driver::OwnFiles);
        assert_eq!(c.baseline, FolderState::Known);
        assert!(c.change_bytes >= (40 * MB) as i64 && c.change_bytes < (41 * MB) as i64);
        assert!(c.share_of_net_change.unwrap() > 90.0);
        assert!(rep.shrank.is_empty());
        assert!(
            rep.unattributed_bytes.unsigned_abs() < MIN_CHANGE_BYTES,
            "noise stays unattributed"
        );
        assert_eq!(
            rep.net_change_bytes,
            rep.unattributed_bytes + c.change_bytes
        );
    }

    #[test]
    fn reports_added_and_removed_folders() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        put(&r.join("old").join("o.bin"), 30 * MB);
        put(&r.join("keep").join("k.bin"), 2 * MB);
        let db = Db::in_memory();
        save(&db, r, 100 * H);
        std::fs::remove_dir_all(r.join("old")).unwrap();
        put(&r.join("fresh").join("f.bin"), 25 * MB);
        save(&db, r, 200 * H);

        let rep = report(&db, None, None).unwrap();
        let added = &rep.grew[0];
        assert!(added.path.ends_with("fresh"));
        assert_eq!(
            (added.kind, added.baseline, added.before_bytes),
            (ChangeKind::Added, FolderState::Missing, Some(0))
        );
        assert_eq!(added.driver, Driver::WholeFolder);
        let gone = &rep.shrank[0];
        assert!(gone.path.ends_with("old"));
        assert_eq!((gone.kind, gone.now_bytes), (ChangeKind::Removed, 0));
        assert!(gone.change_bytes <= -((30 * MB) as i64));
    }

    #[test]
    fn growth_from_a_folder_too_small_to_have_been_stored_is_a_lower_bound() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        put(&r.join("tiny").join("t.bin"), 100 * 1024); // under the 1 MB storage threshold
        put(&r.join("big").join("b.bin"), 5 * MB);
        let db = Db::in_memory();
        save(&db, r, 100 * H);
        put(&r.join("tiny").join("huge.bin"), 30 * MB);
        save(&db, r, 200 * H);

        let rep = report(&db, None, None).unwrap();
        let c = &rep.grew[0];
        assert!(c.path.ends_with("tiny"));
        assert_eq!(c.baseline, FolderState::BelowThreshold);
        assert_eq!(c.before_bytes, None, "no exact earlier size");
        assert_eq!(c.kind, ChangeKind::Grew, "it existed, so it is not 'added'");
        // At least now minus the threshold; never more than now.
        assert!(c.change_bytes >= (29 * MB) as i64 && c.change_bytes <= c.now_bytes as i64);
        assert!(rep.caveats.iter().any(|w| w.contains("lower bound")));
    }

    #[test]
    fn growth_and_shrinkage_that_cancel_are_both_reported() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        put(&r.join("up").join("u.bin"), 2 * MB);
        put(&r.join("down").join("d.bin"), 40 * MB);
        let db = Db::in_memory();
        save(&db, r, 100 * H);
        put(&r.join("up").join("u2.bin"), 38 * MB);
        std::fs::remove_file(r.join("down").join("d.bin")).unwrap();
        put(&r.join("down").join("d.bin"), 2 * MB);
        save(&db, r, 200 * H);

        let rep = report(&db, None, None).unwrap();
        assert!(
            rep.net_change_bytes.unsigned_abs() < (3 * MB) as u64,
            "net is near zero"
        );
        assert!(rep.grew[0].path.ends_with("up"));
        assert!(rep.shrank[0].path.ends_with("down"));
        assert_eq!(rep.shrank[0].kind, ChangeKind::Shrank);
    }

    #[test]
    fn new_large_files_are_listed_only_when_the_baseline_makes_it_certain() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        put(&r.join("a").join("a.bin"), 3 * MB);
        let db = Db::in_memory();
        save(&db, r, 100 * H);
        put(&r.join("a").join("new.bin"), 40 * MB);
        put(&r.join("a").join("a.bin"), 30 * MB); // an existing file that grew
        save(&db, r, 200 * H);

        let rep = report(&db, None, None).unwrap();
        let by = |n: &str| {
            rep.large_files
                .iter()
                .find(|f| f.path.ends_with(n))
                .unwrap()
        };
        assert_eq!(by("new.bin").before_bytes, None);
        assert!(by("a.bin").before_bytes.is_some());
        assert!(by("a.bin").bytes > by("a.bin").before_bytes.unwrap());
        assert_eq!(rep.large_files[0].path, by("new.bin").path, "largest first");
    }

    #[test]
    fn default_baseline_skips_incomplete_analyses_and_bad_pairs_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        put(&r.join("x").join("x.bin"), 12 * MB);
        let db = Db::in_memory();
        let a = save(&db, r, 100 * H);
        let partial = save_with(&db, r, 150 * H, |t| t.stats.cancelled = true);
        put(&r.join("x").join("y.bin"), 30 * MB);
        let c = save(&db, r, 200 * H);

        // The default baseline is the last *complete* analysis, not the cancelled one.
        assert_eq!(report(&db, Some(c), None).unwrap().baseline.scan_id, a);
        assert_eq!(report(&db, None, None).unwrap().current.scan_id, c);
        // Explicit incomplete or identical analyses are refused.
        assert_eq!(
            report(&db, Some(c), Some(partial)).unwrap_err().kind,
            ErrorKind::InvalidInput
        );
        assert_eq!(
            report(&db, Some(c), Some(c)).unwrap_err().kind,
            ErrorKind::InvalidInput
        );
        // The oldest has nothing earlier.
        assert_eq!(
            report(&db, Some(a), None).unwrap_err().kind,
            ErrorKind::NotFound
        );
    }

    #[test]
    fn nothing_to_compare_without_analyses() {
        assert_eq!(
            report(&Db::in_memory(), None, None).unwrap_err().kind,
            ErrorKind::NotFound
        );
    }
}
