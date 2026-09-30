//! Dry-run preview tests. Fixtures are built in temporary directories; the preview
//! itself must never change them, and one test proves that.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sentinel_cleanup::providers::UserTemp;
use sentinel_cleanup::{
    CleanupProvider, Decision, ItemKind, Preview, PreviewLimits, RootReport, preview,
};
use sentinel_safety::{Policy, ProtectedSet};

const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const FILE_WRITE_ATTRIBUTES: u32 = 0x0100;
const DAY: Duration = Duration::from_secs(24 * 60 * 60);

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

/// Set the modification time of a file, folder or link (not its target) to `days` ago.
fn age(path: &Path, days: u32) {
    let f = OpenOptions::new()
        .access_mode(FILE_WRITE_ATTRIBUTES)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .unwrap();
    f.set_modified(SystemTime::now() - DAY * days).unwrap();
}

fn put(path: &Path, bytes: usize, days: u32) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, vec![b'x'; bytes]).unwrap();
    age(path, days);
}

fn junction(link: &Path, target: &Path) {
    let out = Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
}

struct Fixture {
    dir: tempfile::TempDir,
    root: PathBuf,
    outside: PathBuf,
}

/// temp/
///   old.log            30 days, eligible
///   new.log            today, too recent
///   olddir/a, b        30 days, eligible folder
///   mixed/old, new     one recent file, so the folder is too recent
///   secret/.env        protected content
///   repo/.git/HEAD     protected content
///   link  -> outside   junction; only the link counts
///   holder/inner-link  junction inside an old folder; not followed
fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("temp");
    let outside = dir.path().join("outside");
    put(&outside.join("precious.bin"), 50_000, 30);
    put(&root.join("old.log"), 4000, 30);
    put(&root.join("new.log"), 100, 0);
    put(&root.join("olddir").join("a"), 5000, 30);
    put(&root.join("olddir").join("b"), 5000, 30);
    age(&root.join("olddir"), 30);
    put(&root.join("mixed").join("old"), 5000, 30);
    put(&root.join("mixed").join("new"), 5000, 0);
    put(&root.join("secret").join(".env"), 10, 30);
    put(&root.join("repo").join(".git").join("HEAD"), 10, 30);
    junction(&root.join("link"), &outside);
    put(&root.join("holder").join("f"), 3000, 30);
    junction(&root.join("holder").join("inner-link"), &outside);
    age(&root.join("holder").join("inner-link"), 30);
    age(&root.join("holder"), 30);
    Fixture { dir, root, outside }
}

fn run(f: &Fixture, policy: &Policy) -> Preview {
    let provider = UserTemp::with_root(f.root.clone());
    preview(
        &provider,
        policy,
        now_ms(),
        PreviewLimits::default(),
        &AtomicBool::new(false),
    )
}

fn item<'a>(p: &'a Preview, name: &str) -> &'a sentinel_cleanup::PreviewItem {
    p.items
        .iter()
        .find(|i| Path::new(&i.path).file_name().unwrap() == name)
        .unwrap_or_else(|| panic!("{name} missing"))
}

#[test]
fn decides_each_candidate_for_the_right_reason() {
    let f = fixture();
    let p = run(&f, &Policy::new(ProtectedSet::new()));
    assert!(p.dry_run);

    assert_eq!(item(&p, "old.log").decision, Decision::Eligible);
    assert!(matches!(
        item(&p, "new.log").decision,
        Decision::TooRecent { .. }
    ));

    let olddir = item(&p, "olddir");
    assert_eq!(olddir.decision, Decision::Eligible);
    assert_eq!((olddir.kind, olddir.files), (ItemKind::Folder, 2));
    assert!(olddir.bytes >= 10_000);

    assert!(matches!(
        item(&p, "mixed").decision,
        Decision::TooRecent { .. }
    ));
    for name in ["secret", "repo"] {
        assert!(
            matches!(item(&p, name).decision, Decision::Protected { .. }),
            "{name}: {:?}",
            item(&p, name).decision
        );
    }

    let link = item(&p, "link");
    assert_eq!((link.kind, link.bytes, link.files), (ItemKind::Link, 0, 0));

    let holder = item(&p, "holder");
    assert_eq!(holder.decision, Decision::Eligible);
    assert_eq!(
        holder.files, 1,
        "the inner junction is neither followed nor counted"
    );
    assert!(holder.bytes < 50_000);
}

#[test]
fn totals_cover_exactly_the_eligible_items() {
    let f = fixture();
    let p = run(&f, &Policy::new(ProtectedSet::new()));
    let eligible: Vec<_> = p
        .items
        .iter()
        .filter(|i| i.decision == Decision::Eligible)
        .collect();
    assert_eq!(p.eligible_items, eligible.len() as u64);
    assert_eq!(
        p.eligible_bytes,
        eligible.iter().map(|i| i.bytes).sum::<u64>()
    );
    assert_eq!(
        p.eligible_files,
        eligible.iter().map(|i| i.files).sum::<u64>()
    );
    assert!(!p.incomplete);
    assert!(matches!(&p.roots[..], [RootReport::Scanned { .. }]));
    let sizes: Vec<_> = p.items.iter().map(|i| i.bytes).collect();
    assert!(sizes.windows(2).all(|w| w[0] >= w[1]), "largest first");
}

