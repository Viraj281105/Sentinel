#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Randomized (seeded, reproducible) tests of path handling: Phase 12's "fuzzing of path
//! handling". There is no external fuzzer and no new dependency: a small deterministic
//! generator produces hostile input, and every check is either "never panics" or a property
//! stated independently of the code under test (an oracle written separately here).
//!
//! A failure prints the seed-derived input so it can be turned into a plain regression test.
//! Everything that touches the disk happens inside a temporary directory and only reads.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sentinel_safety::{
    CanonicalPath, Policy, ProtectedSet, TargetKind, is_reparse_point, is_within,
};

/// SplitMix64: tiny, well distributed, and fully determined by the seed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn chance(&mut self, one_in: usize) -> bool {
        self.below(one_in) == 0
    }
    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
    /// Random upper/lower casing of each letter.
    fn recase(&mut self, s: &str) -> String {
        s.chars()
            .map(|c| {
                if self.chance(2) {
                    c.to_uppercase().next().unwrap_or(c)
                } else {
                    c.to_lowercase().next().unwrap_or(c)
                }
            })
            .collect()
    }
}

// ---- A. The protected-path rules against an independent oracle --------------------

const SEGMENTS: &[&str] = &[
    "windows",
    "temp",
    "minidump",
    "system32",
    "users",
    "u",
    "documents",
    "documents2",
    "docs",
    "vault",
    "x",
    "readme",
    "program files",
    ".git",
    ".ssh",
    ".aws",
    "id_rsa",
    ".env",
    ".env.local",
    ".environment",
    "env",
    "a.vhdx",
    ".vhdx",
    "b.kdbx",
    "c.pem",
    "d.txt",
    "e.tar.pem",
    "f.pem.txt",
    "g.",
    "$recycle.bin",
    ".sentinel-quarantine",
];

/// The rules, restated for the test (not shared with the code under test).
const NAMES: &[&str] = &[
    ".git",
    ".sentinel-quarantine",
    ".ssh",
    ".gnupg",
    ".aws",
    ".azure",
    ".kube",
    ".npmrc",
    ".pypirc",
    ".netrc",
    "id_rsa",
    "id_ecdsa",
    "id_ed25519",
];
const EXTS: &[&str] = &["vhdx", "vhd", "kdbx", "pfx", "p12", "ppk", "pem"];

struct Spec {
    drive: char,
    segs: Vec<&'static str>,
}

fn roots() -> (Vec<Spec>, Vec<Spec>) {
    // (protected roots, exceptions inside the first one that has any)
    (
        vec![
            Spec {
                drive: 'c',
                segs: vec!["users", "u", "documents"],
            },
            Spec {
                drive: 'c',
                segs: vec!["windows"],
            },
            Spec {
                drive: 'd',
                segs: vec!["vault"],
            },
        ],
        vec![
            Spec {
                drive: 'c',
                segs: vec!["windows", "temp"],
            },
            Spec {
                drive: 'c',
                segs: vec!["windows", "minidump"],
            },
        ],
    )
}

fn prefix_of(path: &(char, Vec<String>), root: &Spec) -> bool {
    path.0 == root.drive
        && path.1.len() >= root.segs.len()
        && root.segs.iter().zip(&path.1).all(|(a, b)| a == b)
}

fn blocked_by_oracle(drive: char, segs: &[String]) -> bool {
    let path = (drive, segs.to_vec());
    if segs
        .iter()
        .any(|s| NAMES.contains(&s.as_str()) || s == ".env" || s.starts_with(".env."))
    {
        return true;
    }
    if let Some(last) = segs.last()
        && let Some(dot) = last.rfind('.')
        && dot > 0
        && EXTS.contains(&&last[dot + 1..])
    {
        return true;
    }
    let (protected, exceptions) = roots();
    for root in &protected {
        if prefix_of(&path, root) {
            let excepted = exceptions.iter().any(|e| prefix_of(&path, e));
            if !excepted {
                return true;
            }
            continue;
        }
        // The path is an ancestor of a protected root (or the drive root itself).
        let ancestor = path.0 == root.drive
            && segs.len() < root.segs.len()
            && segs.iter().zip(&root.segs).all(|(a, b)| a == b);
        if ancestor {
            return true;
        }
    }
    false
}

