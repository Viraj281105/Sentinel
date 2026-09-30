#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use sentinel_classify::{Anchor, Category, Classifier};
use sentinel_scanner::scan::{DirNode, NodeStatus, ScanStats, ScanTree};

fn anchors() -> HashMap<Anchor, PathBuf> {
    let u = PathBuf::from(r"C:\Users\dev");
    HashMap::from([
        (Anchor::Windows, PathBuf::from(r"C:\Windows")),
        (Anchor::ProgramFiles, PathBuf::from(r"C:\Program Files")),
        (
            Anchor::ProgramFilesX86,
            PathBuf::from(r"C:\Program Files (x86)"),
        ),
        (Anchor::ProgramData, PathBuf::from(r"C:\ProgramData")),
        (Anchor::LocalAppData, u.join(r"AppData\Local")),
        (Anchor::RoamingAppData, u.join(r"AppData\Roaming")),
        (Anchor::CargoHome, PathBuf::from(r"D:\Tools\Rust\cargo")),
        (Anchor::RustupHome, u.join(".rustup")),
        (Anchor::Profile, u),
    ])
}

fn cat(c: &Classifier, p: &str) -> Category {
    c.classify(Path::new(p))
        .map_or(Category::Unknown, |x| x.category)
}

#[test]
fn known_locations_map_to_their_categories() {
    let c = Classifier::new(&anchors());
    use Category::*;
    for (path, want) in [
        (r"C:\Windows\System32", Windows),
        (r"C:\Windows\Temp\x.tmp", TemporaryFiles),
        (
            r"C:\Windows\SoftwareDistribution\Download\abc",
            TemporaryFiles,
        ),
        (r"C:\Windows\SoftwareDistribution\DataStore", Windows),
        (r"C:\Program Files\App", Applications),
        (
            r"C:\Program Files\Microsoft Visual Studio\18",
            DeveloperTools,
        ),
        (r"C:\Program Files\Docker\Docker", Containers),
        (r"C:\Program Files (x86)\Steam\steamapps", Games),
        (r"C:\Users\dev\AppData\Local\Temp\a", TemporaryFiles),
        (r"C:\Users\dev\.vscode\extensions\ms-python", DeveloperTools),
        (
            r"C:\Users\dev\AppData\Local\npm-cache\_cacache",
            PackageCaches,
        ),
        (r"C:\Users\dev\AppData\Local\pnpm\store\v10", PackageCaches),
        (r"C:\Users\dev\AppData\Local\pnpm\bin", Applications),
        (r"C:\Users\dev\AppData\Local\pip\cache\http", PackageCaches),
        (r"C:\Users\dev\.m2\repository\org", PackageCaches),
        (r"C:\Users\dev\.gradle\caches\8.0", PackageCaches),
        (
            r"C:\Users\dev\.gradle\wrapper\dists\gradle-8",
            DeveloperTools,
        ),
        (r"D:\Tools\Rust\cargo\registry\cache", PackageCaches),
        (r"D:\Tools\Rust\cargo\bin", DeveloperTools),
        (r"C:\Users\dev\.rustup\toolchains", DeveloperTools),
        (
            r"C:\Users\dev\AppData\Local\Google\Chrome\User Data",
            Browsers,
        ),
        (
            r"C:\Users\dev\AppData\Local\Docker\wsl\disk\docker_data.vhdx",
            Containers,
        ),
        (r"C:\Users\dev\AppData\Local\wsl\{guid}\ext4.vhdx", Wsl),
        (r"C:\Users\dev\AppData\Local\CrashDumps\app.dmp", Logs),
        (r"C:\Users\dev\Documents\notes.txt", UserData),
        (r"C:\Users\dev\Downloads", UserData),
        (r"C:\$Recycle.Bin\S-1-5-21", TemporaryFiles),
        (r"C:\pagefile.sys", Windows),
        (r"C:\Python314\Lib", DeveloperTools),
        (r"C:\Riot Games\League", Games),
    ] {
        assert_eq!(cat(&c, path), want, "{path}");
    }
}

#[test]
fn unknown_stays_unknown() {
    let c = Classifier::new(&anchors());
    for p in [
        r"C:\Users\dev",
        r"C:\Users\dev\projects\app",
        r"C:\Users\dev\projects\app\target",
        r"C:\Users\dev\projects\app\dist",
        r"C:\SomeFolder",
        r"C:\Python",                      // no version digits
        r"C:\Python3x",                    // not a plain version
        r"C:\data\pagefile.sys",           // only at a drive root
        r"E:\Riot Games\inner\Riot Games", // root rule applies to the top folder only
    ] {
        let got = c.classify(Path::new(p));
        if p.starts_with(r"E:\Riot Games") {
            assert_eq!(got.map(|g| g.rule), Some("root.riot"), "{p}");
        } else {
            assert!(got.is_none(), "{p} -> {got:?}");
        }
    }
}

