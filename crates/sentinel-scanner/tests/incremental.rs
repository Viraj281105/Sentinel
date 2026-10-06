//! Incremental rescan must equal a fresh scan. Fixtures live in temporary folders.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sentinel_scanner::incremental::{RescanError, rescan};
use sentinel_scanner::scan::{NodeId, NodeStatus, ScanControl, ScanOptions, ScanTree, scan};
use sentinel_scanner::watch::{Changes, WatchOptions, Watcher};

fn write(path: &Path, bytes: usize) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, vec![b'x'; bytes]).unwrap();
}

fn opts() -> ScanOptions {
    ScanOptions {
        threads: 2,
        ..ScanOptions::default()
    }
}

fn full(root: &Path) -> ScanTree {
    scan(root, &opts(), &ScanControl::new()).unwrap()
}

fn dirty(root: &Path, rels: &[&str]) -> Changes {
    Changes {
        dirty_dirs: rels
            .iter()
            .map(|r| {
                if r.is_empty() {
                    root.to_path_buf()
                } else {
                    root.join(r)
                }
            })
            .collect(),
        ..Changes::default()
    }
}

type Row = (u64, u64, u64, u64, u64, String);

/// Every node by lower-cased relative path: sizes, counts and status. Order-independent.
fn shape(t: &ScanTree) -> BTreeMap<String, Row> {
    fn go(t: &ScanTree, id: NodeId, rel: String, out: &mut BTreeMap<String, Row>) {
        let n = &t.nodes[id as usize];
        let status = match &n.status {
            NodeStatus::Complete => "complete".to_owned(),
            other => format!("{other:?}"),
        };
        out.insert(
            rel.to_lowercase(),
            (
                n.own_bytes,
                n.total_bytes,
                n.total_logical_bytes,
                n.file_count,
                n.dir_count,
                status,
            ),
        );
        for &c in &n.children {
            let child = &t.nodes[c as usize];
            go(t, c, format!("{rel}\\{}", child.name), out);
        }
    }
    let mut out = BTreeMap::new();
    go(t, ScanTree::ROOT, String::new(), &mut out);
    out
}

fn assert_same(inc: &ScanTree, fresh: &ScanTree) {
    assert_eq!(shape(inc), shape(fresh), "tree differs from a fresh scan");
    assert_eq!(inc.stats.dirs, fresh.stats.dirs);
    assert_eq!(inc.stats.files, fresh.stats.files);
    assert_eq!(inc.stats.total_bytes, fresh.stats.total_bytes);
    assert_eq!(
        inc.stats.total_logical_bytes,
        fresh.stats.total_logical_bytes
    );
    assert_eq!(inc.stats.access_denied, fresh.stats.access_denied);
    assert_eq!(inc.stats.links_skipped, fresh.stats.links_skipped);
    let key = |t: &ScanTree| -> Vec<(u64, String)> {
        t.largest_files
            .iter()
            .map(|f| (f.bytes, f.path.to_lowercase()))
            .collect()
    };
    let (mut a, mut b) = (key(inc), key(fresh));
    a.sort();
    b.sort(); // equal sizes may be ordered differently
    assert_eq!(a, b, "largest files differ");
}

fn fixture() -> (tempfile::TempDir, PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let r = d.path().to_path_buf();
    write(&r.join("a.bin"), 5000);
    write(&r.join("sub").join("b.bin"), 3000);
    write(&r.join("sub").join("deep").join("c.bin"), 7000);
    write(&r.join("other").join("o.bin"), 1000);
    fs::create_dir(r.join("empty")).unwrap();
    (d, r)
}

#[test]
fn a_new_changed_and_deleted_file_match_a_fresh_scan() {
    let (_g, r) = fixture();
    let mut tree = full(&r);

    write(&r.join("sub").join("new.bin"), 9000); // added
    write(&r.join("sub").join("b.bin"), 100); // shrunk
    fs::remove_file(r.join("a.bin")).unwrap(); // removed at the root

    let rep = rescan(
        &mut tree,
        &dirty(&r, &["sub", ""]),
        &opts(),
        &ScanControl::new(),
    )
    .unwrap();
    assert_eq!(rep.dirs_refreshed, 2);
    assert_same(&tree, &full(&r));
}

