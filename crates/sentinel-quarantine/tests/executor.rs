//! Executor tests. Everything happens inside temporary fixture folders; nothing outside
//! them is touched.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs::{self, OpenOptions};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sentinel_cleanup::providers::UserTemp;
use sentinel_quarantine::{Context, EntryStatus, Quarantine, QuarantineError, RETENTION_MS};
use sentinel_safety::{Policy, ProtectedSet};
use sentinel_store::{AuditKind, NewAuditRecord, Outcome};

const DAY: Duration = Duration::from_secs(24 * 60 * 60);
const DAY_MS: i64 = 24 * 60 * 60 * 1000;

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

fn age(path: &Path, days: u32) {
    let f = OpenOptions::new()
        .access_mode(0x0100) // FILE_WRITE_ATTRIBUTES
        .custom_flags(0x0200_0000 | 0x0020_0000) // backup semantics, open reparse point
        .open(path)
        .unwrap();
    f.set_modified(SystemTime::now() - DAY * days).unwrap();
}

fn put(path: &Path, bytes: &[u8], days: u32) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
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
    age(link, 30);
}

struct Fx {
    _dir: tempfile::TempDir,
    temp: PathBuf,
    outside: PathBuf,
    q: Quarantine,
    policy: Policy,
    provider: UserTemp,
}

/// temp/old.log, temp/olddir/{a,b}: 30 days old. outside/precious.bin.
fn fx() -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let base = sentinel_safety::CanonicalPath::resolve(dir.path())
        .unwrap()
        .as_path()
        .to_path_buf();
    let temp = base.join("temp");
    let outside = base.join("outside");
    put(&temp.join("old.log"), b"old log contents", 30);
    put(&temp.join("olddir").join("a"), b"aaaa", 30);
    put(&temp.join("olddir").join("b"), b"bbbb", 30);
    age(&temp.join("olddir"), 30);
    put(&outside.join("precious.bin"), b"precious", 30);
    Fx {
        provider: UserTemp::with_root(temp.clone()),
        q: Quarantine::at(base.join("q")),
        policy: Policy::new(ProtectedSet::new()),
        _dir: dir,
        temp,
        outside,
    }
}

fn ctx(op: &str, at: i64) -> Context<'_> {
    Context {
        operation_id: op,
        user: "tester",
        now_ms: at,
    }
}

type Log = Vec<NewAuditRecord>;

fn sink(log: &mut Log) -> impl FnMut(NewAuditRecord) -> Result<i64, String> + '_ {
    move |r| {
        log.push(r);
        Ok(log.len() as i64)
    }
}

fn status(m: &sentinel_quarantine::Manifest, i: usize) -> &EntryStatus {
    &m.entries[i].status
}

#[test]
fn moves_eligible_items_and_restores_them() {
    let f = fx();
    let mut log = Log::new();
    let approved = [f.temp.join("old.log"), f.temp.join("olddir")];
    let m =
        f.q.quarantine(
            &f.policy,
            &f.provider,
            &approved,
            &ctx("op-1", now_ms()),
            &mut sink(&mut log),
        )
        .unwrap();
    assert_eq!(status(&m, 0), &EntryStatus::Quarantined);
    assert_eq!(status(&m, 1), &EntryStatus::Quarantined);
    assert!(!f.temp.join("old.log").exists());
    assert!(!f.temp.join("olddir").exists());
    let op = f.q.root().join("op-1");
    assert_eq!(fs::read(op.join("0")).unwrap(), b"old log contents");
    assert_eq!(fs::read(op.join("1").join("a")).unwrap(), b"aaaa");
    assert!(op.join("manifest.json").exists());
    assert_eq!(m.expires_at_ms - m.created_at_ms, RETENTION_MS);

    // Audit: Started first, then the outcome.
    assert_eq!(log.len(), 2);
    assert_eq!(
        (log[0].kind, log[0].outcome),
        (AuditKind::Quarantine, Outcome::Started)
    );
    assert_eq!((log[1].outcome, log[1].items), (Outcome::Succeeded, 2));

    let entry =
        f.q.restore(
            &f.policy,
            "op-1",
            0,
            &ctx("op-2", now_ms()),
            &mut sink(&mut log),
        )
        .unwrap();
    assert_eq!(entry.status, EntryStatus::Restored);
    assert_eq!(
        fs::read(f.temp.join("old.log")).unwrap(),
        b"old log contents"
    );
    assert_eq!(
        (log[2].kind, log[2].outcome),
        (AuditKind::Restore, Outcome::Started)
    );
    assert_eq!(log[3].outcome, Outcome::Succeeded);

    let again = f.q.restore(
        &f.policy,
        "op-1",
        0,
        &ctx("op-3", now_ms()),
        &mut sink(&mut log),
    );
    assert!(matches!(again, Err(QuarantineError::Refused(_))));
}

