//! Growth history: how a drive or one folder changed across the saved analyses of a root.
//!
//! What is and is not concluded:
//! - Only the saved analyses exist (thinned as they age: see `Retention`), so history is
//!   as dense and as long as the user's own analyses; its real span is reported, never
//!   implied.
//! - Cancelled or truncated analyses are shown but left out of every trend, since their
//!   totals are lower bounds.
//! - A folder that was too small to be stored individually has no exact size; the history
//!   says "below the threshold" rather than inventing a number.
//! - A rate per day is only given when the span is long enough to mean something, and no
//!   forecast is made from it.

use std::collections::HashMap;

use sentinel_classify::Category;
use sentinel_store::{FolderSize, Retention, ScanId};
use serde::Serialize;
use ts_rs::TS;

use crate::commands::error::{CommandError, ErrorKind};
use crate::db::Db;

/// Below this span a per-day rate says more about timing than about growth.
const MIN_RATE_SPAN_MS: i64 = 6 * 60 * 60 * 1000;
const DAY_MS: f64 = 86_400_000.0;

#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GrowthSummary {
    #[ts(type = "number")]
    pub from_ms: i64,
    #[ts(type = "number")]
    pub to_ms: i64,
    #[ts(type = "number")]
    pub from_bytes: u64,
    #[ts(type = "number")]
    pub to_bytes: u64,
    /// Negative when it shrank.
    #[ts(type = "number")]
    pub change_bytes: i64,
    /// Average change per day over the span; `null` when the span is under six hours.
    pub bytes_per_day: Option<f64>,
    /// Complete analyses the summary is based on.
    #[ts(type = "number")]
    pub analyses: u32,
}

