#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Filesystem-sandbox tests for the policy engine. Everything happens inside
//! temporary directories created by the test; nothing outside them is modified.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sentinel_safety::{
    AllowedRoot, CanonicalPath, Policy, ProtectedSet, SafetyError, TargetKind, is_reparse_point,
};

struct Fixture {
    _dir: tempfile::TempDir,
    root: AllowedRoot,
    policy: Policy,
    base: PathBuf,
}

fn fixture_with(protect: impl FnOnce(&Path, &mut ProtectedSet)) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let base = CanonicalPath::resolve(dir.path())
        .unwrap()
        .as_path()
        .to_path_buf();
    let mut set = ProtectedSet::new();
    protect(&base, &mut set);
    let policy = Policy::new(set);
    let root_dir = base.join("root");
    fs::create_dir(&root_dir).unwrap();
    let root = policy.allowed_root(&root_dir).unwrap();
    Fixture {
        _dir: dir,
        root,
        policy,
        base,
    }
}

fn fixture() -> Fixture {
    fixture_with(|_, _| {})
}

fn junction(link: &Path, target: &Path) {
    let out = Command::new("cmd")
        .arg("/C")
        .arg("mklink")
        .arg("/J")
        .arg(link)
        .arg(target)
        .output()
        .unwrap();
    assert!(out.status.success(), "mklink /J failed: {out:?}");
}

#[test]
fn accepts_file_and_directory_inside_root() {
    let f = fixture();
    let root = f.root.path().to_path_buf();
    fs::write(root.join("a.tmp"), b"x").unwrap();
    fs::create_dir(root.join("d")).unwrap();

    let file = f.policy.validate(&f.root, &root.join("a.tmp")).unwrap();
    assert_eq!(file.kind(), TargetKind::File);
    let dir = f.policy.validate(&f.root, &root.join("d")).unwrap();
    assert_eq!(dir.kind(), TargetKind::Directory);
    file.revalidate(&f.policy).unwrap();
}

#[test]
fn rejects_root_itself_outside_and_prefix_siblings() {
    let f = fixture();
    let root = f.root.path().to_path_buf();
    fs::create_dir(f.base.join("root2")).unwrap();
    fs::write(f.base.join("outside.txt"), b"x").unwrap();

    assert!(matches!(
        f.policy.validate(&f.root, &root),
        Err(SafetyError::TargetIsRoot(_))
    ));
    assert!(matches!(
        f.policy.validate(&f.root, &f.base.join("outside.txt")),
        Err(SafetyError::OutsideRoot { .. })
    ));
    // "root2" shares a string prefix with "root" but is a different directory.
    assert!(matches!(
        f.policy.validate(&f.root, &f.base.join("root2")),
        Err(SafetyError::OutsideRoot { .. })
    ));
}

#[test]
fn rejects_traversal_streams_reserved_names_and_missing() {
    let f = fixture();
    let root = f.root.path().to_path_buf();
    fs::write(root.join("a.txt"), b"x").unwrap();

    for bad in [
        root.join("..").join("root").join("a.txt"),
        root.join("a.txt:hidden"),
        root.join("CON"),
        root.join("trailing."),
    ] {
        assert!(
            matches!(
                f.policy.validate(&f.root, &bad),
                Err(SafetyError::InvalidPath { .. })
            ),
            "{bad:?}"
        );
    }
    assert!(matches!(
        f.policy.validate(&f.root, &root.join("missing.txt")),
        Err(SafetyError::NotFound(_))
    ));
    assert!(matches!(
        f.policy.validate(&f.root, Path::new(r"relative\a.txt")),
        Err(SafetyError::InvalidPath { .. })
    ));
}

#[test]
fn case_variants_resolve_to_real_spelling() {
    let f = fixture();
    let root = f.root.path().to_path_buf();
    fs::create_dir(root.join("Sub")).unwrap();
    fs::write(root.join("Sub").join("File.TXT"), b"x").unwrap();

    let shouty = PathBuf::from(root.to_string_lossy().to_uppercase())
        .join("SUB")
        .join("file.txt");
    let t = f.policy.validate(&f.root, &shouty).unwrap();
    assert!(t.path().ends_with(r"Sub\File.TXT"), "{:?}", t.path());
}

