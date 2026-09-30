//! Git index parsing against real repositories created with the git CLI in temporary
//! folders (tests only; Sentinel itself never runs git).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::Path;
use std::process::Command;

use sentinel_devenv::tracks_anything_under;

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "core.autocrlf=false",
        ])
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git is installed");
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

fn put(p: &Path) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, b"x").unwrap();
}

fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    git(r, &["init", "-q"]);
    put(&r.join("src").join("main.rs"));
    put(&r.join("bin").join("tool.exe"));
    put(&r.join("web").join("package.json"));
    put(&r.join("web").join("dist").join("app.js"));
    put(&r
        .join("web")
        .join("node_modules")
        .join("react")
        .join("index.js"));
    put(&r.join("target").join("debug").join("x"));
    git(r, &["add", "src", "bin", "web/package.json", "web/dist"]);
    git(r, &["commit", "-q", "-m", "init"]);
    dir
}

fn check_all(r: &Path) {
    assert!(
        tracks_anything_under(&r.join("bin")).unwrap(),
        "committed build output"
    );
    assert!(
        tracks_anything_under(&r.join("web").join("dist")).unwrap(),
        "nested tracked"
    );
    assert!(
        !tracks_anything_under(&r.join("target")).unwrap(),
        "untracked"
    );
    assert!(!tracks_anything_under(&r.join("web").join("node_modules")).unwrap());
    assert!(
        !tracks_anything_under(&r.join("Bi")).unwrap(),
        "no prefix false positive"
    );
    // Case-insensitive, as on Windows.
    assert!(tracks_anything_under(&r.join("BIN")).unwrap());
}

#[test]
fn detects_tracked_and_untracked_folders_in_index_v2() {
    let dir = repo();
    check_all(dir.path());
}

#[test]
fn understands_index_v3_and_v4() {
    for v in ["3", "4"] {
        let dir = repo();
        git(dir.path(), &["update-index", "--index-version", v]);
        check_all(dir.path());
    }
}

#[test]
fn understands_worktrees() {
    let dir = repo();
    let wt = dir.path().parent().unwrap().join(format!(
        "{}-wt",
        dir.path().file_name().unwrap().to_string_lossy()
    ));
    git(
        dir.path(),
        &["worktree", "add", "-q", "--detach", wt.to_str().unwrap()],
    );
    assert!(fs::symlink_metadata(wt.join(".git")).unwrap().is_file());
    assert!(tracks_anything_under(&wt.join("bin")).unwrap());
    assert!(!tracks_anything_under(&wt.join("target")).unwrap());
    git(
        dir.path(),
        &["worktree", "remove", "--force", wt.to_str().unwrap()],
    );
}

#[test]
fn outside_any_repository_nothing_is_tracked() {
    let dir = tempfile::tempdir().unwrap();
    put(&dir.path().join("node_modules").join("x"));
    assert!(!tracks_anything_under(&dir.path().join("node_modules")).unwrap());
}

#[test]
fn a_split_index_is_reported_as_unknown() {
    let dir = repo();
    git(dir.path(), &["update-index", "--split-index"]);
    assert!(tracks_anything_under(&dir.path().join("target")).is_err());
}
