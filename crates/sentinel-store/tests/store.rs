#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use sentinel_scanner::scan::{
    DirNode, LargeFile, LinkKind, NodeId, NodeStatus, ScanStats, ScanTree,
};
use sentinel_store::{DriveSnapshot, FolderSize, Retention, Store, StoreError};

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
        own_newest_ms: None,
        newest_ms: None,
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
                modified_ms: Some(1_700_000_000_000),
            },
            LargeFile {
                path: format!("{root}b.bin"),
                bytes: 5 * MB,
                logical_bytes: 5 * MB,
                modified_ms: None,
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
    assert_eq!(s.schema_version().unwrap(), 6);
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
            "DROP TABLE settings; DROP TABLE project_searches; DROP TABLE audit_log; DROP TABLE scan_categories;
             ALTER TABLE scan_nodes DROP COLUMN newest_ms;
             ALTER TABLE scan_largest_files DROP COLUMN modified_ms;
             PRAGMA user_version = 1;",
        )
        .unwrap();
    }
    let s = Store::open(&path).unwrap();
    assert_eq!(s.schema_version().unwrap(), 6);
    assert_eq!(
        s.latest_scan().unwrap().unwrap().id,
        id,
        "existing data kept"
    );
    assert!(s.scan_categories(id).unwrap().is_empty());
    // Rows from before change times were recorded read back as unknown, not as zero.
    let root = s.node(id, 0).unwrap().unwrap();
    assert_eq!(root.newest_ms, None);
    assert_eq!(s.largest_files(id).unwrap()[0].modified_ms, None);
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

fn save_at(store: &mut Store, tree: &ScanTree, finished: i64) -> i64 {
    store
        .save_scan(tree, &[], finished - 10, finished, Retention::default())
        .unwrap()
}

#[test]
fn folder_history_follows_a_folder_across_scans_by_path() {
    let mut store = Store::open_in_memory().unwrap();
    let a = save_at(&mut store, &tree("C:\\", 80 * MB), 1_000);
    let b = save_at(&mut store, &tree("C:\\", 120 * MB), 2_000);
    save_at(&mut store, &tree("D:\\", 90 * MB), 1_500); // another root is ignored

    let scans = store.scans_for_root("c:\\").unwrap();
    assert_eq!(
        scans.iter().map(|s| s.id).collect::<Vec<_>>(),
        [a, b],
        "oldest first, root case-insensitive"
    );

    let big = store.folder_history("C:\\", &["big"]).unwrap();
    assert_eq!(big.len(), 2);
    assert_eq!(big[0].size, FolderSize::Known(80 * MB));
    assert_eq!(big[1].size, FolderSize::Known(120 * MB));
    assert_eq!(big[0].finished_at_ms, 1_000);
    assert!(!big[0].partial);

    let inner = store.folder_history("C:\\", &["big", "inner"]).unwrap();
    assert_eq!(inner[1].size, FolderSize::Known(70 * MB));

    // The root itself is the empty path.
    let root = store.folder_history("C:\\", &[]).unwrap();
    assert_eq!(
        root[1].size,
        FolderSize::Known(20 * MB + 120 * MB + 500 * 1024)
    );
}

#[test]
fn folder_history_tells_small_from_missing() {
    let mut store = Store::open_in_memory().unwrap();
    save_at(&mut store, &tree("C:\\", 80 * MB), 1_000);
    // "tiny" exists but was below the 1 MB threshold; "ghost" never existed.
    let tiny = store.folder_history("C:\\", &["big", "tiny"]).unwrap();
    assert_eq!(tiny[0].size, FolderSize::BelowThreshold { max_bytes: MB });
    // "inner" has no pruned children, so an unknown name under it is simply missing.
    let ghost = store
        .folder_history("C:\\", &["big", "inner", "ghost"])
        .unwrap();
    assert_eq!(ghost[0].size, FolderSize::Missing);
    // Something below a pruned folder was never stored either, so it is "small", not missing.
    let under_small = store.folder_history("C:\\", &["small", "x"]).unwrap();
    assert_eq!(
        under_small[0].size,
        FolderSize::BelowThreshold { max_bytes: MB }
    );
    assert!(store.folder_history("Z:\\", &["big"]).unwrap().is_empty());
}

#[test]
fn partial_scans_are_flagged_in_the_history() {
    let mut store = Store::open_in_memory().unwrap();
    let mut t = tree("C:\\", 80 * MB);
    t.stats.cancelled = true;
    save_at(&mut store, &t, 1_000);
    assert!(store.folder_history("C:\\", &["big"]).unwrap()[0].partial);
}

mod thinning {
    use sentinel_store::{Retention, ScanMeta, scans_to_thin};

    const DAY: i64 = 86_400_000;
    const MIN: i64 = 60_000;
    /// A UTC midnight, so "days" in these tests line up with the rule's buckets.
    const BASE: i64 = 20_000 * DAY;