fn protected_set() -> ProtectedSet {
    let (protected, exceptions) = roots();
    let render = |s: &Spec| {
        let mut p = PathBuf::from(format!("{}:\\", s.drive.to_ascii_uppercase()));
        for seg in &s.segs {
            p.push(seg);
        }
        p
    };
    let mut set = ProtectedSet::new();
    set.add_root(&render(&protected[0]), "documents");
    let ex: Vec<PathBuf> = exceptions.iter().map(render).collect();
    set.add_root_with_exceptions(&render(&protected[1]), "system", &ex);
    set.add_root(&render(&protected[2]), "vault");
    set
}

#[test]
fn the_protected_rules_match_an_independent_oracle_on_200k_random_paths() {
    let policy = Policy::new(protected_set());
    let mut rng = Rng(0x5EC0_4E71);
    let mut blocked = 0u32;
    for i in 0..200_000u32 {
        let drive = *rng.pick(&['c', 'd', 'e']);
        let n = rng.below(7);
        let segs: Vec<String> = (0..n)
            .map(|_| {
                // Every protected name and extension must come up, not just a few of them.
                match rng.below(8) {
                    0 => (*rng.pick(NAMES)).to_owned(),
                    1 => format!("f.{}", rng.pick(EXTS)),
                    _ => (*rng.pick(SEGMENTS)).to_owned(),
                }
            })
            .collect();
        // Spell it with random case, separators, doubled and trailing separators.
        let mut text = format!(
            "{}:",
            if rng.chance(2) {
                drive.to_ascii_uppercase()
            } else {
                drive
            }
        );
        let sep = |rng: &mut Rng| if rng.chance(3) { "/" } else { "\\" };
        text.push_str(sep(&mut rng));
        for (k, s) in segs.iter().enumerate() {
            if k > 0 {
                text.push_str(sep(&mut rng));
                if rng.chance(8) {
                    text.push_str(sep(&mut rng));
                }
            }
            text.push_str(&rng.recase(s));
        }
        if !segs.is_empty() && rng.chance(6) {
            text.push_str(sep(&mut rng));
        }
        let expected = blocked_by_oracle(drive, &segs);
        let got = policy.check_protected(Path::new(&text)).is_err();
        blocked += u32::from(got);
        assert_eq!(
            got, expected,
            "iteration {i}: {text:?} (segments {segs:?}): code says blocked={got}, oracle says {expected}"
        );
    }
    assert!(
        blocked > 20_000,
        "the generator must exercise both outcomes ({blocked})"
    );
}

#[test]
fn within_agrees_with_a_component_oracle_and_is_never_a_string_prefix_test() {
    let mut rng = Rng(0xC0FF_EE11);
    let alphabet = ["a", "ab", "abc", "b", "A", "Ab", "x y", "x"];
    for i in 0..100_000u32 {
        let base: Vec<&str> = (0..rng.below(4)).map(|_| *rng.pick(&alphabet)).collect();
        let mut path = base.clone();
        let mut root = base.clone();
        // Relate them: equal, extended, truncated, or independently different.
        match rng.below(4) {
            0 => {}
            1 => (0..=rng.below(3)).for_each(|_| path.push(rng.pick(&alphabet))),
            2 => (0..=rng.below(3)).for_each(|_| root.push(rng.pick(&alphabet))),
            _ => path = (0..rng.below(4)).map(|_| *rng.pick(&alphabet)).collect(),
        }
        let render = |v: &[&str]| format!("C:\\{}", v.join("\\"));
        let expected = root.len() <= path.len()
            && root
                .iter()
                .zip(&path)
                .all(|(r, p)| r.eq_ignore_ascii_case(p));
        let (p, r) = (render(&path), render(&root));
        assert_eq!(
            is_within(Path::new(&p), Path::new(&r)),
            expected,
            "iteration {i}: {p:?} within {r:?}"
        );
    }
    // The classic mistake, stated outright.
    assert!(!is_within(
        Path::new(r"C:\Users\ab"),
        Path::new(r"C:\Users\a")
    ));
    assert!(!is_within(
        Path::new(r"C:\Users\a b"),
        Path::new(r"C:\Users\a")
    ));
}

