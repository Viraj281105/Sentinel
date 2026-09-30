#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use sentinel_scanner::scan::{
    DirNode, LargeFile, LinkKind, NodeId, NodeStatus, ScanStats, ScanTree,
};
use sentinel_store::{DriveSnapshot, Retention, Store, StoreError};

const MB: u64 = 1024 * 1024;

fn node(name: &str, parent: Option<NodeId>, total: u64, own: u64, status: NodeStatus) -> DirNode {
    DirNode {
        name: name.into(),
        parent,
        children: Vec::new(),
        own_bytes: own,
        total_bytes: total,
        total_logical_bytes: total,
        file_count: 1,
        dir_count: 0,
        status,
    }
}

/// C:\ (100 MB)
///   big (80 MB)
///     inner (70 MB)
///     tiny (10 KB)        <- pruned
///   small (500 KB)        <- pruned
///   link (junction)       <- kept: not Complete
fn tree(root: &str, big: u64) -> ScanTree {
    use NodeStatus::Complete;
    let mut nodes = vec![
        node(root, None, 20 * MB + big + 500 * 1024, 20 * MB, Complete),
        node("big", Some(0), big, big - 70 * MB - 10 * 1024, Complete),
        node("inner", Some(1), 70 * MB, 70 * MB, Complete),
        node("tiny", Some(1), 10 * 1024, 10 * 1024, Complete),
        node("small", Some(0), 500 * 1024, 500 * 1024, Complete),
        node(
            "link",
            Some(0),
            0,
            0,
            NodeStatus::Link {
                kind: LinkKind::Junction,
            },
        ),
    ];
    nodes[0].children = vec![1, 4, 5];
    nodes[1].children = vec![2, 3];
    ScanTree {
        root: PathBuf::from(root),
        nodes,
        largest_files: vec![
            LargeFile {
                path: format!("{root}big\\inner\\a.bin"),
                bytes: 60 * MB,
                logical_bytes: 60 * MB,
            },
            LargeFile {
                path: format!("{root}b.bin"),
                bytes: 5 * MB,
                logical_bytes: 5 * MB,
            },
        ],
        stats: ScanStats {
            dirs: 6,
            files: 6,
            total_bytes: 20 * MB + big + 500 * 1024,
            links_skipped: 1,
            ..ScanStats::default()
        },
    }
}

#[test]
fn roundtrips_scan_summary_tree_and_files() {
    let mut s = Store::open_in_memory().unwrap();
    let t = tree("C:\\", 80 * MB);
    let id = s
        .save_scan(&t, &[], 1000, 2000, Retention::default())
        .unwrap();

    let rec = s.latest_scan().unwrap().unwrap();
    assert_eq!(rec.id, id);
    assert_eq!(rec.root, "C:\\");
    assert_eq!(rec.stats, t.stats);
    assert_eq!((rec.started_at_ms, rec.finished_at_ms), (1000, 2000));

    let root = s.node(id, 0).unwrap().unwrap();
    assert_eq!(root.total_bytes, t.nodes[0].total_bytes);
    // Each fixture node has file_count 1; the root's children hold 3 of its 1.
    assert_eq!(root.own_files, 0);
    assert_eq!(s.node(id, 2).unwrap().unwrap().own_files, 1);
    assert_eq!(root.child_count, 3);

    let names: Vec<_> = s
        .children(id, 0, 100)
        .unwrap()
        .into_iter()
        .map(|n| n.name)
        .collect();
    assert_eq!(names, ["big", "link"]);
    assert_eq!(
        s.node(id, 5).unwrap().unwrap().status,
        NodeStatus::Link {
            kind: LinkKind::Junction
        }
    );
    let files = s.largest_files(id).unwrap();
    assert_eq!(files, t.largest_files);
}