    fn meta(id: i64, at: i64) -> ScanMeta {
        ScanMeta {
            id,
            finished_at_ms: at,
            partial: false,
        }
    }

    fn deleted(scans: &[ScanMeta], r: &Retention) -> Vec<i64> {
        let mut d = scans_to_thin(scans, r);
        d.sort_unstable();
        d
    }

    #[test]
    fn a_burst_of_updates_cannot_use_up_the_history() {
        // An old, distinct day plus twelve analyses within an hour.
        let mut scans = vec![meta(100, BASE - 5 * DAY), meta(101, BASE - 2 * DAY)];
        scans.extend((1..=12).map(|i| meta(i, BASE + 10 * 60 * MIN + i * 5 * MIN)));
        let d = deleted(&scans, &Retention::default());
        assert_eq!(
            d,
            (1..=9).collect::<Vec<_>>(),
            "only the newest three of the burst stay"
        );
        assert!(
            !d.contains(&100) && !d.contains(&101),
            "older days are untouched"
        );
    }

    #[test]
    fn the_newest_three_are_kept_even_on_one_day() {
        let scans: Vec<_> = (1..=3).map(|i| meta(i, BASE + i * MIN)).collect();
        assert!(deleted(&scans, &Retention::default()).is_empty());
    }

    #[test]
    fn older_analyses_thin_to_one_per_day_then_one_per_week() {
        let r = Retention::default();
        let mut scans = Vec::new();
        let mut id = 0;
        // Two analyses a day for 60 days, evening one newer.
        for day in 0..60 {
            for hour in [9, 20] {
                id += 1;
                scans.push(meta(id, BASE - day * DAY + hour * 60 * MIN));
            }
        }
        let doomed = deleted(&scans, &r);
        let kept: Vec<ScanMeta> = scans
            .iter()
            .filter(|s| !doomed.contains(&s.id))
            .copied()
            .collect();
        let newest = scans.iter().map(|s| s.finished_at_ms).max().unwrap();
        let (recent_window, older): (Vec<&ScanMeta>, Vec<&ScanMeta>) = kept
            .iter()
            .partition(|s| newest - s.finished_at_ms <= 14 * DAY);
        // One per day in the last month: no two kept scans share a day.
        let mut days: Vec<i64> = recent_window
            .iter()
            .map(|s| s.finished_at_ms.div_euclid(DAY))
            .collect();
        days.sort_unstable();
        let before = days.len();
        days.dedup();
        assert!(
            before - days.len() <= 2,
            "only the always-kept newest three may share a day"
        );
        assert!(
            recent_window.len() >= 14,
            "two weeks of daily scans survive: {}",
            recent_window.len()
        );
        // One per week beyond that.
        let mut weeks: Vec<i64> = older
            .iter()
            .map(|s| s.finished_at_ms.div_euclid(7 * DAY))
            .collect();
        weeks.sort_unstable();
        let n = weeks.len();
        weeks.dedup();
        assert_eq!(n, weeks.len());
        assert!((5..=8).contains(&n), "weekly scans for the rest: {n}");
        assert!(kept.len() as u32 <= r.keep_scans_per_root);
    }

    #[test]
    fn the_cap_drops_the_oldest_and_never_the_newest() {
        let scans: Vec<_> = (0..10).map(|i| meta(i + 1, BASE - (9 - i) * DAY)).collect();
        let r = Retention {
            keep_scans_per_root: 5,
            ..Retention::default()
        };
        assert_eq!(deleted(&scans, &r), vec![1, 2, 3, 4, 5]);
        let zero = Retention {
            keep_scans_per_root: 0,
            ..Retention::default()
        };
        assert!(
            !deleted(&scans, &zero).contains(&10),
            "the newest always survives"
        );
    }

    #[test]
    fn incomplete_scans_are_dropped_when_old_and_do_not_stand_in_for_a_day() {
        let mut scans: Vec<ScanMeta> = (1..=3).map(|i| meta(i, BASE + i * MIN)).collect();
        // An old incomplete scan, and an old complete one on the same day as a recent partial.
        scans.push(ScanMeta {
            id: 10,
            finished_at_ms: BASE - 10 * DAY,
            partial: true,
        });
        scans.push(meta(11, BASE - 10 * DAY + MIN));
        let d = deleted(&scans, &Retention::default());
        assert_eq!(
            d,
            vec![10],
            "the partial one goes, the complete one on its day stays"
        );

        // A day covered only by incomplete scans does not make an older complete scan of
        // that day redundant: it is the only usable one, so it stays.
        let partial = |id: i64, at: i64| ScanMeta {
            id,
            finished_at_ms: at,
            partial: true,
        };
        let only_partial_recent = vec![
            partial(1, BASE + 5 * MIN),
            partial(2, BASE + 4 * MIN),
            partial(3, BASE + 3 * MIN),
            meta(4, BASE + 2 * MIN),
        ];
        assert!(deleted(&only_partial_recent, &Retention::default()).is_empty());

        // But a complete scan among the recent ones does stand in for that day.
        let complete_recent = vec![
            meta(1, BASE + 5 * MIN),
            partial(2, BASE + 4 * MIN),
            partial(3, BASE + 3 * MIN),
            meta(4, BASE + 2 * MIN),
        ];
        assert_eq!(deleted(&complete_recent, &Retention::default()), vec![4]);
    }