// ---- B. Nothing hostile panics ---------------------------------------------------

fn hostile_string(rng: &mut Rng) -> String {
    const PIECES: &[&str] = &[
        "C:",
        "c:",
        "D:",
        "\\",
        "/",
        "\\\\",
        "\\\\?\\",
        "\\\\.\\",
        "\\\\?\\UNC\\",
        "\\\\?\\GLOBALROOT\\",
        "\\\\server\\share",
        "..",
        ".",
        "...",
        "CON",
        "nul",
        "aux.txt",
        "COM1",
        "lpt9.log",
        "a:b",
        "x ",
        "x.",
        "~1",
        "PROGRA~1",
        "*",
        "?",
        "<",
        ">",
        "|",
        "\"",
        "\u{0}",
        "\u{202e}",
        "\u{feff}",
        "é",
        "ß",
        "İ",
        "ǅ",
        "名前",
        ".git",
        ".GIT",
        ".env",
        ".ssh",
        "id_rsa",
        "a.vhdx",
        "$MFT",
        "$Recycle.Bin",
        "System Volume Information",
        "::$DATA",
        ":stream",
        "\t",
        "\n",
    ];
    let mut s = String::new();
    for _ in 0..=rng.below(10) {
        match rng.below(10) {
            0 => s.push(char::from_u32(rng.below(0x11_0000) as u32).unwrap_or('?')),
            1 => s.push_str(&"a".repeat(rng.below(400))),
            _ => s.push_str(rng.pick(PIECES)),
        }
    }
    s
}

#[test]
fn arbitrary_hostile_text_never_panics_and_never_yields_a_target() {
    let dir = tempfile::tempdir().unwrap();
    let base = CanonicalPath::resolve(dir.path())
        .unwrap()
        .as_path()
        .to_path_buf();
    let root_dir = base.join("root");
    fs::create_dir(&root_dir).unwrap();
    fs::write(root_dir.join("real.tmp"), b"x").unwrap();
    let policy = Policy::new(protected_set());
    let root = policy.allowed_root(&root_dir).unwrap();
    let mut rng = Rng(0xBAD5_EED5);
    for i in 0..30_000u32 {
        let text = hostile_string(&mut rng);
        let p = Path::new(&text);
        let _ = is_within(p, root.path());
        let _ = policy.check_protected(p);
        if let Ok(t) = policy.validate(&root, p) {
            // Garbage text cannot be an existing child of the root, except by naming it.
            assert!(
                is_within(t.path(), root.path()),
                "iteration {i}: {text:?} validated outside the root: {:?}",
                t.path()
            );
        }
        let _ = CanonicalPath::resolve(p);
        let _ = policy.allowed_root(p);
    }
}

// ---- C. validate() is sound on mutated real paths --------------------------------

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

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        out.push(p.clone());
        // Do not descend into the junction: it is exercised by name mutations instead.
        if p.is_dir() && !is_reparse_point(&p).unwrap() {
            walk(&p, out);
        }
    }
}

fn mutate(rng: &mut Rng, original: &str, root: &str, outside: &str) -> String {
    let mut s = original.to_owned();
    for _ in 0..=rng.below(3) {
        match rng.below(14) {
            0 => s = s.replace('\\', "/"),
            1 => s = s.replace('\\', "\\\\"),
            2 => s.push('.'),
            3 => s.push(' '),
            4 => s.push_str(":stream"),
            5 => s.push_str("::$DATA"),
            6 => s.push('\\'),
            7 => s.push_str("\\.."),
            8 => s.push_str("\\..\\..\\outside\\victim.txt"),
            9 => s = rng.recase(&s),
            10 => s = format!("\\\\?\\{s}"),
            11 => s = s.replacen(root, &format!("{root}\\link\\.."), 1),
            12 => s = s.replacen(root, &format!("{root}\\link"), 1),
            _ => {
                // A hop that returns, or a short-name spelling of one component.
                let parts: Vec<&str> = s.split('\\').collect();
                if parts.len() > 3 {
                    let k = 2 + rng.below(parts.len() - 2);
                    let mut v: Vec<String> = parts.iter().map(|p| (*p).to_owned()).collect();
                    v[k] = if rng.chance(2) {
                        "..".into()
                    } else {
                        format!("{}~1", &parts[k][..parts[k].len().min(6)])
                    };
                    s = v.join("\\");
                }
            }
        }
    }
    if rng.chance(15) {
        s = s.replacen(root, outside, 1);
    }
    s
}

