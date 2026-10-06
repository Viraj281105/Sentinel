//! Scanner tests on synthetic fixtures inside temporary directories. The scanner is
//! read-only; the only writes here build and tear down the fixtures themselves.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sentinel_scanner::scan::{
    LinkKind, NodeId, NodeStatus, NotScannedReason, ScanControl, ScanOptions, ScanTree, scan,
};

fn write(path: &Path, bytes: usize) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, vec![b'x'; bytes]).unwrap();
}

fn junction(link: &Path, target: &Path) {
    let out = Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .unwrap();
    assert!(out.status.success(), "mklink /J failed: {out:?}");
}

fn run(root: &Path, opts: &ScanOptions) -> ScanTree {
    scan(root, opts, &ScanControl::new()).unwrap()
}

fn find(tree: &ScanTree, rel: &str) -> NodeId {
    let mut id = ScanTree::ROOT;
    for part in rel.split('\\') {
        id = *tree.nodes[id as usize]
            .children
            .iter()
            .find(|&&c| tree.nodes[c as usize].name == part)
            .unwrap_or_else(|| panic!("{rel}: missing {part}"));
    }
    id
}

/// root/
///   a.bin (5000)
///   sub/b.bin (3000)
///   sub/deep/c.bin (7000)
///   empty/
fn basic_fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    write(&r.join("a.bin"), 5000);
    write(&r.join("sub").join("b.bin"), 3000);
    write(&r.join("sub").join("deep").join("c.bin"), 7000);
    fs::create_dir(r.join("empty")).unwrap();
    dir
}

#[test]
fn aggregates_sizes_and_counts() {
    let dir = basic_fixture();
    let t = run(dir.path(), &ScanOptions::default());
    let root = &t.nodes[0];
    assert_eq!(root.total_logical_bytes, 15_000);
    assert_eq!(root.file_count, 3);
    assert_eq!(root.dir_count, 3);
    assert_eq!(root.status, NodeStatus::Complete);

    let sub = &t.nodes[find(&t, "sub") as usize];
    assert_eq!(sub.total_logical_bytes, 10_000);
    assert_eq!(sub.file_count, 2);
    assert_eq!(t.nodes[find(&t, "empty") as usize].total_bytes, 0);

    // Allocated totals are the sum of own bytes over the subtree.
    let sum: u64 = t.nodes.iter().map(|n| n.own_bytes).sum();
    assert_eq!(root.total_bytes, sum);

    assert_eq!(t.stats.files, 3);
    assert_eq!(t.stats.dirs, 4);
    assert!(!t.stats.cancelled && !t.stats.truncated);
    assert_eq!(
        t.path_of(find(&t, r"sub\deep")).unwrap(),
        t.root.join("sub").join("deep")
    );
}

#[test]
fn children_are_ordered_largest_first() {
    let dir = basic_fixture();
    let t = run(dir.path(), &ScanOptions::default());
    let names: Vec<_> = t
        .children_by_size(ScanTree::ROOT)
        .into_iter()
        .map(|c| t.nodes[c as usize].name.clone())
        .collect();
    assert_eq!(names, ["sub", "empty"]);
}

#[test]
fn largest_files_are_ranked_and_limited() {
    let dir = basic_fixture();
    let opts = ScanOptions {
        largest_files: 2,
        ..ScanOptions::default()
    };
    let t = run(dir.path(), &opts);
    let got: Vec<_> = t.largest_files.iter().map(|f| f.logical_bytes).collect();
    assert_eq!(got, [7000, 5000]);
    assert!(t.largest_files[0].path.ends_with(r"sub\deep\c.bin"));
}

#[test]
fn junctions_are_recorded_but_never_followed() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    let outside = dir.path().join("outside");
    write(&root.join("own.bin"), 1000);
    write(&outside.join("big.bin"), 50_000);
    junction(&root.join("escape"), &outside);
    // A junction pointing back at an ancestor must not loop.
    junction(&root.join("loop"), &root);

    let t = run(&root, &ScanOptions::default());
    assert_eq!(t.nodes[0].total_logical_bytes, 1000);
    for name in ["escape", "loop"] {
        let n = &t.nodes[find(&t, name) as usize];
        assert_eq!(
            n.status,
            NodeStatus::Link {
                kind: LinkKind::Junction
            }
        );
        assert_eq!(n.total_bytes, 0);
    }
    assert_eq!(t.stats.links_skipped, 2);
}