#[test]
fn a_new_directory_with_content_is_read_completely() {
    let (_g, r) = fixture();
    let mut tree = full(&r);
    write(&r.join("fresh").join("x").join("y").join("deep.bin"), 4000);
    write(&r.join("fresh").join("top.bin"), 2000);

    // The watcher would report the parent (and probably the new folders too).
    rescan(&mut tree, &dirty(&r, &[""]), &opts(), &ScanControl::new()).unwrap();
    assert_same(&tree, &full(&r));
}

#[test]
fn a_deleted_directory_disappears_with_its_subtree() {
    let (_g, r) = fixture();
    let mut tree = full(&r);
    fs::remove_dir_all(r.join("sub")).unwrap();
    rescan(&mut tree, &dirty(&r, &[""]), &opts(), &ScanControl::new()).unwrap();
    assert_same(&tree, &full(&r));
    assert!(!shape(&tree).contains_key(r"\sub\deep"));
}

#[test]
fn a_directory_replaced_by_a_file_and_back() {
    let (_g, r) = fixture();
    let mut tree = full(&r);
    fs::remove_dir_all(r.join("other")).unwrap();
    write(&r.join("other"), 1234); // now a file called "other"
    rescan(&mut tree, &dirty(&r, &[""]), &opts(), &ScanControl::new()).unwrap();
    assert_same(&tree, &full(&r));

    fs::remove_file(r.join("other")).unwrap();
    write(&r.join("other").join("back.bin"), 555);
    rescan(&mut tree, &dirty(&r, &[""]), &opts(), &ScanControl::new()).unwrap();
    assert_same(&tree, &full(&r));
}

#[test]
fn a_noisy_subtree_is_read_again_in_full() {
    let (_g, r) = fixture();
    write(
        &r.join("proj").join("node_modules").join("p1").join("i.js"),
        800,
    );
    let mut tree = full(&r);
    fs::remove_dir_all(r.join("proj").join("node_modules").join("p1")).unwrap();
    write(
        &r.join("proj")
            .join("node_modules")
            .join("p2")
            .join("lib")
            .join("j.js"),
        3000,
    );
    write(
        &r.join("proj").join("node_modules").join("p2").join("k.js"),
        100,
    );

    let changes = Changes {
        dirty_subtrees: vec![r.join("proj").join("node_modules")],
        ..Changes::default()
    };
    let rep = rescan(&mut tree, &changes, &opts(), &ScanControl::new()).unwrap();
    assert_eq!(rep.subtrees_rewalked, 1);
    assert_same(&tree, &full(&r));
}

#[test]
fn case_differences_in_reported_paths_do_not_matter() {
    let (_g, r) = fixture();
    let mut tree = full(&r);
    write(&r.join("sub").join("deep").join("more.bin"), 6000);
    let changes = Changes {
        dirty_dirs: vec![r.join("SUB").join("DEEP")],
        ..Changes::default()
    };
    let rep = rescan(&mut tree, &changes, &opts(), &ScanControl::new()).unwrap();
    assert_eq!(rep.dirs_refreshed, 1);
    assert_same(&tree, &full(&r));
}

#[test]
fn unknown_paths_are_counted_not_fatal() {
    let (_g, r) = fixture();
    let mut tree = full(&r);
    let before = shape(&tree);
    let changes = Changes {
        dirty_dirs: vec![
            r.join("never").join("scanned"),
            PathBuf::from(r"Z:\elsewhere"),
        ],
        ..Changes::default()
    };
    let rep = rescan(&mut tree, &changes, &opts(), &ScanControl::new()).unwrap();
    assert_eq!(rep.skipped_unknown, 2);
    assert_eq!(shape(&tree), before);
}