#[test]
fn junction_ancestor_is_refused_but_link_itself_is_a_link_target() {
    let f = fixture();
    let root = f.root.path().to_path_buf();
    let outside = f.base.join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("victim.txt"), b"precious").unwrap();
    junction(&root.join("link"), &outside);
    assert!(is_reparse_point(&root.join("link")).unwrap());

    // Reaching through the junction must fail.
    let err = f
        .policy
        .validate(&f.root, &root.join("link").join("victim.txt"))
        .unwrap_err();
    assert!(matches!(err, SafetyError::NotCanonical(_)), "{err:?}");

    // The link itself is addressable, flagged as a link, and never followed.
    let t = f.policy.validate(&f.root, &root.join("link")).unwrap();
    assert_eq!(t.kind(), TargetKind::Link);
    assert!(outside.join("victim.txt").exists());
}

#[test]
fn symlink_ancestor_is_refused() {
    let f = fixture();
    let root = f.root.path().to_path_buf();
    let outside = f.base.join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("victim.txt"), b"x").unwrap();
    if std::os::windows::fs::symlink_dir(&outside, root.join("sym")).is_err() {
        eprintln!("SKIPPED: creating symlinks needs Developer Mode or elevation");
        return;
    }
    assert!(matches!(
        f.policy
            .validate(&f.root, &root.join("sym").join("victim.txt")),
        Err(SafetyError::NotCanonical(_))
    ));
    assert_eq!(
        f.policy
            .validate(&f.root, &root.join("sym"))
            .unwrap()
            .kind(),
        TargetKind::Link
    );
}

#[test]
fn protected_names_and_roots_are_enforced() {
    let f = fixture_with(|base, set| {
        set.add_root(
            &base.join("root").join("proj").join("keep"),
            "test-protected",
        );
    });
    let root = f.root.path().to_path_buf();
    fs::create_dir_all(root.join("repo").join(".git")).unwrap();
    fs::write(root.join(".env"), b"SECRET=1").unwrap();
    fs::create_dir_all(root.join("proj").join("keep")).unwrap();
    fs::write(root.join("proj").join("keep").join("f"), b"x").unwrap();

    for p in [
        root.join("repo").join(".git"),
        root.join(".env"),
        root.join("proj").join("keep"),
        root.join("proj").join("keep").join("f"),
    ] {
        assert!(
            matches!(
                f.policy.validate(&f.root, &p),
                Err(SafetyError::Protected { .. })
            ),
            "{p:?}"
        );
    }
    // A directory that would take a protected location down with it is refused too.
    assert!(matches!(
        f.policy.validate(&f.root, &root.join("proj")),
        Err(SafetyError::ContainsProtected { .. })
    ));
}

#[test]
fn allowed_root_cannot_be_protected() {
    let dir = tempfile::tempdir().unwrap();
    let base = CanonicalPath::resolve(dir.path()).unwrap();
    let mut set = ProtectedSet::new();
    set.add_root(base.as_path(), "test-protected");
    let policy = Policy::new(set);
    assert!(matches!(
        policy.allowed_root(base.as_path()),
        Err(SafetyError::Protected { .. })
    ));
    let file = base.as_path().join("f.txt");
    fs::write(&file, b"x").unwrap();
    let policy = Policy::new(ProtectedSet::new());
    assert!(matches!(
        policy.allowed_root(&file),
        Err(SafetyError::NotADirectory(_))
    ));
}

#[test]
fn revalidate_detects_object_swap() {
    let f = fixture();
    let root = f.root.path().to_path_buf();
    let target = root.join("t.txt");
    fs::write(&target, b"original").unwrap();
    let validated = f.policy.validate(&f.root, &target).unwrap();

    // Move the original aside (it keeps existing, so the new file gets a distinct
    // file index) and put a different object at the validated path.
    fs::rename(&target, root.join("moved.txt")).unwrap();
    fs::write(&target, b"impostor").unwrap();
    assert!(matches!(
        validated.revalidate(&f.policy),
        Err(SafetyError::IdentityChanged(_))
    ));
}