#[test]
fn excluded_paths_are_skipped() {
    let dir = basic_fixture();
    let opts = ScanOptions {
        exclude: vec![dir.path().join("sub")],
        ..ScanOptions::default()
    };
    let t = run(dir.path(), &opts);
    assert_eq!(
        t.nodes[find(&t, "sub") as usize].status,
        NodeStatus::Excluded
    );
    assert_eq!(t.nodes[0].total_logical_bytes, 5000);
    assert_eq!(t.stats.excluded, 1);
}

#[test]
fn cancellation_returns_flagged_partial_result() {
    let dir = basic_fixture();
    let ctl = ScanControl::new();
    ctl.cancel();
    let t = scan(dir.path(), &ScanOptions::default(), &ctl).unwrap();
    assert!(t.stats.cancelled);
    assert_eq!(
        t.nodes[0].status,
        NodeStatus::NotScanned {
            reason: NotScannedReason::Cancelled
        }
    );
    assert_eq!(t.nodes[0].total_bytes, 0);
}

#[test]
fn depth_limit_marks_unscanned_children_and_truncates() {
    let dir = basic_fixture();
    let opts = ScanOptions {
        max_depth: 1,
        ..ScanOptions::default()
    };
    let t = run(dir.path(), &opts);
    let deep = &t.nodes[find(&t, r"sub\deep") as usize];
    assert_eq!(
        deep.status,
        NodeStatus::NotScanned {
            reason: NotScannedReason::DepthLimit
        }
    );
    assert!(t.stats.truncated);
    assert_eq!(t.nodes[0].total_logical_bytes, 8000);
}

#[test]
fn entry_budget_stops_descent() {
    let dir = basic_fixture();
    let opts = ScanOptions {
        max_entries: 2,
        ..ScanOptions::default()
    };
    let t = run(dir.path(), &opts);
    assert!(t.stats.truncated);
    assert_eq!(
        t.nodes[find(&t, "sub") as usize].status,
        NodeStatus::NotScanned {
            reason: NotScannedReason::EntryLimit
        }
    );
}

#[test]
fn unreadable_directory_is_reported_and_scan_continues() {
    let dir = basic_fixture();
    let locked = dir.path().join("locked");
    write(&locked.join("secret.bin"), 9000);
    let deny = Command::new("icacls")
        .arg(&locked)
        .args(["/deny", "*S-1-1-0:(RD)"])
        .output()
        .unwrap();
    assert!(deny.status.success(), "{deny:?}");

    let t = run(dir.path(), &ScanOptions::default());

    let restore = Command::new("icacls")
        .arg(&locked)
        .args(["/remove:d", "*S-1-1-0"])
        .output()
        .unwrap();
    assert!(restore.status.success(), "{restore:?}");

    assert_eq!(
        t.nodes[find(&t, "locked") as usize].status,
        NodeStatus::AccessDenied
    );
    assert_eq!(t.stats.access_denied, 1);
    assert_eq!(t.nodes[0].total_logical_bytes, 15_000);
}

#[test]
fn long_paths_and_unicode_names_work() {
    let dir = tempfile::tempdir().unwrap();
    let mut p: PathBuf = dir.path().to_path_buf();
    for i in 0..12 {
        p.push(format!("level-{i:02}-{}", "x".repeat(24)));
    }
    p.push("日本語フォルダ");
    assert!(p.as_os_str().len() > 300);
    write(&p.join("ファイル.bin"), 4321);

    let t = run(dir.path(), &ScanOptions::default());
    assert_eq!(t.nodes[0].total_logical_bytes, 4321);
    assert!(t.nodes.iter().any(|n| n.name == "日本語フォルダ"));
    assert!(t.largest_files[0].path.ends_with("ファイル.bin"));
}

#[test]
fn progress_counters_match_result() {
    let dir = basic_fixture();
    let ctl = ScanControl::new();
    let t = scan(dir.path(), &ScanOptions::default(), &ctl).unwrap();
    let p = ctl.progress();
    assert_eq!(p.files, t.stats.files);
    assert_eq!(p.dirs, t.stats.dirs);
    assert_eq!(p.bytes, t.stats.total_bytes);
}