/// First-to-last change over `(time, bytes)` points of complete analyses, oldest first.
pub(crate) fn summarize(points: &[(i64, u64)]) -> Option<GrowthSummary> {
    let (&(from_ms, from_bytes), &(to_ms, to_bytes)) = (points.first()?, points.last()?);
    if points.len() < 2 || to_ms <= from_ms {
        return None;
    }
    let change = i128::from(to_bytes) - i128::from(from_bytes);
    let span = to_ms - from_ms;
    Some(GrowthSummary {
        from_ms,
        to_ms,
        from_bytes,
        to_bytes,
        change_bytes: i64::try_from(change).unwrap_or(if change < 0 { i64::MIN } else { i64::MAX }),
        bytes_per_day: (span >= MIN_RATE_SPAN_MS).then(|| change as f64 / (span as f64 / DAY_MS)),
        analyses: points.len() as u32,
    })
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GrowthPoint {
    #[ts(type = "number")]
    pub scan_id: ScanId,
    #[ts(type = "number")]
    pub finished_at_ms: i64,
    #[ts(type = "number")]
    pub total_bytes: u64,
    #[ts(type = "number")]
    pub files: u64,
    /// Cancelled or truncated: shown, but not part of any trend.
    pub partial: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CategoryChange {
    pub category: Category,
    #[ts(type = "number")]
    pub first_bytes: u64,
    #[ts(type = "number")]
    pub last_bytes: u64,
    #[ts(type = "number")]
    pub change_bytes: i64,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GrowthHistory {
    pub root: String,
    pub points: Vec<GrowthPoint>,
    pub summary: Option<GrowthSummary>,
    /// Change per category between the first and last complete analysis, largest change
    /// first. Empty when either analysis predates classification.
    pub category_changes: Vec<CategoryChange>,
    /// The most analyses of one root that are kept. Older ones are thinned (one per day,
    /// then one per week), so the history is sparser the further back it goes.
    pub kept_analyses: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum FolderState {
    Known,
    /// Smaller than `maxBytes`; too small to have been stored individually.
    BelowThreshold,
    /// The analysis shows no such folder.
    Missing,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FolderHistoryPoint {
    #[ts(type = "number")]
    pub scan_id: ScanId,
    #[ts(type = "number")]
    pub finished_at_ms: i64,
    pub partial: bool,
    pub state: FolderState,
    #[ts(type = "number | null")]
    pub bytes: Option<u64>,
    #[ts(type = "number | null")]
    pub max_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FolderHistory {
    pub path: String,
    pub points: Vec<FolderHistoryPoint>,
    /// Based only on complete analyses that stored the folder.
    pub summary: Option<GrowthSummary>,
}

fn not_found(what: &str) -> CommandError {
    CommandError::new(ErrorKind::NotFound, what)
}

/// History of the root of `scan_id` (or the latest analysis when `None`).
pub(crate) fn growth_history(
    db: &Db,
    scan_id: Option<ScanId>,
) -> Result<GrowthHistory, CommandError> {
    let store = db.lock();
    let anchor = match scan_id {
        Some(id) => store.scan(id)?,
        None => store.latest_scan()?,
    }
    .ok_or_else(|| not_found("There is no analysis yet, so there is no history."))?;
    let scans = store.scans_for_root(&anchor.root)?;

    let points: Vec<GrowthPoint> = scans
        .iter()
        .map(|s| GrowthPoint {
            scan_id: s.id,
            finished_at_ms: s.finished_at_ms,
            total_bytes: s.stats.total_bytes,
            files: s.stats.files,
            partial: s.stats.cancelled || s.stats.truncated,
        })
        .collect();
    let complete: Vec<&GrowthPoint> = points.iter().filter(|p| !p.partial).collect();
    let summary = summarize(
        &complete
            .iter()
            .map(|p| (p.finished_at_ms, p.total_bytes))
            .collect::<Vec<_>>(),
    );

    let mut category_changes = Vec::new();
    if let (Some(first), Some(last)) = (complete.first(), complete.last())
        && first.scan_id != last.scan_id
    {
        let read = |id: ScanId| -> Result<HashMap<Category, u64>, CommandError> {
            Ok(store
                .scan_categories(id)?
                .into_iter()
                .filter_map(|(k, b)| Category::from_key(&k).map(|c| (c, b)))
                .collect())
        };
        let (a, b) = (read(first.scan_id)?, read(last.scan_id)?);
        if !a.is_empty() && !b.is_empty() {
            let mut cats: Vec<Category> = a.keys().chain(b.keys()).copied().collect();
            cats.sort_by_key(|c| c.key());
            cats.dedup();
            category_changes = cats
                .into_iter()
                .map(|category| {
                    let (fb, lb) = (
                        a.get(&category).copied().unwrap_or(0),
                        b.get(&category).copied().unwrap_or(0),
                    );
                    CategoryChange {
                        category,
                        first_bytes: fb,
                        last_bytes: lb,
                        change_bytes: i64::try_from(i128::from(lb) - i128::from(fb)).unwrap_or(0),
                    }
                })
                .filter(|c| c.change_bytes != 0)
                .collect();
            category_changes.sort_by_key(|c| std::cmp::Reverse(c.change_bytes.unsigned_abs()));
        }
    }
    Ok(GrowthHistory {
        root: anchor.root,
        points,
        summary,
        category_changes,
        kept_analyses: Retention::default().keep_scans_per_root,
    })
}

/// History of one folder of a saved analysis, by path, across all analyses of its root.
pub(crate) fn folder_history(
    db: &Db,
    scan_id: ScanId,
    node: u32,
) -> Result<FolderHistory, CommandError> {
    let store = db.lock();
    let rec = store
        .scan(scan_id)?
        .ok_or_else(|| not_found("That analysis no longer exists."))?;
    let crumbs = store.crumbs(scan_id, node)?;
    if crumbs.is_empty() {
        return Err(not_found("That folder is not part of a saved analysis."));
    }
    let names: Vec<&str> = crumbs.iter().skip(1).map(|c| c.name.as_str()).collect();
    let mut path = std::path::PathBuf::from(&rec.root);
    path.extend(&names);

    let raw = store.folder_history(&rec.root, &names)?;
    let points: Vec<FolderHistoryPoint> = raw
        .iter()
        .map(|p| {
            let (state, bytes, max_bytes) = match p.size {
                FolderSize::Known(b) => (FolderState::Known, Some(b), None),
                FolderSize::BelowThreshold { max_bytes } => {
                    (FolderState::BelowThreshold, None, Some(max_bytes))
                }
                FolderSize::Missing => (FolderState::Missing, None, None),
            };
            FolderHistoryPoint {
                scan_id: p.scan_id,
                finished_at_ms: p.finished_at_ms,
                partial: p.partial,
                state,
                bytes,
                max_bytes,
            }
        })
        .collect();
    let known: Vec<(i64, u64)> = points
        .iter()
        .filter(|p| !p.partial)
        .filter_map(|p| p.bytes.map(|b| (p.finished_at_ms, b)))
        .collect();
    Ok(FolderHistory {
        path: path.display().to_string(),
        summary: summarize(&known),
        points,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use sentinel_scanner::scan::{ScanControl, ScanOptions, scan};
    use sentinel_store::Retention;

    use super::*;

    const H: i64 = 3_600_000;
    const MB: usize = 1024 * 1024;

    #[test]
    fn summarizes_first_to_last_and_withholds_a_rate_over_short_spans() {
        assert_eq!(summarize(&[]), None);
        assert_eq!(summarize(&[(0, 5)]), None, "one point is not a trend");
        assert_eq!(
            summarize(&[(10, 5), (10, 9)]),
            None,
            "no time between points"
        );

        let s = summarize(&[(0, 100), (2 * H, 400)]).unwrap();
        assert_eq!((s.change_bytes, s.analyses), (300, 2));
        assert_eq!(s.bytes_per_day, None, "two hours is too short for a rate");

        let s = summarize(&[(0, 1000), (24 * H, 3000), (48 * H, 2000)]).unwrap();
        assert_eq!(s.change_bytes, 1000, "first to last, not the peak");
        assert!((s.bytes_per_day.unwrap() - 500.0).abs() < 1e-9);
        let shrink = summarize(&[(0, 5000), (24 * H, 1000)]).unwrap();
        assert_eq!(shrink.change_bytes, -4000);
        assert!(shrink.bytes_per_day.unwrap() < 0.0);
    }

    fn put(path: &std::path::Path, bytes: usize) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, vec![0u8; bytes]).unwrap();
    }

    fn save(db: &Db, root: &std::path::Path, finished: i64, cats: &[(&str, u64)]) -> ScanId {
        let tree = scan(root, &ScanOptions::default(), &ScanControl::new()).unwrap();
        db.lock()
            .save_scan(&tree, cats, finished - 5, finished, Retention::default())
            .unwrap()
    }

    #[test]
    fn follows_a_drive_and_a_folder_across_analyses() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        put(&r.join("big").join("f"), 3 * MB);
        put(&r.join("other").join("f"), 2 * MB);
        let db = Db::in_memory();
        let a = save(&db, r, 1_000 * H, &[("windows", 100), ("userData", 50)]);

        put(&r.join("big").join("g"), 4 * MB); // big grows
        std::fs::remove_dir_all(r.join("other")).unwrap(); // other disappears
        put(&r.join("new").join("f"), 2 * MB); // a folder appears
        let b = save(&db, r, 1_048 * H, &[("windows", 100), ("userData", 400)]);

        let g = growth_history(&db, None).unwrap();
        assert_eq!(
            g.points.iter().map(|p| p.scan_id).collect::<Vec<_>>(),
            [a, b]
        );
        assert_eq!(g.kept_analyses, 30);
        let s = g.summary.unwrap();
        assert_eq!(s.analyses, 2);
        assert!(s.change_bytes > 0);
        assert!(
            s.bytes_per_day.is_some(),
            "two days apart is enough for a rate"
        );
        // Only the changed category is listed, with its change.
        assert_eq!(g.category_changes.len(), 1);
        assert_eq!(g.category_changes[0].category, Category::UserData);
        assert_eq!(g.category_changes[0].change_bytes, 350);

        // Folder history by path, asked from either analysis.
        let top = db.lock().children(b, 0, 10).unwrap();
        let big = top.iter().find(|n| n.name == "big").unwrap();
        for from in [a, b] {
            let node = if from == b {
                big.id
            } else {
                db.lock().find_by_names(a, &["big"]).unwrap().unwrap()
            };
            let h = folder_history(&db, from, node).unwrap();
            assert!(h.path.ends_with("big"));
            assert_eq!(h.points.len(), 2);
            assert_eq!(h.points[0].state, FolderState::Known);
            assert!(h.points[1].bytes.unwrap() > h.points[0].bytes.unwrap());
            assert!(h.summary.unwrap().change_bytes > 0);
        }
        // A folder that did not exist before is "missing" then, known now.
        let new = top.iter().find(|n| n.name == "new").unwrap();
        let h = folder_history(&db, b, new.id).unwrap();
        assert_eq!(h.points[0].state, FolderState::Missing);
        assert_eq!(h.points[1].state, FolderState::Known);
        assert_eq!(h.summary, None, "one known point is not a trend");
    }

    #[test]
    fn partial_analyses_are_shown_but_never_trended() {
        let dir = tempfile::tempdir().unwrap();
        put(&dir.path().join("a").join("f"), 3 * MB);
        let db = Db::in_memory();
        save(&db, dir.path(), 1_000 * H, &[]);
        let mut tree = scan(dir.path(), &ScanOptions::default(), &ScanControl::new()).unwrap();
        tree.stats.cancelled = true;
        tree.stats.total_bytes = 1;
        db.lock()
            .save_scan(&tree, &[], 1_099 * H, 1_100 * H, Retention::default())
            .unwrap();
        let g = growth_history(&db, None).unwrap();
        assert_eq!(g.points.len(), 2);
        assert!(g.points[1].partial);
        assert_eq!(g.summary, None, "only one complete analysis, so no trend");
    }

    #[test]
    fn no_analysis_means_no_history() {
        let db = Db::in_memory();
        assert_eq!(
            growth_history(&db, None).unwrap_err().kind,
            ErrorKind::NotFound
        );
        assert_eq!(
            folder_history(&db, 9, 0).unwrap_err().kind,
            ErrorKind::NotFound
        );
    }
}