#[test]
fn restore_never_overwrites() {
    let f = fx();
    let mut log = Log::new();
    f.q.quarantine(
        &f.policy,
        &f.provider,
        &[f.temp.join("old.log")],
        &ctx("op-1", now_ms()),
        &mut sink(&mut log),
    )
    .unwrap();
    fs::write(f.temp.join("old.log"), b"new file").unwrap();
    let r = f.q.restore(
        &f.policy,
        "op-1",
        0,
        &ctx("op-2", now_ms()),
        &mut sink(&mut log),
    );
    assert!(matches!(r, Err(QuarantineError::Refused(_))));
    assert_eq!(fs::read(f.temp.join("old.log")).unwrap(), b"new file");
    assert_eq!(
        fs::read(f.q.root().join("op-1").join("0")).unwrap(),
        b"old log contents"
    );
}

#[test]
fn skips_items_that_changed_since_the_preview() {
    let f = fx();
    fs::write(f.temp.join("old.log"), b"just written").unwrap();
    let mut log = Log::new();
    let m =
        f.q.quarantine(
            &f.policy,
            &f.provider,
            &[f.temp.join("old.log")],
            &ctx("op-1", now_ms()),
            &mut sink(&mut log),
        )
        .unwrap();
    assert!(
        matches!(status(&m, 0), EntryStatus::Skipped { reason } if reason.contains("changed recently"))
    );
    assert_eq!(fs::read(f.temp.join("old.log")).unwrap(), b"just written");
    assert!(!f.q.root().join("op-1").exists(), "empty operation removed");
    assert_eq!(log[1].outcome, Outcome::NoChanges);
}

#[test]
fn skips_items_that_became_protected() {
    let f = fx();
    put(&f.temp.join("olddir").join(".git").join("HEAD"), b"ref", 30);
    age(&f.temp.join("olddir").join(".git"), 30);
    age(&f.temp.join("olddir"), 30);
    let mut log = Log::new();
    let m =
        f.q.quarantine(
            &f.policy,
            &f.provider,
            &[f.temp.join("olddir")],
            &ctx("op-1", now_ms()),
            &mut sink(&mut log),
        )
        .unwrap();
    assert!(
        matches!(status(&m, 0), EntryStatus::Skipped { reason } if reason.contains("protected"))
    );
    assert!(f.temp.join("olddir").join(".git").join("HEAD").exists());
}

#[test]
fn skips_files_in_use() {
    let f = fx();
    let _busy = OpenOptions::new()
        .read(true)
        .share_mode(0x1 | 0x2) // read + write, no delete
        .open(f.temp.join("old.log"))
        .unwrap();
    let mut log = Log::new();
    let m =
        f.q.quarantine(
            &f.policy,
            &f.provider,
            &[f.temp.join("old.log")],
            &ctx("op-1", now_ms()),
            &mut sink(&mut log),
        )
        .unwrap();
    assert!(
        matches!(status(&m, 0), EntryStatus::Skipped { reason } if reason.contains("in use")),
        "{:?}",
        status(&m, 0)
    );
    assert!(f.temp.join("old.log").exists());
}

#[test]
fn never_touches_unapproved_nested_or_outside_items() {
    let f = fx();
    let mut log = Log::new();
    let approved = [
        f.outside.join("precious.bin"),
        f.temp.join("olddir").join("a"),
        f.temp.join("does-not-exist"),
    ];
    let m =
        f.q.quarantine(
            &f.policy,
            &f.provider,
            &approved,
            &ctx("op-1", now_ms()),
            &mut sink(&mut log),
        )
        .unwrap();
    for i in 0..3 {
        assert!(
            matches!(status(&m, i), EntryStatus::Skipped { .. }),
            "{i}: {:?}",
            status(&m, i)
        );
    }
    assert!(f.outside.join("precious.bin").exists());
    assert!(f.temp.join("olddir").join("a").exists());
    assert!(f.temp.join("old.log").exists(), "unapproved items stay");
}

#[test]
fn audit_failure_changes_nothing() {
    let f = fx();
    let mut failing = |_r: NewAuditRecord| -> Result<i64, String> { Err("disk full".into()) };
    let r = f.q.quarantine(
        &f.policy,
        &f.provider,
        &[f.temp.join("old.log")],
        &ctx("op-1", now_ms()),
        &mut failing,
    );
    assert!(matches!(r, Err(QuarantineError::AuditUnavailable(_))));
    assert!(f.temp.join("old.log").exists());
    assert!(
        !f.q.root().exists(),
        "not even the quarantine folder was created"
    );
}