#[test]
fn rejects_missing_and_file_roots() {
    let dir = basic_fixture();
    let ctl = ScanControl::new();
    assert!(scan(&dir.path().join("nope"), &ScanOptions::default(), &ctl).is_err());
    assert!(scan(&dir.path().join("a.bin"), &ScanOptions::default(), &ctl).is_err());
}

mod newest_change {
    use std::fs::OpenOptions;
    use std::os::windows::fs::OpenOptionsExt;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use super::*;

    fn age(path: &Path, days: u64) -> i64 {
        let when = SystemTime::now() - Duration::from_secs(days * 86_400);
        // FILE_WRITE_ATTRIBUTES, and BACKUP_SEMANTICS so directories can be opened too.
        OpenOptions::new()
            .access_mode(0x0100)
            .custom_flags(0x0200_0000)
            .open(path)
            .unwrap()
            .set_modified(when)
            .unwrap();
        when.duration_since(UNIX_EPOCH).unwrap().as_millis() as i64
    }

    fn near(a: Option<i64>, b: i64) -> bool {
        a.is_some_and(|a| (a - b).abs() < 5_000)
    }

    #[test]
    fn each_folder_reports_the_newest_change_in_its_subtree() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        write(&r.join("old").join("deep").join("c.bin"), 5000);
        write(&r.join("fresh").join("n.bin"), 3000);
        // Age everything under `old` (the file first, then the folders that hold it).
        let t_file = age(&r.join("old").join("deep").join("c.bin"), 400);
        let t_deep = age(&r.join("old").join("deep"), 400);
        let t_old = age(&r.join("old"), 400);
        assert!(t_file <= t_deep && t_deep <= t_old + 1);

        let tree = run(r, &ScanOptions::default());
        let deep = &tree.nodes[find(&tree, r"old\deep") as usize];
        let old = &tree.nodes[find(&tree, "old") as usize];
        let fresh = &tree.nodes[find(&tree, "fresh") as usize];
        let root = &tree.nodes[ScanTree::ROOT as usize];

        // `deep` holds only c.bin; `old` holds only the folder `deep`.
        assert!(near(deep.own_newest_ms, t_file), "{:?}", deep.own_newest_ms);
        assert!(near(deep.newest_ms, t_file));
        assert!(near(old.own_newest_ms, t_deep), "{:?}", old.own_newest_ms);
        assert!(
            near(old.newest_ms, t_deep),
            "the subtree is as old as its newest entry"
        );
        // A folder is as new as the newest thing under it.
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        assert!(near(fresh.newest_ms, now), "{:?}", fresh.newest_ms);
        assert!(
            near(root.newest_ms, now),
            "the root includes the fresh folder"
        );
        assert!(root.newest_ms >= old.newest_ms);
        // Own time is about direct entries only: the root's direct entries are both folders.
        assert!(near(root.own_newest_ms, now));
    }

    #[test]
    fn the_largest_files_carry_their_modification_time() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        write(&r.join("a").join("big.bin"), 9000);
        write(&r.join("b").join("small.bin"), 100);
        let t = age(&r.join("a").join("big.bin"), 200);
        let tree = run(r, &ScanOptions::default());
        let big = tree
            .largest_files
            .iter()
            .find(|f| f.path.ends_with("big.bin"))
            .unwrap();
        assert!(near(big.modified_ms, t), "{:?}", big.modified_ms);
        let small = tree
            .largest_files
            .iter()
            .find(|f| f.path.ends_with("small.bin"))
            .unwrap();
        assert!(small.modified_ms.is_some_and(|m| m > t));
    }

    #[test]
    fn an_empty_folder_still_reports_when_it_was_last_changed() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("empty")).unwrap();
        let tree = run(dir.path(), &ScanOptions::default());
        let empty = &tree.nodes[find(&tree, "empty") as usize];
        assert_eq!(
            empty.own_newest_ms, None,
            "no entries inside, so nothing to report"
        );
        assert_eq!(empty.newest_ms, None);
        // The root's own entry for it carries the folder's own time.
        assert!(tree.nodes[0].newest_ms.is_some());
    }
}