#[test]
fn name_rules_apply_anywhere_and_deepest_match_wins() {
    let c = Classifier::new(&anchors());
    let m = c
        .classify(Path::new(r"C:\Users\dev\Documents\app\node_modules\react"))
        .unwrap();
    assert_eq!(m.category, Category::DeveloperDependencies);
    assert_eq!(m.rule, "name.node-modules");
    assert_eq!(
        cat(&c, r"D:\Projects\api\.venv\Lib\site-packages"),
        Category::DeveloperDependencies
    );
    assert_eq!(
        cat(&c, r"D:\Projects\api\src\__pycache__"),
        Category::BuildArtifacts
    );
    // A location rule deeper than a name rule wins.
    assert_eq!(
        cat(&c, r"C:\Users\dev\AppData\Roaming\npm\node_modules\x"),
        Category::DeveloperDependencies
    );
    // Case-insensitive, verbatim prefixes tolerated.
    assert_eq!(cat(&c, r"\\?\c:\WINDOWS\temp"), Category::TemporaryFiles);
}

#[test]
fn every_classification_explains_itself() {
    let c = Classifier::new(&anchors());
    let m = c
        .classify(Path::new(r"C:\Users\dev\AppData\Local\npm-cache"))
        .unwrap();
    assert_eq!(m.rule, "npm.cache");
    assert!(m.reason.contains("npm"));
}

fn node(name: &str, parent: Option<u32>, own: u64) -> DirNode {
    DirNode {
        name: name.into(),
        parent,
        children: Vec::new(),
        own_bytes: own,
        total_bytes: 0,
        total_logical_bytes: 0,
        file_count: 0,
        dir_count: 0,
        status: NodeStatus::Complete,
    }
}

#[test]
fn breakdown_attributes_own_bytes_and_sums_to_total() {
    let c = Classifier::new(&anchors());
    // C:\ (10) > Windows (100) > Temp (5); Users (0) > dev (7) > AppData (0) > Local (3)
    //   > npm-cache (50); proj (20) > node_modules (30)
    let nodes = vec![
        node(r"C:\", None, 10),
        node("Windows", Some(0), 100),
        node("Temp", Some(1), 5),
        node("Users", Some(0), 0),
        node("dev", Some(3), 7),
        node("AppData", Some(4), 0),
        node("Local", Some(5), 3),
        node("npm-cache", Some(6), 50),
        node("proj", Some(0), 20),
        node("node_modules", Some(8), 30),
    ];
    let tree = ScanTree {
        root: PathBuf::from(r"C:\"),
        nodes,
        largest_files: Vec::new(),
        stats: ScanStats::default(),
    };
    let b = c.breakdown(&tree);
    let get = |cat: Category| b.iter().find(|x| x.category == cat).map_or(0, |x| x.bytes);
    assert_eq!(get(Category::Windows), 100);
    assert_eq!(get(Category::TemporaryFiles), 5);
    assert_eq!(get(Category::PackageCaches), 50);
    assert_eq!(get(Category::Applications), 3);
    assert_eq!(get(Category::DeveloperDependencies), 30);
    assert_eq!(get(Category::Unknown), 10 + 7 + 20);
    assert_eq!(b.iter().map(|x| x.bytes).sum::<u64>(), 225);
    assert_eq!(b[0].category, Category::Windows, "largest first");
}

#[test]
fn system_anchors_resolve_on_this_machine() {
    let a = sentinel_classify::system_anchors();
    for k in [
        Anchor::Windows,
        Anchor::ProgramFiles,
        Anchor::Profile,
        Anchor::LocalAppData,
        Anchor::CargoHome,
    ] {
        assert!(a.contains_key(&k), "{k:?} missing");
    }
    let c = Classifier::for_system();
    assert_eq!(cat(&c, r"C:\Windows\System32"), Category::Windows);
}

#[test]
fn large_files_are_moved_to_their_own_category() {
    use sentinel_scanner::scan::LargeFile;
    let c = Classifier::new(&anchors());
    let mut root = node(r"C:\", None, 100);
    root.children = vec![1];
    let tree = ScanTree {
        root: PathBuf::from(r"C:\"),
        nodes: vec![root, node("Windows", Some(0), 10)],
        largest_files: vec![
            LargeFile {
                path: r"C:\pagefile.sys".into(),
                bytes: 60,
                logical_bytes: 60,
            },
            // Same category as its folder: nothing moves.
            LargeFile {
                path: r"C:\Windows\big.dat".into(),
                bytes: 8,
                logical_bytes: 8,
            },
        ],
        stats: ScanStats::default(),
    };
    let b = c.breakdown(&tree);
    let get = |cat: Category| b.iter().find(|x| x.category == cat).map_or(0, |x| x.bytes);
    assert_eq!(get(Category::Windows), 70);
    assert_eq!(get(Category::Unknown), 40);
}