#[test]
fn validate_only_ever_accepts_real_unlinked_unprotected_children_of_the_root() {
    let dir = tempfile::tempdir().unwrap();
    let base = CanonicalPath::resolve(dir.path())
        .unwrap()
        .as_path()
        .to_path_buf();
    let root_dir = base.join("root");
    let outside = base.join("outside");
    for d in [
        root_dir.join("Sub Dir"),
        root_dir.join("deep").join("x").join("y"),
        root_dir.join(".git"),
        root_dir.join("vault"),
        outside.clone(),
    ] {
        fs::create_dir_all(d).unwrap();
    }
    for f in [
        root_dir.join("a.tmp"),
        root_dir.join("Sub Dir").join("inner.txt"),
        root_dir.join("deep").join("x").join("y").join("z.bin"),
        root_dir.join(".git").join("HEAD"),
        root_dir.join("keep.vhdx"),
        root_dir.join("vault").join("secret.txt"),
        outside.join("victim.txt"),
    ] {
        fs::write(f, b"data").unwrap();
    }
    junction(&root_dir.join("link"), &outside);

    let mut set = ProtectedSet::new();
    set.add_root(&root_dir.join("vault"), "vault");
    let policy = Policy::new(set);
    let root = policy.allowed_root(&root_dir).unwrap();

    let mut real = Vec::new();
    walk(&root_dir, &mut real);
    real.push(outside.join("victim.txt"));
    // Reaching the outside folder's file through the junction inside the root.
    real.push(root_dir.join("link").join("victim.txt"));
    real.push(outside.clone());
    let real: Vec<String> = real.iter().map(|p| p.display().to_string()).collect();
    let (root_s, outside_s) = (
        root_dir.display().to_string(),
        outside.display().to_string(),
    );

    let mut rng = Rng(0x7E57_AB1E);
    let (mut accepted, mut rejected) = (0u32, 0u32);
    for i in 0..6_000u32 {
        let original = rng.pick(&real).clone();
        let candidate = if rng.chance(5) {
            original.clone()
        } else {
            mutate(&mut rng, &original, &root_s, &outside_s)
        };
        let Ok(t) = policy.validate(&root, Path::new(&candidate)) else {
            rejected += 1;
            continue;
        };
        accepted += 1;
        let at = t.path();
        let ctx = format!("iteration {i}: {candidate:?} (from {original:?}) -> {at:?}");
        assert!(
            is_within(at, root.path()) && at != root.path(),
            "outside or the root itself: {ctx}"
        );
        assert!(
            !is_within(at, &outside),
            "reached the outside folder: {ctx}"
        );
        assert!(
            at.exists() || fs::symlink_metadata(at).is_ok(),
            "does not exist: {ctx}"
        );
        assert!(policy.check_protected(at).is_ok(), "protected: {ctx}");
        let text = at.display().to_string();
        assert!(
            !text.contains("..") && !text.contains('/') && !text.contains(':')
                || text.matches(':').count() == 1,
            "odd spelling: {ctx}"
        );
        // No component between the root and the target may be a link; only the target itself.
        let mut up = at.parent();
        while let Some(p) = up {
            if !is_within(p, root.path()) || p == root.path() {
                break;
            }
            assert!(
                !is_reparse_point(p).unwrap(),
                "linked ancestor {p:?}: {ctx}"
            );
            up = p.parent();
        }
        if t.kind() == TargetKind::Link {
            assert!(is_reparse_point(at).unwrap(), "{ctx}");
        } else {
            assert!(!is_reparse_point(at).unwrap(), "{ctx}");
        }
        t.revalidate(&policy)
            .unwrap_or_else(|e| panic!("revalidate failed ({e}): {ctx}"));
    }
    assert!(
        accepted > 100,
        "unmutated paths must be accepted ({accepted})"
    );
    assert!(
        rejected > 1_000,
        "hostile mutations must be refused ({rejected})"
    );
}