#[test]
fn link_items_move_only_the_link() {
    let f = fx();
    junction(&f.temp.join("link"), &f.outside);
    let mut log = Log::new();
    let m =
        f.q.quarantine(
            &f.policy,
            &f.provider,
            &[f.temp.join("link")],
            &ctx("op-1", now_ms()),
            &mut sink(&mut log),
        )
        .unwrap();
    assert_eq!(status(&m, 0), &EntryStatus::Quarantined);
    assert_eq!(
        fs::read(f.outside.join("precious.bin")).unwrap(),
        b"precious"
    );
    let moved = fs::symlink_metadata(f.q.root().join("op-1").join("0")).unwrap();
    assert!(
        moved.file_attributes() & 0x400 != 0,
        "the link itself moved"
    );
}

#[test]
fn purge_removes_only_expired_operations_and_never_follows_links() {
    let f = fx();
    junction(&f.temp.join("olddir").join("inner-link"), &f.outside);
    age(&f.temp.join("olddir"), 30);
    let t0 = now_ms();
    let mut log = Log::new();
    f.q.quarantine(
        &f.policy,
        &f.provider,
        &[f.temp.join("olddir")],
        &ctx("op-old", t0),
        &mut sink(&mut log),
    )
    .unwrap();
    f.q.quarantine(
        &f.policy,
        &f.provider,
        &[f.temp.join("old.log")],
        &ctx("op-new", t0 + 10 * DAY_MS),
        &mut sink(&mut log),
    )
    .unwrap();

    let report =
        f.q.purge_expired(&ctx("op-purge", t0 + 15 * DAY_MS), &mut sink(&mut log))
            .unwrap();
    assert_eq!((report.operations_purged, report.entries_purged), (1, 1));
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert!(!f.q.root().join("op-old").exists());
    assert!(f.q.root().join("op-new").join("0").exists(), "not expired");
    assert_eq!(
        fs::read(f.outside.join("precious.bin")).unwrap(),
        b"precious"
    );
    let purge: Vec<_> = log.iter().filter(|r| r.kind == AuditKind::Purge).collect();
    assert_eq!(
        purge.iter().map(|r| r.outcome).collect::<Vec<_>>(),
        [Outcome::Started, Outcome::Succeeded]
    );
}

#[test]
fn refuses_a_quarantine_folder_that_is_a_link() {
    let f = fx();
    let elsewhere = f.outside.join("redirected");
    fs::create_dir(&elsewhere).unwrap();
    junction(f.q.root(), &elsewhere);
    let mut log = Log::new();
    let r = f.q.quarantine(
        &f.policy,
        &f.provider,
        &[f.temp.join("old.log")],
        &ctx("op-1", now_ms()),
        &mut sink(&mut log),
    );
    assert!(
        matches!(r, Err(QuarantineError::UnsafeLocation(_))),
        "{r:?}"
    );
    assert!(f.temp.join("old.log").exists());
    assert_eq!(log.last().unwrap().outcome, Outcome::Failed);
    assert!(fs::read_dir(&elsewhere).unwrap().next().is_none());
}

#[test]
fn lists_operations_and_reports_unreadable_manifests() {
    let f = fx();
    let mut log = Log::new();
    f.q.quarantine(
        &f.policy,
        &f.provider,
        &[f.temp.join("old.log")],
        &ctx("op-1", now_ms()),
        &mut sink(&mut log),
    )
    .unwrap();
    fs::create_dir(f.q.root().join("op-broken")).unwrap();
    fs::write(f.q.root().join("op-broken").join("manifest.json"), b"{nope").unwrap();
    let (ops, problems) = f.q.operations().unwrap();
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].operation_id, "op-1");
    assert_eq!(problems.len(), 1);
}

/// The 8.3 short form of `path`, if the volume has short names enabled.
fn short_path(path: &Path) -> Option<PathBuf> {
    let out = Command::new("cmd")
        .arg("/C")
        .arg(format!("for %I in (\"{}\") do @echo %~sI", path.display()))
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    let p = PathBuf::from(s);
    (p.exists() && p != path).then_some(p)
}