#[test]
fn pruning_keeps_tree_connected_and_summarizes_dropped_folders() {
    let mut s = Store::open_in_memory().unwrap();
    let id = s
        .save_scan(&tree("C:\\", 80 * MB), &[], 0, 1, Retention::default())
        .unwrap();
    let root = s.node(id, 0).unwrap().unwrap();
    assert_eq!((root.pruned_children, root.pruned_bytes), (1, 500 * 1024));
    let big = s.node(id, 1).unwrap().unwrap();
    assert_eq!((big.pruned_children, big.pruned_bytes), (1, 10 * 1024));
    assert!(s.node(id, 3).unwrap().is_none());
    assert!(s.node(id, 4).unwrap().is_none());

    let crumbs: Vec<_> = s
        .crumbs(id, 2)
        .unwrap()
        .into_iter()
        .map(|n| n.name)
        .collect();
    assert_eq!(crumbs, ["C:\\", "big", "inner"]);
    assert_eq!(s.find_by_names(id, &["big", "inner"]).unwrap(), Some(2));
    assert_eq!(s.find_by_names(id, &["BIG", "Inner"]).unwrap(), Some(2));
    assert_eq!(s.find_by_names(id, &["big", "nope"]).unwrap(), None);
    assert_eq!(s.find_by_names(id, &[]).unwrap(), Some(0));
}

#[test]
fn children_beyond_limit_are_counted() {
    let mut s = Store::open_in_memory().unwrap();
    let id = s
        .save_scan(&tree("C:\\", 80 * MB), &[], 0, 1, Retention::default())
        .unwrap();
    assert_eq!(s.children(id, 0, 1).unwrap().len(), 1);
    assert_eq!(s.children_beyond(id, 0, 1).unwrap(), (1, 0));
    assert_eq!(s.children_beyond(id, 0, 0).unwrap(), (2, 80 * MB));
}

#[test]
fn previous_scan_is_same_root_and_earlier() {
    let mut s = Store::open_in_memory().unwrap();
    let r = Retention::default();
    let c1 = s.save_scan(&tree("C:\\", 80 * MB), &[], 0, 100, r).unwrap();
    let _d = s.save_scan(&tree("D:\\", 80 * MB), &[], 0, 150, r).unwrap();
    let c2 = s.save_scan(&tree("C:\\", 90 * MB), &[], 0, 200, r).unwrap();
    assert_eq!(s.previous_scan(c2).unwrap().unwrap().id, c1);
    assert!(s.previous_scan(c1).unwrap().is_none());
    assert_eq!(s.latest_scan().unwrap().unwrap().id, c2);
}

#[test]
fn retention_deletes_old_scans_and_their_rows() {
    let mut s = Store::open_in_memory().unwrap();
    let r = Retention {
        keep_scans_per_root: 2,
        ..Retention::default()
    };
    let first = s.save_scan(&tree("C:\\", 80 * MB), &[], 0, 1, r).unwrap();
    let d = s.save_scan(&tree("D:\\", 80 * MB), &[], 0, 2, r).unwrap();
    s.save_scan(&tree("C:\\", 80 * MB), &[], 0, 3, r).unwrap();
    s.save_scan(&tree("C:\\", 80 * MB), &[], 0, 4, r).unwrap();
    assert!(s.scan(first).unwrap().is_none());
    assert!(s.node(first, 0).unwrap().is_none());
    assert!(s.largest_files(first).unwrap().is_empty());
    assert!(s.scan(d).unwrap().is_some(), "other roots are unaffected");
}

#[test]
fn file_database_persists_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested").join("sentinel.db");
    let id = {
        let mut s = Store::open(&path).unwrap();
        s.save_scan(&tree("C:\\", 80 * MB), &[], 0, 1, Retention::default())
            .unwrap()
    };
    let s = Store::open(&path).unwrap();
    assert_eq!(s.schema_version().unwrap(), 4);
    assert_eq!(s.latest_scan().unwrap().unwrap().id, id);
}

#[test]
fn refuses_database_from_a_newer_version() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sentinel.db");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.pragma_update(None, "user_version", 99).unwrap();
    }
    assert!(matches!(
        Store::open(&path),
        Err(StoreError::FutureSchema { found: 99, .. })
    ));
}

