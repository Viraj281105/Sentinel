//! Folder watcher against the real file system, in temporary folders. The watcher is
//! read-only; the only writes are the fixtures' own changes.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sentinel_scanner::watch::{Changes, WatchOptions, Watcher};

/// Keep draining until `done` accepts the accumulated changes, or time runs out.
fn collect_until(w: &Watcher, done: impl Fn(&Changes) -> bool) -> Changes {
    let mut all = Changes::default();
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let c = w.drain();
        all.dirty_dirs.extend(c.dirty_dirs);
        all.dirty_subtrees.extend(c.dirty_subtrees);
        all.root_dirty |= c.root_dirty;
        all.events += c.events;
        all.overflows += c.overflows;
        all.dropped_batches += c.dropped_batches;
        if c.broken.is_some() {
            all.broken = c.broken;
        }
        if done(&all) {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    all.dirty_dirs.sort();
    all.dirty_dirs.dedup();
    all
}

fn root() -> (tempfile::TempDir, PathBuf) {
    let d = tempfile::tempdir().unwrap();
    // Resolve 8.3 short names so event paths compare equal to what we build.
    let p = fs::canonicalize(d.path()).unwrap();
    let p = PathBuf::from(p.to_string_lossy().trim_start_matches(r"\?\"));
    (d, p)
}

fn has(c: &Changes, p: &Path) -> bool {
    c.dirty_dirs.iter().any(|d| d == p)
}

#[test]
fn reports_the_directories_whose_contents_changed() {
    let (_g, r) = root();
    fs::create_dir_all(r.join("a").join("b")).unwrap();
    fs::create_dir_all(r.join("quiet")).unwrap();
    let w = Watcher::start(&r, &WatchOptions::default()).unwrap();

    fs::write(r.join("a").join("b").join("new.txt"), b"hello").unwrap();
    fs::write(r.join("top.txt"), b"x").unwrap();

    let c = collect_until(&w, |c| has(c, &r.join("a").join("b")) && has(c, &r));
    assert!(has(&c, &r.join("a").join("b")), "{c:?}");
    assert!(has(&c, &r), "a file at the top marks the root folder");
    assert!(!has(&c, &r.join("quiet")), "untouched folder stays clean");
    assert!(!c.root_dirty && c.broken.is_none());
    assert!(c.events > 0);
}

#[test]
fn deletes_and_renames_dirty_both_sides() {
    let (_g, r) = root();
    fs::create_dir_all(r.join("from")).unwrap();
    fs::create_dir_all(r.join("to")).unwrap();
    fs::write(r.join("from").join("f.txt"), b"x").unwrap();
    fs::write(r.join("from").join("g.txt"), b"x").unwrap();
    let w = Watcher::start(&r, &WatchOptions::default()).unwrap();

    fs::rename(r.join("from").join("f.txt"), r.join("to").join("f.txt")).unwrap();
    fs::remove_file(r.join("from").join("g.txt")).unwrap();

    let c = collect_until(&w, |c| has(c, &r.join("from")) && has(c, &r.join("to")));
    assert!(has(&c, &r.join("from")) && has(&c, &r.join("to")), "{c:?}");
}

#[test]
fn noisy_folders_are_coalesced_into_one_subtree() {
    let (_g, r) = root();
    fs::create_dir_all(r.join("proj").join("node_modules").join("pkg").join("lib")).unwrap();
    let w = Watcher::start(&r, &WatchOptions::default()).unwrap();

    for i in 0..30 {
        fs::write(
            r.join("proj")
                .join("node_modules")
                .join("pkg")
                .join("lib")
                .join(format!("f{i}.js")),
            b"x",
        )
        .unwrap();
    }
    fs::write(r.join("proj").join("package.json"), b"{}").unwrap();

    let nm = r.join("proj").join("node_modules");
    let c = collect_until(&w, |c| {
        c.dirty_subtrees.contains(&nm) && has(c, &r.join("proj"))
    });
    assert!(c.dirty_subtrees.contains(&nm), "{c:?}");
    assert!(
        has(&c, &r.join("proj")),
        "the project folder itself changed too"
    );
    assert!(
        !c.dirty_dirs.iter().any(|d| d.starts_with(&nm)),
        "nothing inside node_modules is tracked singly: {c:?}"
    );
}

#[test]
fn a_stalled_reader_overflows_into_root_dirty() {
    let (_g, r) = root();
    let opts = WatchOptions {
        buffer_bytes: 4096,
        reader_stall: Duration::from_millis(300),
        ..WatchOptions::default()
    };
    let w = Watcher::start(&r, &opts).unwrap();
    // Far more events than 4 KB holds, delivered while the reader sleeps.
    for i in 0..3000 {
        fs::write(r.join(format!("burst{i}.txt")), b"x").unwrap();
    }
    let c = collect_until(&w, |c| c.root_dirty && c.overflows > 0);
    assert!(c.root_dirty, "{c:?}");
    assert!(
        c.overflows > 0,
        "the kernel buffer must have overflowed: {c:?}"
    );
}

#[test]
fn draining_clears_and_stopping_is_clean() {
    let (_g, r) = root();
    let mut w = Watcher::start(&r, &WatchOptions::default()).unwrap();
    fs::write(r.join("x.txt"), b"x").unwrap();
    let c = collect_until(&w, |c| has(c, &r));
    assert!(has(&c, &r));
    std::thread::sleep(Duration::from_millis(200));
    let _ = w.drain();
    assert!(
        w.drain().is_empty(),
        "a second drain with no activity is empty"
    );
    w.stop();
    w.stop(); // idempotent
    // The folder can be deleted after the watch stopped (and while it ran, see below).
    drop(w);
}

#[test]
fn watching_does_not_block_deleting_the_folder() {
    let (g, r) = root();
    let sub = r.join("sub");
    fs::create_dir(&sub).unwrap();
    let w = Watcher::start(&sub, &WatchOptions::default()).unwrap();
    fs::remove_dir(&sub).expect("full sharing lets the watched folder be deleted");
    drop(w);
    drop(g);
}

#[test]
fn refuses_relative_and_missing_folders() {
    assert!(Watcher::start(Path::new("relative"), &WatchOptions::default()).is_err());
    assert!(
        Watcher::start(
            Path::new(r"Z:\definitely\not\here"),
            &WatchOptions::default()
        )
        .is_err()
    );
}