#[test]
fn a_quarantine_path_spelled_with_short_names_works() {
    let f = fx();
    let long = f.q.root().parent().unwrap().join("a long quarantine name");
    fs::create_dir(&long).unwrap();
    let Some(short) = short_path(&long) else {
        eprintln!("SKIPPED: 8.3 names are disabled on this volume");
        return;
    };
    let q = Quarantine::at(short.join("q"));
    let mut log = Log::new();
    let m = q
        .quarantine(
            &f.policy,
            &f.provider,
            &[f.temp.join("old.log")],
            &ctx("op-1", now_ms()),
            &mut sink(&mut log),
        )
        .unwrap();
    assert_eq!(status(&m, 0), &EntryStatus::Quarantined);
    assert!(long.join("q").join("op-1").join("0").exists());
}

#[test]
fn refuses_a_quarantine_folder_whose_parent_is_a_link() {
    let f = fx();
    let real_parent = f.outside.join("real-parent");
    fs::create_dir(&real_parent).unwrap();
    let linked_parent = f.q.root().parent().unwrap().join("linked-parent");
    junction(&linked_parent, &real_parent);
    let q = Quarantine::at(linked_parent.join("q"));
    let mut log = Log::new();
    let r = q.quarantine(
        &f.policy,
        &f.provider,
        &[f.temp.join("old.log")],
        &ctx("op-1", now_ms()),
        &mut sink(&mut log),
    );
    assert!(
        matches!(r, Err(QuarantineError::UnsafeLocation(_))),
        "{r:?}"
    );
    assert!(f.temp.join("old.log").exists());
}

mod caches {
    use super::*;
    use sentinel_cleanup::providers::{CacheKind, PackageCache};

    /// npm-cache/{_cacache, _npx, my-notes}, all 30 days old.
    fn npm_fixture(f: &Fx) -> (PathBuf, PackageCache) {
        let root = f.temp.parent().unwrap().join("npm-cache");
        put(
            &root.join("_cacache").join("index-v5").join("a"),
            b"index",
            30,
        );
        put(&root.join("_npx").join("x").join("package.json"), b"{}", 30);
        put(&root.join("my-notes").join("todo.txt"), b"mine", 30);
        for d in [
            "_cacache/index-v5",
            "_cacache",
            "_npx/x",
            "_npx",
            "my-notes",
        ] {
            age(&root.join(d), 30);
        }
        (root.clone(), PackageCache::with_root(CacheKind::Npm, root))
    }

    #[test]
    fn moves_only_the_caches_known_folders() {
        let f = fx();
        let (root, npm) = npm_fixture(&f);
        let mut log = Log::new();
        let approved = [
            root.join("_cacache"),
            root.join("_npx"),
            root.join("my-notes"),
        ];
        let m =
            f.q.quarantine(
                &f.policy,
                &npm,
                &approved,
                &ctx("op-1", now_ms()),
                &mut sink(&mut log),
            )
            .unwrap();
        assert_eq!(status(&m, 0), &EntryStatus::Quarantined);
        assert_eq!(status(&m, 1), &EntryStatus::Quarantined);
        assert!(matches!(status(&m, 2), EntryStatus::Skipped { .. }));
        assert!(root.join("my-notes").join("todo.txt").exists());
        assert!(!root.join("_cacache").exists());
    }

    #[test]
    fn recently_used_caches_are_left_alone() {
        let f = fx();
        let (root, npm) = npm_fixture(&f);
        put(&root.join("_cacache").join("fresh"), b"x", 0);
        let mut log = Log::new();
        let m =
            f.q.quarantine(
                &f.policy,
                &npm,
                &[root.join("_cacache")],
                &ctx("op-1", now_ms()),
                &mut sink(&mut log),
            )
            .unwrap();
        assert!(
            matches!(status(&m, 0), EntryStatus::Skipped { reason } if reason.contains("recently"))
        );
    }

    #[test]
    fn analysis_only_providers_are_refused_before_anything_happens() {
        let f = fx();
        let store = f.temp.parent().unwrap().join("pnpm-store");
        put(&store.join("v10").join("files").join("a"), b"x", 30);
        let pnpm = PackageCache::with_root(CacheKind::Pnpm, store.clone());
        let mut log = Log::new();
        let r = f.q.quarantine(
            &f.policy,
            &pnpm,
            &[store.join("v10")],
            &ctx("op-1", now_ms()),
            &mut sink(&mut log),
        );
        assert!(matches!(r, Err(QuarantineError::Refused(_))));
        assert!(
            log.is_empty(),
            "nothing recorded because nothing was attempted"
        );
        assert!(store.join("v10").exists());
        assert!(!f.q.root().exists());
    }
}