/// Every path, size and modification time under `dir`, without following links.
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, (u64, SystemTime, bool)> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).unwrap() {
            let e = e.unwrap();
            let md = fs::symlink_metadata(e.path()).unwrap();
            if md.is_dir() {
                stack.push(e.path());
            }
            out.insert(
                e.path(),
                (
                    md.len(),
                    md.modified().unwrap(),
                    md.file_type().is_symlink(),
                ),
            );
        }
    }
    out
}

#[test]
fn preview_changes_nothing() {
    let f = fixture();
    let before = snapshot(f.dir.path());
    let p = run(&f, &Policy::new(ProtectedSet::new()));
    assert!(p.eligible_items > 0);
    assert_eq!(snapshot(f.dir.path()), before);
    assert!(f.outside.join("precious.bin").exists());
}

#[test]
fn missing_and_protected_roots_are_reported_not_scanned() {
    let f = fixture();
    let missing = UserTemp::with_root(f.dir.path().join("nope"));
    let p = preview(
        &missing,
        &Policy::new(ProtectedSet::new()),
        now_ms(),
        PreviewLimits::default(),
        &AtomicBool::new(false),
    );
    assert!(matches!(&p.roots[..], [RootReport::Missing { .. }]));
    assert!(p.items.is_empty());

    let mut set = ProtectedSet::new();
    set.add_root(&f.root, "test-protected");
    let p = run(&f, &Policy::new(set));
    assert!(matches!(&p.roots[..], [RootReport::Unavailable { .. }]));
    assert!(p.items.is_empty());
}

#[test]
fn cancellation_and_limits_mark_the_preview_incomplete() {
    let f = fixture();
    let provider = UserTemp::with_root(f.root.clone());
    let policy = Policy::new(ProtectedSet::new());
    let p = preview(
        &provider,
        &policy,
        now_ms(),
        PreviewLimits::default(),
        &AtomicBool::new(true),
    );
    assert!(p.incomplete);
    assert_eq!(p.eligible_items, 0);

    let tight = PreviewLimits {
        max_depth: 128,
        max_entries: 1,
    };
    let p = preview(&provider, &policy, now_ms(), tight, &AtomicBool::new(false));
    assert!(p.incomplete);
    assert!(
        p.items
            .iter()
            .any(|i| matches!(i.decision, Decision::Skipped { .. }))
    );
}

#[test]
fn user_temp_provider_describes_itself() {
    let p = UserTemp::for_system();
    let info = p.info();
    assert_eq!(info.id, "user-temp");
    assert_eq!(info.min_age_days, 7);
    let roots = p.roots();
    assert_eq!(roots.len(), 1);
    assert!(roots[0].ends_with(r"AppData\Local\Temp"), "{roots:?}");
}

// Read-only preview of this machine's real TEMP folder: inspection only.
#[test]
fn previews_the_real_user_temp_folder_read_only() {
    let p = preview(
        &UserTemp::for_system(),
        &Policy::for_system(),
        now_ms(),
        PreviewLimits::default(),
        &AtomicBool::new(false),
    );
    assert!(p.dry_run);
    assert!(
        matches!(&p.roots[..], [RootReport::Scanned { .. }]),
        "{:?}",
        p.roots
    );
    eprintln!(
        "real TEMP: {} items, {} eligible ({} bytes)",
        p.items.len(),
        p.eligible_items,
        p.eligible_bytes
    );
}

#[test]
fn a_recently_created_link_inside_keeps_the_folder() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("temp");
    let outside = dir.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    put(&root.join("busy").join("old.bin"), 1000, 30);
    junction(&root.join("busy").join("fresh-link"), &outside);
    age(&root.join("busy"), 30);
    let p = preview(
        &UserTemp::with_root(root),
        &Policy::new(ProtectedSet::new()),
        now_ms(),
        PreviewLimits::default(),
        &AtomicBool::new(false),
    );
    assert!(matches!(p.items[0].decision, Decision::TooRecent { .. }));
}

mod caches {
    use super::*;
    use sentinel_cleanup::providers::{CacheKind, PackageCache};

    #[test]
    fn previews_list_only_known_cache_folders() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("pip-cache");
        put(&root.join("http-v2").join("a"), 5000, 30);
        age(&root.join("http-v2"), 30);
        put(&root.join("selfcheck").join("state.json"), 10, 30);
        put(&root.join("unrelated").join("x"), 10, 30);
        let p = preview(
            &PackageCache::with_root(CacheKind::Pip, root),
            &Policy::new(ProtectedSet::new()),
            now_ms(),
            PreviewLimits::default(),
            &AtomicBool::new(false),
        );
        let names: Vec<_> = p
            .items
            .iter()
            .map(|i| {
                Path::new(&i.path)
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(names, ["http-v2"]);
        assert_eq!(p.items[0].decision, Decision::Eligible);
    }

    #[test]
    fn analysis_only_caches_show_size_but_nothing_is_eligible() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("store");
        put(&root.join("v10").join("files").join("a"), 8000, 30);
        age(&root.join("v10").join("files"), 30);
        age(&root.join("v10"), 30);
        let p = preview(
            &PackageCache::with_root(CacheKind::Pnpm, root),
            &Policy::new(ProtectedSet::new()),
            now_ms(),
            PreviewLimits::default(),
            &AtomicBool::new(false),
        );
        assert_eq!(p.eligible_items, 0);
        assert!(p.items[0].bytes >= 8000);
        assert!(
            matches!(&p.items[0].decision, Decision::Skipped { reason } if reason.contains("pnpm store prune"))
        );
    }
}
