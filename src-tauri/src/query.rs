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
    if let Some(d) = q.older_than_days {
        not_applied.push(NotApplied {
            field: "olderThanDays".into(),
            reason: format!(
                "Saved analyses record sizes, not when folders were last changed, so \"not changed for {d} days\" could not be checked. The results below are not limited by age."
            ),
        });
    }
    let mut notes = Vec::new();
    let min = q.min_bytes.unwrap_or(0);

    // (path, bytes, files, node id, parent id) for every candidate, before sorting.
    struct Row {
        path: String,
        bytes: u64,
        files: Option<u64>,
        id: Option<u32>,
        parent: Option<u32>,
        class: Option<Classification>,
    }
    let mut rows: Vec<Row> = Vec::new();
    let mut all_paths: HashMap<u32, (Option<u32>, bool)> = HashMap::new();

    match q.target {
        Target::Folders => {
            notes.push(format!(
                "Folders smaller than {} bytes were not stored in this analysis, so they cannot match.",
                rec.min_node_bytes
            ));
            let nodes = store.all_nodes(rec.id)?;
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
                let ok = n.total_bytes >= min
                    && matches_text(&shown, q.path_contains.as_deref())
                    && matches_category(class, &q.categories);
                all_paths.insert(n.id, (n.parent, ok));
                if ok {
                    rows.push(Row {
                        path: shown,
                        bytes: n.total_bytes,
                        files: Some(n.file_count),
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
            for f in store.largest_files(rec.id)? {
                let class = classify(Path::new(&f.path));
                if f.bytes >= min
                    && matches_text(&f.path, q.path_contains.as_deref())
                    && matches_category(class, &q.categories)
                {
                    rows.push(Row {
                        path: f.path,
                        bytes: f.bytes,
                        files: None,
                        id: None,
                        parent: None,
                        class,
                    });
                }
            }
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
            .save_scan(&tree, &[], 1, 2, Retention::default())
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

    #[test]
    fn age_cannot_be_evaluated_so_it_is_reported_not_silently_dropped() {
        let (_d, db, id, c) = saved();
        let with = go(
            &db,
            &c,
            Some(id),
            r#"{"target":"folders","olderThanDays":90}"#,
        )
        .unwrap();
        let without = go(&db, &c, Some(id), r#"{"target":"folders"}"#).unwrap();
        assert_eq!(with.not_applied.len(), 1);
        assert_eq!(with.not_applied[0].field, "olderThanDays");
        assert!(with.not_applied[0].reason.contains("not limited by age"));
        assert_eq!(
            names(&with),
            names(&without),
            "the other filters still apply"
        );
        assert!(with.description.contains("not changed for 90 days or more"));
        assert!(without.not_applied.is_empty());
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