#[test]
fn revalidate_detects_file_replaced_by_junction() {
    let f = fixture();
    let root = f.root.path().to_path_buf();
    let outside = f.base.join("outside");
    fs::create_dir(&outside).unwrap();
    let target = root.join("d");
    fs::create_dir(&target).unwrap();
    let validated = f.policy.validate(&f.root, &target).unwrap();

    fs::remove_dir(&target).unwrap();
    junction(&target, &outside);
    assert!(matches!(
        validated.revalidate(&f.policy),
        Err(SafetyError::IdentityChanged(_))
    ));
    assert!(outside.exists());
}

#[test]
fn short_8dot3_alias_of_a_link_is_refused() {
    let f = fixture();
    let root = f.root.path().to_path_buf();
    let outside = f.base.join("outside");
    fs::create_dir(&outside).unwrap();
    junction(&root.join("a-long-junction-name"), &outside);
    assert!(matches!(
        f.policy.validate(&f.root, &root.join("A-LONG~1")),
        Err(SafetyError::NotFound(_) | SafetyError::NotCanonical(_))
    ));
}

#[test]
fn canonicalization_expands_short_names_when_enabled() {
    let Ok(c) = CanonicalPath::resolve(Path::new(r"C:\PROGRA~1")) else {
        eprintln!("SKIPPED: 8.3 names disabled on this volume");
        return;
    };
    assert!(c.as_path().ends_with("Program Files"), "{:?}", c.as_path());
}

// Read-only checks against the real machine's protection set. Nothing is modified.
#[test]
fn system_policy_protects_real_user_and_system_locations() {
    let policy = Policy::for_system();
    let mut checked = 0;
    if let Some(profile) = std::env::var_os("USERPROFILE").map(PathBuf::from) {
        for sub in ["Documents", "Desktop", "Downloads", ".ssh"] {
            let p = profile.join(sub);
            if p.is_dir() {
                assert!(
                    matches!(
                        policy.allowed_root(&p),
                        Err(SafetyError::Protected { .. } | SafetyError::ContainsProtected { .. })
                    ),
                    "{p:?} must be protected"
                );
                checked += 1;
            }
        }
        // A profile-wide root is tolerated, but the profile itself is never a target
        // because it contains protected folders.
        let root = policy.allowed_root(&profile.join("AppData")).unwrap();
        assert!(policy.validate(&root, &profile).is_err());
    }
    assert!(policy.allowed_root(Path::new(r"C:\Windows")).is_err());
    let c_root = policy.allowed_root(Path::new(r"C:\")).unwrap();
    assert!(matches!(
        policy.validate(&c_root, Path::new(r"C:\Windows")),
        Err(SafetyError::Protected { .. } | SafetyError::ContainsProtected { .. })
    ));
    assert!(
        policy.allowed_root(Path::new(r"C:\Windows\Temp")).is_ok(),
        "Windows\\Temp is an explicit exception"
    );
    eprintln!("checked {checked} user folders");
}

#[test]
fn open_verified_returns_a_handle_to_the_validated_object() {
    let f = fixture();
    let root = f.root.path().to_path_buf();
    let target = root.join("t.txt");
    fs::write(&target, b"original").unwrap();
    let validated = f.policy.validate(&f.root, &target).unwrap();
    let handle = validated.open_verified(&f.policy).unwrap();
    drop(handle);

    // Replace the object: opening must now fail.
    fs::rename(&target, root.join("moved.txt")).unwrap();
    fs::write(&target, b"impostor").unwrap();
    assert!(matches!(
        validated.open_verified(&f.policy),
        Err(SafetyError::IdentityChanged(_))
    ));
}

#[test]
fn open_verified_fails_while_another_process_blocks_deletion() {
    use std::os::windows::fs::OpenOptionsExt;
    let f = fixture();
    let target = f.root.path().join("busy.txt");
    fs::write(&target, b"x").unwrap();
    let validated = f.policy.validate(&f.root, &target).unwrap();
    // Share read/write but not delete, like many programs holding a file open.
    let _busy = fs::OpenOptions::new()
        .read(true)
        .share_mode(0x1 | 0x2)
        .open(&target)
        .unwrap();
    assert!(validated.open_verified(&f.policy).is_err());
}
