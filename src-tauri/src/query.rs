//! Running a validated [`StructuredQuery`] against a saved analysis. Read-only and
//! deterministic: no model is involved here, so the same query on the same analysis always
//! gives the same answer, whoever wrote the query.
//!
//! What is evaluated, and what cannot be:
//! - Folder queries cover the folders the analysis stored (those at or above its folder
//!   threshold). File queries cover only the analysis's saved largest files.
//! - A saved analysis records sizes, not modification times, so "not changed for N days"
//!   **cannot be evaluated**. That filter is reported as not applied, ahead of the results,
//!   rather than silently ignored.
//! - Matches can nest (a folder and its subfolders); the unique total counts each byte once.
//!
//! The query text is re-validated here ([`run_json`]) so that every caller, an AI answer or
//! a person's form, passes through the same single choke point.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use sentinel_ai::{QueryError, QueryVocabulary, Sort, StructuredQuery, Target, parse_query};
use sentinel_classify::{Category, Classification};
use sentinel_scanner::scan::NodeStatus;
use sentinel_store::ScanId;
use serde::Serialize;
use ts_rs::TS;

use crate::commands::error::{CommandError, ErrorKind};
use crate::db::Db;

const DAY_MS: i64 = 86_400_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ResultTarget {
    Folders,
    Files,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct QueryItem {
    pub path: String,
    #[ts(type = "number")]
    pub bytes: u64,
    /// Files in the folder (folders only).
    #[ts(type = "number | null")]
    pub files: Option<u64>,
    /// When anything in it last changed (a folder's newest entry, a file's own time), as
    /// of the analysis; `null` if not recorded.
    #[ts(type = "number | null")]
    pub last_changed_ms: Option<i64>,
    pub classification: Option<Classification>,
}

/// A part of the question the saved data cannot answer, so it was left out.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct NotApplied {
    pub field: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct QueryResult {
    #[ts(type = "number")]
    pub scan_id: ScanId,
    pub root: String,
    #[ts(type = "number")]
    pub finished_at_ms: i64,
    pub target: ResultTarget,
    /// The query in plain words.
    pub description: String,
    /// How many items matched, before the limit.
    #[ts(type = "number")]
    pub matches: u64,
    pub items: Vec<QueryItem>,
    /// Bytes in all matches, counting nested folders once.
    #[ts(type = "number")]
    pub unique_bytes: u64,
    pub more_than_shown: bool,
    /// Parts of the question that could not be evaluated. Show these before the results.
    pub not_applied: Vec<NotApplied>,
    /// What the data does and does not cover.
    pub notes: Vec<String>,
}

/// The category names a query may use: exactly the ones Sentinel classifies into.
pub(crate) fn vocabulary() -> QueryVocabulary {
    QueryVocabulary {
        categories: Category::ALL.iter().map(|c| c.key().to_owned()).collect(),
    }
}

fn invalid(e: &QueryError) -> CommandError {
    CommandError::new(
        ErrorKind::InvalidInput,
        format!("That is not a valid query: {e}."),
    )
}

/// Validate `query_json` and run it. This is the entry point for every caller.
pub(crate) fn run_json(
    db: &Db,
    classify: &dyn Fn(&Path) -> Option<Classification>,
    scan_id: Option<ScanId>,
    query_json: &str,
) -> Result<QueryResult, CommandError> {
    let query = parse_query(query_json, &vocabulary()).map_err(|e| invalid(&e))?;
    run(db, classify, scan_id, &query)
}

fn matches_text(path: &str, needle: Option<&str>) -> bool {
    needle.is_none_or(|n| path.to_lowercase().contains(&n.to_lowercase()))
}

fn matches_category(c: Option<Classification>, wanted: &[String]) -> bool {
    wanted.is_empty()
        || c.is_some_and(|c| {
            wanted
                .iter()
                .any(|w| w.eq_ignore_ascii_case(c.category.key()))
        })
}

pub(crate) fn run(
    db: &Db,
    classify: &dyn Fn(&Path) -> Option<Classification>,
    scan_id: Option<ScanId>,
    q: &StructuredQuery,
) -> Result<QueryResult, CommandError> {
    let store = db.lock();
    let rec = match scan_id {
        Some(id) => store.scan(id)?,
        None => store.latest_scan()?,
    }
    .ok_or_else(|| {
        CommandError::new(ErrorKind::NotFound, "There is no saved analysis to query.")
    })?;

    let mut not_applied = Vec::new();
    let mut notes = Vec::new();
    let min = q.min_bytes.unwrap_or(0);
    let age_ms = q.older_than_days.map(|d| i64::from(d) * DAY_MS);
    // Ages are measured as of when the analysis finished, which makes the answer
    // deterministic (and, if anything, conservative: a later change is not known).
    let reference = rec.finished_at_ms;
    let mut unknown_age = 0u64;

    struct Row {
        path: String,
        bytes: u64,
        files: Option<u64>,
        changed: Option<i64>,
        id: Option<u32>,
        parent: Option<u32>,
        class: Option<Classification>,
    }
    let mut rows: Vec<Row> = Vec::new();
    let mut all_paths: HashMap<u32, (Option<u32>, bool)> = HashMap::new();

    // The age filter needs recorded change times. Analyses saved before Sentinel recorded
    // them have none, and then the filter cannot be applied at all.
    let age_unavailable = |any_known: bool, what: &str, not_applied: &mut Vec<NotApplied>| {
        if let Some(d) = q.older_than_days
            && !any_known
        {
            not_applied.push(NotApplied {
                field: "olderThanDays".into(),
                reason: format!(
                    "This analysis has no record of when {what} last changed (it was saved before Sentinel kept that), so \"not changed for {d} days\" could not be checked. The results are not limited by age. Analyze the drive again to enable it."
                ),
            });
        }
    };

    match q.target {
        Target::Folders => {
            notes.push(format!(
                "Folders smaller than {} bytes were not stored in this analysis, so they cannot match.",
                rec.min_node_bytes
            ));
            let nodes = store.all_nodes(rec.id)?;
            let any_known = nodes.iter().any(|n| n.newest_ms.is_some());
            age_unavailable(any_known, "folders", &mut not_applied);
            let apply_age = age_ms.filter(|_| any_known);
            // Parents always have smaller ids than their children, so one pass builds paths.
            let mut paths: HashMap<u32, PathBuf> = HashMap::new();
            for n in &nodes {
                let path = match n.parent.and_then(|p| paths.get(&p)) {
                    Some(parent) => parent.join(&n.name),
                    None => PathBuf::from(&rec.root),
                };
                paths.insert(n.id, path.clone());
                if n.id == 0 || !matches!(n.status, NodeStatus::Complete) {
                    continue; // the root itself, links and unreadable folders
                }
                let shown = path.display().to_string();
                let class = classify(&path);
                let mut ok = n.total_bytes >= min
                    && matches_text(&shown, q.path_contains.as_deref())
                    && matches_category(class, &q.categories);
                if ok && let Some(age) = apply_age {
                    match n.newest_ms {
                        Some(t) => ok = reference - t >= age,
                        None => {
                            // An empty folder has nothing to date; it is not claimed stale.
                            unknown_age += 1;
                            ok = false;
                        }
                    }
                }
                all_paths.insert(n.id, (n.parent, ok));
                if ok {
                    rows.push(Row {
                        path: shown,
                        bytes: n.total_bytes,
                        files: Some(n.file_count),
                        changed: n.newest_ms,
                        id: Some(n.id),
                        parent: n.parent,
                        class,
                    });
                }
            }
            // Ancestors that are not Complete still sit on the parent chain.
            for n in &nodes {
                all_paths.entry(n.id).or_insert((n.parent, false));
            }
        }
        Target::Files => {
            notes.push(
                "Only the largest files of each analysis are saved, so this searches those, not every file."
                    .to_owned(),
            );
            let files = store.largest_files(rec.id)?;
            let any_known = files.iter().any(|f| f.modified_ms.is_some());
            age_unavailable(any_known, "files", &mut not_applied);
            let apply_age = age_ms.filter(|_| any_known);
            for f in files {
                let class = classify(Path::new(&f.path));
                let mut ok = f.bytes >= min
                    && matches_text(&f.path, q.path_contains.as_deref())
                    && matches_category(class, &q.categories);
                if ok && let Some(age) = apply_age {
                    match f.modified_ms {
                        Some(t) => ok = reference - t >= age,
                        None => {
                            unknown_age += 1;
                            ok = false;
                        }
                    }
                }
                if ok {
                    rows.push(Row {
                        path: f.path,
                        bytes: f.bytes,
                        files: None,
                        changed: f.modified_ms,
                        id: None,
                        parent: None,
                        class,
                    });
                }
            }
        }
    }
    if q.older_than_days.is_some() && not_applied.is_empty() {
        notes.push(
            "Ages are measured as of when this analysis finished, not today: a change made since then is not known."
                .to_owned(),
        );
        if unknown_age > 0 {
            notes.push(format!(
                "{unknown_age} item(s) had no recorded change time (for example empty folders) and were left out of the age filter."
            ));
        }
    }

    // Count each byte once when matches nest: skip folders that have a matched ancestor.
    let matched: HashSet<u32> = rows.iter().filter_map(|r| r.id).collect();
    let has_matched_ancestor = |start: Option<u32>| -> bool {
        let mut cur = start;
        let mut hops = 0;
        while let Some(p) = cur {
            if matched.contains(&p) {
                return true;
            }
            cur = all_paths.get(&p).and_then(|x| x.0);
            hops += 1;
            if hops > 4096 {
                break;
            }
        }
        false
    };
    let unique_bytes: u64 = rows
        .iter()
        .filter(|r| r.id.is_none() || !has_matched_ancestor(r.parent))
        .map(|r| r.bytes)
        .sum();

    let total = rows.len() as u64;
    match q.sort {
        Sort::SizeDesc => {
            rows.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.path.cmp(&b.path)))
        }
        Sort::SizeAsc => {
            rows.sort_by(|a, b| a.bytes.cmp(&b.bytes).then_with(|| a.path.cmp(&b.path)))
        }
        Sort::Name => rows.sort_by_key(|r| r.path.to_lowercase()),
    }
    let limit = q.limit as usize;
    let more = rows.len() > limit;
    rows.truncate(limit);

    Ok(QueryResult {
        scan_id: rec.id,
        root: rec.root,
        finished_at_ms: rec.finished_at_ms,
        target: match q.target {
            Target::Folders => ResultTarget::Folders,
            Target::Files => ResultTarget::Files,
        },
        description: q.describe(),
        matches: total,
        items: rows
            .into_iter()
            .map(|r| QueryItem {
                path: r.path,
                bytes: r.bytes,
                files: r.files,
                last_changed_ms: r.changed,
                classification: r.class,
            })
            .collect(),
        unique_bytes,
        more_than_shown: more,
        not_applied,
        notes,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::collections::HashMap;

    use sentinel_classify::Classifier;
    use sentinel_scanner::scan::{ScanControl, ScanOptions, scan};
    use sentinel_store::Retention;

    use super::*;

    const MB: usize = 1024 * 1024;

    fn put(path: &Path, bytes: usize) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, vec![3u8; bytes]).unwrap();
    }

    /// web/node_modules/pkg (5 MB), web/src (2 MB), docs (3 MB), tmp (2 MB), tiny (10 KB).
    fn saved() -> (tempfile::TempDir, Db, ScanId, Classifier) {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        put(
            &r.join("web").join("node_modules").join("pkg").join("f"),
            5 * MB,
        );
        put(&r.join("web").join("src").join("a.js"), 2 * MB);
        put(&r.join("docs").join("big.bin"), 3 * MB);
        put(&r.join("tmp").join("x.tmp"), 2 * MB);
        put(&r.join("tiny").join("f"), 10 * 1024);
        let db = Db::in_memory();
        let tree = scan(r, &ScanOptions::default(), &ScanControl::new()).unwrap();
        let id = db
            .lock()
            .save_scan(
                &tree,
                &[],
                crate::db::now_ms() - 5,
                crate::db::now_ms(),
                Retention::default(),
            )
            .unwrap();
        (dir, db, id, Classifier::new(&HashMap::new()))
    }

    fn go(
        db: &Db,
        c: &Classifier,
        id: Option<ScanId>,
        json: &str,
    ) -> Result<QueryResult, CommandError> {
        run_json(db, &|p| c.classify(p), id, json)
    }

    fn names(r: &QueryResult) -> Vec<String> {
        r.items
            .iter()
            .map(|i| i.path.rsplit('\\').next().unwrap().to_owned())
            .collect()
    }

    #[test]
    fn filters_folders_by_category_and_counts_nested_matches_once() {
        let (_d, db, id, c) = saved();
        let r = go(
            &db,
            &c,
            Some(id),
            r#"{"target":"folders","categories":["developerDependencies"]}"#,
        )
        .unwrap();
        // node_modules and its subfolder pkg both match; bytes are counted once.
        assert_eq!(r.matches, 2, "{:?}", names(&r));
        assert_eq!(names(&r), ["node_modules", "pkg"]);
        let top = r.items[0].bytes;
        assert_eq!(r.unique_bytes, top, "the nested folder adds nothing");
        assert!(
            r.items
                .iter()
                .all(|i| i.classification.unwrap().category == Category::DeveloperDependencies)
        );
        assert!(r.not_applied.is_empty());
        assert!(
            r.description
                .starts_with("Folders, in the categories developerDependencies")
        );
    }

    #[test]
    fn size_text_and_sort_filters_work_and_are_case_insensitive() {
        let (_d, db, id, c) = saved();
        let big = go(
            &db,
            &c,
            Some(id),
            r#"{"target":"folders","minBytes":4194304}"#,
        )
        .unwrap();
        assert_eq!(
            names(&big),
            ["web", "node_modules", "pkg"],
            "web 7 MB, node_modules and pkg 5 MB"
        );
        assert_eq!(big.items[0].files, Some(2));

        let docs = go(
            &db,
            &c,
            Some(id),
            r#"{"target":"folders","pathContains":"DOCS"}"#,
        )
        .unwrap();
        assert_eq!(names(&docs), ["docs"]);

        let asc = go(
            &db,
            &c,
            Some(id),
            r#"{"target":"folders","sort":"sizeAsc","limit":200}"#,
        )
        .unwrap();
        assert!(asc.items.windows(2).all(|w| w[0].bytes <= w[1].bytes));
        assert!(
            !names(&asc).contains(&"tiny".to_owned()),
            "folders under the threshold were never stored"
        );
        assert!(asc.notes.iter().any(|n| n.contains("were not stored")));
    }

    #[test]
    fn the_limit_trims_the_list_but_not_the_match_count() {
        let (_d, db, id, c) = saved();
        let r = go(
            &db,
            &c,
            Some(id),
            r#"{"target":"folders","sort":"name","limit":2}"#,
        )
        .unwrap();
        assert_eq!(r.items.len(), 2);
        assert!(r.more_than_shown);
        assert!(r.matches > 2);
        let all = go(
            &db,
            &c,
            Some(id),
            r#"{"target":"folders","sort":"name","limit":200}"#,
        )
        .unwrap();
        assert!(!all.more_than_shown);
        assert_eq!(all.matches, r.matches);
        // Name order is case-insensitive and deterministic.
        let again = go(
            &db,
            &c,
            Some(id),
            r#"{"target":"folders","sort":"name","limit":2}"#,
        )
        .unwrap();
        assert_eq!(names(&r), names(&again));
    }

    #[test]
    fn file_queries_cover_the_saved_largest_files_and_say_so() {
        let (_d, db, id, c) = saved();
        let r = go(
            &db,
            &c,
            Some(id),
            r#"{"target":"files","minBytes":3000000}"#,
        )
        .unwrap();
        assert_eq!(names(&r), ["f", "big.bin"], "5 MB and 3 MB files");
        assert!(r.items.iter().all(|i| i.files.is_none()));
        assert!(r.notes.iter().any(|n| n.contains("largest files")));
        let tmp = go(
            &db,
            &c,
            Some(id),
            r#"{"target":"files","categories":["developerDependencies"]}"#,
        )
        .unwrap();
        assert_eq!(tmp.matches, 1);
        assert_eq!(r.target, ResultTarget::Files);
    }

    use std::fs::OpenOptions;
    use std::os::windows::fs::OpenOptionsExt;
    use std::time::{Duration, SystemTime};

    /// Set a file's or folder's modification time `days` into the past.
    fn age(path: &Path, days: u64) {
        OpenOptions::new()
            .access_mode(0x0100) // FILE_WRITE_ATTRIBUTES
            .custom_flags(0x0200_0000) // BACKUP_SEMANTICS: folders can be opened too
            .open(path)
            .unwrap()
            .set_modified(SystemTime::now() - Duration::from_secs(days * 86_400))
            .unwrap();
    }

    /// stale/{old.bin} aged 300 days, fresh/{new.bin}, mixed/{old2.bin aged, new2.bin}.
    fn aged() -> (tempfile::TempDir, Db, ScanId, Classifier) {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        put(&r.join("stale").join("old.bin"), 2 * MB);
        put(&r.join("fresh").join("new.bin"), 2 * MB);
        put(&r.join("mixed").join("old2.bin"), 2 * MB);
        put(&r.join("mixed").join("new2.bin"), 2 * MB);
        // The file first, then the folder that holds it: folders carry their own times.
        age(&r.join("stale").join("old.bin"), 300);
        age(&r.join("stale"), 300);
        age(&r.join("mixed").join("old2.bin"), 300);
        let db = Db::in_memory();
        let tree = scan(r, &ScanOptions::default(), &ScanControl::new()).unwrap();
        let id = db
            .lock()
            .save_scan(
                &tree,
                &[],
                crate::db::now_ms() - 5,
                crate::db::now_ms(),
                Retention::default(),
            )
            .unwrap();
        (dir, db, id, Classifier::new(&HashMap::new()))
    }

    #[test]
    fn older_than_uses_the_recorded_change_times() {
        let (_d, db, id, c) = aged();
        let r = go(
            &db,
            &c,
            Some(id),
            r#"{"target":"folders","olderThanDays":90}"#,
        )
        .unwrap();
        assert!(r.not_applied.is_empty(), "{:?}", r.not_applied);
        // Only `stale` is entirely old; `mixed` holds a recent file, `fresh` is recent.
        assert_eq!(names(&r), ["stale"]);
        let age_days = (r.finished_at_ms - r.items[0].last_changed_ms.unwrap()) / 86_400_000;
        assert!((299..=301).contains(&age_days), "{age_days}");
        assert!(
            r.notes
                .iter()
                .any(|n| n.contains("as of when this analysis finished"))
        );
        assert!(r.description.contains("not changed for 90 days or more"));

        // A threshold beyond every age matches nothing; one inside matches the same folder.
        assert!(
            go(
                &db,
                &c,
                Some(id),
                r#"{"target":"folders","olderThanDays":400}"#
            )
            .unwrap()
            .items
            .is_empty()
        );
        let inside = go(
            &db,
            &c,
            Some(id),
            r#"{"target":"folders","olderThanDays":299}"#,
        )
        .unwrap();
        assert_eq!(names(&inside), ["stale"]);
        // Without the age filter all three folders are there.
        assert_eq!(
            go(&db, &c, Some(id), r#"{"target":"folders"}"#)
                .unwrap()
                .matches,
            3
        );
    }

    #[test]
    fn older_than_works_for_the_saved_largest_files_too() {
        let (_d, db, id, c) = aged();
        let r = go(
            &db,
            &c,
            Some(id),
            r#"{"target":"files","olderThanDays":90}"#,
        )
        .unwrap();
        assert_eq!(names(&r).len(), 2, "the two aged files: {:?}", names(&r));
        assert!(names(&r).iter().all(|n| n == "old.bin" || n == "old2.bin"));
        assert!(r.items.iter().all(|i| i.last_changed_ms.is_some()));
        assert!(r.not_applied.is_empty());
    }

    #[test]
    fn an_analysis_saved_before_change_times_existed_cannot_apply_the_filter_and_says_so() {
        let dir = tempfile::tempdir().unwrap();
        put(&dir.path().join("a").join("f"), 2 * MB);
        let db = Db::in_memory();
        let mut tree = scan(dir.path(), &ScanOptions::default(), &ScanControl::new()).unwrap();
        for n in &mut tree.nodes {
            n.newest_ms = None; // as if saved before the migration
        }
        for f in &mut tree.largest_files {
            f.modified_ms = None;
        }
        let id = db
            .lock()
            .save_scan(&tree, &[], 1, crate::db::now_ms(), Retention::default())
            .unwrap();
        let c = Classifier::new(&HashMap::new());
        for target in ["folders", "files"] {
            let q = format!(r#"{{"target":"{target}","olderThanDays":30}}"#);
            let r = go(&db, &c, Some(id), &q).unwrap();
            assert_eq!(r.not_applied.len(), 1, "{target}");
            assert!(r.not_applied[0].reason.contains("Analyze the drive again"));
            assert!(r.not_applied[0].reason.contains("not limited by age"));
            assert_eq!(
                r.items.len(),
                1,
                "everything is returned, flagged, not hidden"
            );
        }
    }

    #[test]
    fn folders_with_no_recorded_time_are_left_out_not_called_stale() {
        let dir = tempfile::tempdir().unwrap();
        put(&dir.path().join("a").join("f"), 2 * MB);
        put(&dir.path().join("b").join("f"), 2 * MB);
        age(&dir.path().join("a").join("f"), 200);
        age(&dir.path().join("a"), 200);
        let db = Db::in_memory();
        let mut tree = scan(dir.path(), &ScanOptions::default(), &ScanControl::new()).unwrap();
        // One folder lacks a time, the other has one.
        let b = tree.nodes.iter().position(|n| n.name == "b").unwrap();
        tree.nodes[b].newest_ms = None;
        let id = db
            .lock()
            .save_scan(&tree, &[], 1, crate::db::now_ms(), Retention::default())
            .unwrap();
        let c = Classifier::new(&HashMap::new());
        let r = go(
            &db,
            &c,
            Some(id),
            r#"{"target":"folders","olderThanDays":90}"#,
        )
        .unwrap();
        assert_eq!(names(&r), ["a"]);
        assert!(r.not_applied.is_empty());
        assert!(
            r.notes
                .iter()
                .any(|n| n.contains("no recorded change time")),
            "{:?}",
            r.notes
        );
    }

    #[test]
    fn every_caller_goes_through_the_same_validation() {
        let (_d, db, id, c) = saved();
        for bad in [
            "not json",
            r#"{"target":"folders","delete":true}"#,
            r#"{"target":"folders","categories":["bitcoin"]}"#,
            r#"{"target":"folders","limit":0}"#,
            "Sure! {\"target\":\"folders\"}",
        ] {
            let e = go(&db, &c, Some(id), bad).unwrap_err();
            assert_eq!(e.kind, ErrorKind::InvalidInput, "{bad}");
        }
    }

    #[test]
    fn defaults_to_the_latest_analysis_and_reports_a_missing_one() {
        let (_d, db, id, c) = saved();
        assert_eq!(
            go(&db, &c, None, r#"{"target":"folders"}"#)
                .unwrap()
                .scan_id,
            id
        );
        assert_eq!(
            go(&db, &c, Some(999), r#"{"target":"folders"}"#)
                .unwrap_err()
                .kind,
            ErrorKind::NotFound
        );
        let empty = Db::in_memory();
        assert_eq!(
            go(&empty, &c, None, r#"{"target":"folders"}"#)
                .unwrap_err()
                .kind,
            ErrorKind::NotFound
        );
    }

    #[test]
    fn the_vocabulary_is_exactly_the_classifier_categories() {
        let v = vocabulary();
        assert_eq!(v.categories.len(), Category::ALL.len());
        assert!(v.categories.contains(&"developerDependencies".to_owned()));
        for k in &v.categories {
            assert!(Category::from_key(k).is_some(), "{k}");
        }
    }
}