mod projects {
    use super::*;
    use sentinel_cleanup::providers::ProjectArtifacts;
    use sentinel_cleanup::{Decision, PreviewLimits, preview};
    use std::sync::atomic::AtomicBool;

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    }

    /// Age every file and folder under `dir` (bottom-up), including `.git`.
    fn age_tree(dir: &Path, days: u32) {
        for e in fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if fs::symlink_metadata(&p).unwrap().is_dir() {
                age_tree(&p, days);
            }
            age(&p, days);
        }
        age(dir, days);
    }

    /// A Git repository `name` under `base` with a committed package.json and src,
    /// untracked node_modules, and (for `gradle`) committed build output.
    fn project(base: &Path, name: &str, gradle: bool) -> PathBuf {
        let p = base.join(name);
        put(&p.join("package.json"), br#"{"name":"x"}"#, 0);
        put(&p.join("src").join("index.js"), b"code", 0);
        put(
            &p.join("node_modules").join("react").join("index.js"),
            &[b'x'; 5000],
            0,
        );
        git(&p, &["init", "-q"]);
        let mut add = vec!["add", "package.json", "src"];
        if gradle {
            put(&p.join("build.gradle"), b"", 0);
            put(&p.join("build").join("libs").join("app.jar"), b"jar", 0);
            add.extend(["build.gradle", "build"]);
        }
        git(&p, &add);
        git(&p, &["commit", "-q", "-m", "init"]);
        p
    }

    #[test]
    fn moves_untracked_artifacts_of_inactive_projects_only() {
        let f = fx();
        let base = f.temp.parent().unwrap().join("code");
        let old = project(&base, "old", true);
        let active = project(&base, "active", false);
        age_tree(&old, 120);
        age_tree(&active, 120);
        put(&active.join("src").join("new.js"), b"today", 0);

        let provider = ProjectArtifacts::for_projects(&[old.clone(), active.clone()], now_ms());
        let pv = preview(
            &provider,
            &f.policy,
            now_ms(),
            PreviewLimits::default(),
            &AtomicBool::new(false),
        );
        let find = |suffix: &str| {
            pv.items
                .iter()
                .find(|i| i.path.ends_with(suffix))
                .unwrap_or_else(|| panic!("{suffix} missing: {:#?}", pv.items))
        };
        assert_eq!(find(r"old\node_modules").decision, Decision::Eligible);
        assert!(
            matches!(&find(r"old\build").decision, Decision::Protected { reason } if reason.contains("Git"))
        );
        assert!(
            matches!(&find("active").decision, Decision::Skipped { reason } if reason.contains("changed 0 days ago"))
        );
        assert_eq!(pv.eligible_items, 1);

        // Approve everything, including things that must not move.
        let approved = [
            old.join("node_modules"),
            old.join("build"),
            old.join("src"),
            active.join("node_modules"),
        ];
        let mut log = Log::new();
        let m =
            f.q.quarantine(
                &f.policy,
                &provider,
                &approved,
                &ctx("op-1", now_ms()),
                &mut sink(&mut log),
            )
            .unwrap();
        assert_eq!(status(&m, 0), &EntryStatus::Quarantined);
        for i in 1..4 {
            assert!(
                matches!(status(&m, i), EntryStatus::Skipped { .. }),
                "{i}: {:?}",
                status(&m, i)
            );
        }
        assert!(!old.join("node_modules").exists());
        assert!(
            old.join("build").join("libs").join("app.jar").exists(),
            "tracked output kept"
        );
        assert!(
            old.join("src").join("index.js").exists(),
            "source never touched"
        );
        assert!(
            active.join("node_modules").exists(),
            "active project untouched"
        );

        // And it comes back.
        f.q.restore(
            &f.policy,
            "op-1",
            0,
            &ctx("op-2", now_ms()),
            &mut sink(&mut log),
        )
        .unwrap();
        assert!(
            old.join("node_modules")
                .join("react")
                .join("index.js")
                .exists()
        );
    }

    #[test]
    fn a_project_that_disappeared_or_changed_is_explained() {
        let f = fx();
        let gone = f.temp.parent().unwrap().join("gone");
        let provider = ProjectArtifacts::for_projects(&[gone], now_ms());
        let pv = preview(
            &provider,
            &f.policy,
            now_ms(),
            PreviewLimits::default(),
            &AtomicBool::new(false),
        );
        assert_eq!(pv.items.len(), 1);
        assert!(
            matches!(&pv.items[0].decision, Decision::Skipped { reason } if reason.contains("no longer exists"))
        );
        assert_eq!(pv.eligible_items, 0);
    }
}