#[test]
fn drive_snapshots_are_throttled_and_ordered() {
    let s = Store::open_in_memory().unwrap();
    let snap = |t, free| DriveSnapshot {
        taken_at_ms: t,
        total_bytes: 1000,
        free_bytes: free,
    };
    let hour = 3_600_000;
    assert!(s.record_drive_snapshot("C:\\", snap(0, 500), hour).unwrap());
    assert!(
        !s.record_drive_snapshot("c:\\", snap(10, 490), hour)
            .unwrap()
    );
    assert!(
        s.record_drive_snapshot("C:\\", snap(hour, 400), hour)
            .unwrap()
    );
    assert!(s.record_drive_snapshot("D:\\", snap(5, 900), hour).unwrap());
    let h = s.drive_history("C:\\", 0).unwrap();
    assert_eq!(
        h.iter().map(|x| x.free_bytes).collect::<Vec<_>>(),
        [500, 400]
    );
    assert_eq!(s.drive_history("C:\\", 1).unwrap().len(), 1);
}

#[test]
fn category_totals_roundtrip_and_cascade() {
    let mut s = Store::open_in_memory().unwrap();
    let r = Retention {
        keep_scans_per_root: 1,
        ..Retention::default()
    };
    let id = s
        .save_scan(
            &tree("C:\\", 80 * MB),
            &[("windows", 5), ("unknown", 9), ("packageCaches", 7)],
            0,
            1,
            r,
        )
        .unwrap();
    assert_eq!(
        s.scan_categories(id).unwrap(),
        [
            ("unknown".to_owned(), 9),
            ("packageCaches".to_owned(), 7),
            ("windows".to_owned(), 5)
        ]
    );
    let newer = s.save_scan(&tree("C:\\", 80 * MB), &[], 0, 2, r).unwrap();
    assert!(
        s.scan_categories(id).unwrap().is_empty(),
        "deleted with its scan"
    );
    assert!(
        s.scan_categories(newer).unwrap().is_empty(),
        "none recorded"
    );
}

#[test]
fn upgrades_a_version_1_database_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sentinel.db");
    let id = {
        let mut s = Store::open(&path).unwrap();
        s.save_scan(&tree("C:\\", 80 * MB), &[], 0, 1, Retention::default())
            .unwrap()
    };
    // Turn it back into a schema-1 database: drop every table added by later migrations.
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "DROP TABLE project_searches; DROP TABLE audit_log; DROP TABLE scan_categories;
             PRAGMA user_version = 1;",
        )
        .unwrap();
    }
    let s = Store::open(&path).unwrap();
    assert_eq!(s.schema_version().unwrap(), 4);
    assert_eq!(
        s.latest_scan().unwrap().unwrap().id,
        id,
        "existing data kept"
    );
    assert!(s.scan_categories(id).unwrap().is_empty());
}

#[test]
fn project_searches_are_replaced_per_root_and_removable() {
    let s = Store::open_in_memory().unwrap();
    s.save_project_search(r"D:\Projects", 1, "{\"v\":1}")
        .unwrap();
    s.save_project_search(r"d:\projects", 2, "{\"v\":2}")
        .unwrap();
    s.save_project_search(r"C:\src", 3, "{}").unwrap();
    let rows = s.project_searches().unwrap();
    assert_eq!(rows.len(), 2, "same folder regardless of case");
    let d = rows
        .iter()
        .find(|r| r.root.eq_ignore_ascii_case(r"D:\Projects"))
        .unwrap();
    assert_eq!((d.searched_at_ms, d.result_json.as_str()), (2, "{\"v\":2}"));
    assert!(s.remove_project_search(r"C:\SRC").unwrap());
    assert!(!s.remove_project_search(r"C:\src").unwrap());
    assert_eq!(s.project_searches().unwrap().len(), 1);
}