#[test]
fn lost_events_demand_a_full_scan_and_leave_the_tree_alone() {
    let (_g, r) = fixture();
    let mut tree = full(&r);
    let before = shape(&tree);
    let changes = Changes {
        root_dirty: true,
        ..Changes::default()
    };
    assert!(matches!(
        rescan(&mut tree, &changes, &opts(), &ScanControl::new()),
        Err(RescanError::FullScanNeeded)
    ));
    assert_eq!(shape(&tree), before);
}

#[test]
fn a_cancelled_rescan_does_not_touch_the_tree() {
    let (_g, r) = fixture();
    let mut tree = full(&r);
    let before = shape(&tree);
    write(&r.join("sub").join("new.bin"), 9000);
    let ctl = ScanControl::new();
    ctl.cancel();
    assert!(matches!(
        rescan(&mut tree, &dirty(&r, &["sub"]), &opts(), &ctl),
        Err(RescanError::Cancelled)
    ));
    assert_eq!(shape(&tree), before);
}

#[test]
fn the_largest_files_list_follows_the_changes_and_says_when_it_may_be_incomplete() {
    let (_g, r) = fixture();
    let small = ScanOptions {
        largest_files: 3,
        threads: 2,
        ..ScanOptions::default()
    };
    let mut tree = scan(&r, &small, &ScanControl::new()).unwrap();
    // Top 3 are c (7000), a (5000), b (3000). Delete c: o.bin (1000) would have to come
    // in from outside the rescanned directory, which an incremental rescan cannot know.
    fs::remove_file(r.join("sub").join("deep").join("c.bin")).unwrap();
    let rep = rescan(
        &mut tree,
        &dirty(&r, &["sub\\deep"]),
        &small,
        &ScanControl::new(),
    )
    .unwrap();
    assert!(
        rep.largest_files_may_be_incomplete,
        "o.bin could be missing from the list"
    );
    assert!(
        tree.largest_files
            .iter()
            .all(|f| !f.path.ends_with("c.bin"))
    );

    // A pure addition of a big file cannot make the list incomplete.
    let mut tree = scan(&r, &small, &ScanControl::new()).unwrap();
    write(&r.join("sub").join("huge.bin"), 50_000);
    let rep = rescan(&mut tree, &dirty(&r, &["sub"]), &small, &ScanControl::new()).unwrap();
    assert!(!rep.largest_files_may_be_incomplete);
    assert!(tree.largest_files[0].path.ends_with("huge.bin"));
}

/// End to end: a real watcher feeds the rescan, and the result equals a fresh scan.
#[test]
fn watcher_events_drive_the_rescan_to_a_correct_tree() {
    let (_g, r) = fixture();
    let canon = fs::canonicalize(&r).unwrap();
    let r = PathBuf::from(canon.to_string_lossy().trim_start_matches(r"\\?\"));
    let mut tree = full(&r);
    let w = Watcher::start(&r, &WatchOptions::default()).unwrap();

    write(&r.join("sub").join("deep").join("added.bin"), 8000);
    write(&r.join("newdir").join("n.bin"), 2500);
    fs::remove_file(r.join("other").join("o.bin")).unwrap();
    write(&r.join("a.bin"), 10);

    let mut all = Changes::default();
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let c = w.drain();
        all.dirty_dirs.extend(c.dirty_dirs);
        all.dirty_subtrees.extend(c.dirty_subtrees);
        all.root_dirty |= c.root_dirty;
        let have = |p: PathBuf| all.dirty_dirs.contains(&p);
        if have(r.join("sub").join("deep")) && have(r.join("other")) && have(r.clone()) {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    // Let any straggling events land before judging.
    std::thread::sleep(Duration::from_millis(300));
    let c = w.drain();
    all.dirty_dirs.extend(c.dirty_dirs);
    all.dirty_subtrees.extend(c.dirty_subtrees);
    all.root_dirty |= c.root_dirty;
    assert!(!all.root_dirty, "{all:?}");
    rescan(&mut tree, &all, &opts(), &ScanControl::new()).unwrap();
    assert_same(&tree, &full(&r));
}