    #[test]
    fn the_input_order_does_not_matter_and_a_long_gap_is_not_thinned_away() {
        let mut scans = vec![
            meta(1, BASE - 400 * DAY),
            meta(2, BASE - 399 * DAY),
            meta(3, BASE),
        ];
        let a = deleted(&scans, &Retention::default());
        scans.reverse();
        assert_eq!(a, deleted(&scans, &Retention::default()));
        assert!(
            a.is_empty(),
            "three scans are all inside the always-kept three"
        );
        assert!(scans_to_thin(&[], &Retention::default()).is_empty());
    }
}

#[test]
fn saving_many_updates_in_a_row_keeps_the_history_that_was_already_there() {
    let mut store = Store::open_in_memory().unwrap();
    let r = Retention::default();
    let day = 86_400_000;
    let base = 20_000 * day;
    let old_a = store
        .save_scan(&tree("C:\\", 80 * MB), &[], 0, base - 7 * day, r)
        .unwrap();
    let old_b = store
        .save_scan(&tree("C:\\", 82 * MB), &[], 0, base - 3 * day, r)
        .unwrap();
    let other = store
        .save_scan(&tree("D:\\", 80 * MB), &[], 0, base - 3 * day, r)
        .unwrap();
    let mut burst = Vec::new();
    for i in 0..15 {
        burst.push(
            store
                .save_scan(&tree("C:\\", 85 * MB), &[], 0, base + i * 60_000, r)
                .unwrap(),
        );
    }
    // The week-old and three-day-old analyses survive 15 saves in an hour.
    assert!(store.scan(old_a).unwrap().is_some());
    assert!(store.scan(old_b).unwrap().is_some());
    assert!(
        store.scan(other).unwrap().is_some(),
        "other roots are untouched"
    );
    // Of the burst only the newest three remain, and the newest is the latest save.
    let alive: Vec<i64> = burst
        .iter()
        .copied()
        .filter(|id| store.scan(*id).unwrap().is_some())
        .collect();
    assert_eq!(alive, burst[12..].to_vec());
    assert_eq!(
        store.latest_scan().unwrap().unwrap().id,
        *burst.last().unwrap()
    );
    // Comparison still works: the previous scan is the one before it.
    assert_eq!(
        store.previous_scan(burst[14]).unwrap().unwrap().id,
        burst[13]
    );
    assert_eq!(store.scans_for_root("C:\\").unwrap().len(), 5);
}

#[test]
fn change_times_roundtrip_and_unknown_stays_unknown() {
    let mut store = Store::open_in_memory().unwrap();
    let mut t = tree("C:\\", 80 * MB);
    t.nodes[0].newest_ms = Some(1_700_000_500_000);
    t.nodes[1].newest_ms = Some(1_700_000_400_000); // big
    t.nodes[2].newest_ms = Some(1_600_000_000_000); // inner: a long time ago
    t.nodes[3].newest_ms = None; // tiny, pruned
    let id = store
        .save_scan(&t, &[], 0, 10, Retention::default())
        .unwrap();

    let n = |names: &[&str]| {
        let node = store.find_by_names(id, names).unwrap().unwrap();
        store.node(id, node).unwrap().unwrap()
    };
    assert_eq!(n(&[]).newest_ms, Some(1_700_000_500_000));
    assert_eq!(n(&["big"]).newest_ms, Some(1_700_000_400_000));
    assert_eq!(n(&["big", "inner"]).newest_ms, Some(1_600_000_000_000));
    // A folder with no known time is None, never 0 ("changed in 1970").
    let link = n(&["link"]);
    assert_eq!(link.newest_ms, None);

    // The saved largest files keep their times too; one had none.
    let files = store.largest_files(id).unwrap();
    assert_eq!(files[0].modified_ms, Some(1_700_000_000_000));
    assert_eq!(files[1].modified_ms, None);
    assert_eq!(
        store.all_nodes(id).unwrap().len(),
        4,
        "root, big, inner, link are stored"
    );
}

#[test]
fn settings_are_saved_replaced_and_absent_until_set() {
    let s = Store::open_in_memory().unwrap();
    assert_eq!(s.setting("ai.enabled").unwrap(), None);
    s.set_setting("ai.enabled", "true").unwrap();
    s.set_setting("ai.enabled", "false").unwrap();
    assert_eq!(s.setting("ai.enabled").unwrap().as_deref(), Some("false"));
}
