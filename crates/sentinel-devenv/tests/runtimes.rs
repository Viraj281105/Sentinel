//! Runtime inventory on synthetic install layouts.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};

use sentinel_devenv::{
    InstalledRuntime, Locations, Project, Runtime, RuntimeRequirement, inventory_runtimes,
};

fn put(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn project(path: &str, reqs: &[(Runtime, &str, &str)]) -> Project {
    Project {
        path: path.to_owned(),
        name: path.to_owned(),
        ecosystems: vec![],
        package_managers: vec![],
        markers: vec![],
        runtimes: reqs
            .iter()
            .map(|(runtime, version, source)| RuntimeRequirement {
                runtime: *runtime,
                version: (*version).to_owned(),
                source: (*source).to_owned(),
            })
            .collect(),
        git: false,
        last_activity_ms: None,
        total_bytes: None,
        artifacts: vec![],
        warnings: vec![],
    }
}

struct Machine {
    _dir: tempfile::TempDir,
    root: PathBuf,
    loc: Locations,
}

fn machine() -> Machine {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let loc = Locations {
        profile: Some(root.join("u")),
        local_app_data: Some(root.join("u").join("Local")),
        app_data: Some(root.join("u").join("Roaming")),
        program_files: vec![root.join("pf")],
        ..Locations::default()
    };
    Machine {
        _dir: dir,
        root,
        loc,
    }
}

fn find<'a>(inv: &'a [InstalledRuntime], runtime: Runtime, version: &str) -> &'a InstalledRuntime {
    inv.iter()
        .find(|i| i.runtime == runtime && i.version == version)
        .unwrap_or_else(|| panic!("{runtime:?} {version} not found in {inv:#?}"))
}

#[test]
fn finds_installs_from_their_own_files() {
    let m = machine();
    let u = m.root.join("u");
    let pf = m.root.join("pf");

    // Node: nvm-windows and fnm layouts.
    put(&u.join("Roaming/nvm/v22.4.0/node.exe"), "x");
    put(&u.join("Roaming/nvm/v18.19.1/node.exe"), "x");
    put(
        &u.join("Roaming/nvm/v99.0.0/readme.txt"),
        "no node.exe: ignored",
    );
    put(
        &u.join("Local/fnm/node-versions/v20.11.0/installation/node.exe"),
        "x",
    );

    // Python: exact version from patchlevel.h, folder name as fallback, pyenv-win, uv.
    let py = u.join("Local/Programs/Python/Python312");
    put(&py.join("python.exe"), "x");
    put(
        &py.join("include/patchlevel.h"),
        "#define PY_MAJOR_VERSION 3\n#define PY_VERSION \"3.12.4\"\n",
    );
    put(&u.join("Local/Programs/Python/Python311/python.exe"), "x");
    put(&u.join(".pyenv/pyenv-win/versions/3.10.11/python.exe"), "x");
    put(
        &u.join("Roaming/uv/python/cpython-3.13.1-windows-x86_64-none/python.exe"),
        "x",
    );

    // Java: a JDK, and a JRE without javac.
    let jdk = pf.join("Eclipse Adoptium/jdk-21.0.4.7-hotspot");
    put(&jdk.join("bin/java.exe"), "x");
    put(&jdk.join("bin/javac.exe"), "x");
    put(
        &jdk.join("release"),
        "IMPLEMENTOR=\"Eclipse Adoptium\"\nJAVA_VERSION=\"21.0.4\"\n",
    );
    let jre = pf.join("Java/jre1.8.0_402");
    put(&jre.join("bin/java.exe"), "x");
    put(&jre.join("release"), "JAVA_VERSION=\"1.8.0_402\"\n");
    put(&pf.join("Microsoft/not-a-jdk/readme.txt"), "ignored");

    // .NET.
    put(&pf.join("dotnet/sdk/8.0.404/dotnet.dll"), "x");
    put(
        &pf.join("dotnet/shared/Microsoft.NETCore.App/8.0.11/a.dll"),
        "x",
    );

    // Rust (version from the manifest, name as fallback) and Go.
    put(
        &u.join(".rustup/toolchains/stable-x86_64-pc-windows-msvc/lib/rustlib/multirust-channel-manifest.toml"),
        "[pkg.rustc]\nversion = \"1.98.1 (abc 2026-01-01)\"\n",
    );
    put(
        &u.join(".rustup/toolchains/1.85.0-x86_64-pc-windows-msvc/bin/rustc.exe"),
        "x",
    );
    put(&pf.join("Go/bin/go.exe"), "x");
    put(
        &pf.join("Go/VERSION"),
        "go1.23.1\ntime 2024-09-05T00:00:00Z\n",
    );

    // Android SDK.
    put(
        &u.join("Local/Android/Sdk/platforms/android-34/android.jar"),
        "x",
    );
    put(
        &u.join("Local/Android/Sdk/build-tools/34.0.0/aapt.exe"),
        "x",
    );
    put(
        &u.join("Local/Android/Sdk/ndk/26.1.10909125/ndk-build.cmd"),
        "x",
    );

    let inv = inventory_runtimes(&m.loc, &[]);
    let i = &inv.installed;

    for v in ["22.4.0", "18.19.1", "20.11.0"] {
        find(i, Runtime::Node, v);
    }
    assert_eq!(
        i.iter().filter(|x| x.runtime == Runtime::Node).count(),
        3,
        "folder without node.exe is not an install"
    );
    assert_eq!(find(i, Runtime::Node, "22.4.0").installed_by, "nvm-windows");
    assert_eq!(find(i, Runtime::Node, "20.11.0").installed_by, "fnm");

    assert_eq!(
        find(i, Runtime::Python, "3.12.4").installed_by,
        "python.org installer"
    );
    find(i, Runtime::Python, "3.12.4");
    find(i, Runtime::Python, "3.11");
    assert_eq!(
        find(i, Runtime::Python, "3.10.11").installed_by,
        "pyenv-win"
    );
    assert_eq!(find(i, Runtime::Python, "3.13.1").installed_by, "uv");

    assert_eq!(
        find(i, Runtime::Java, "21.0.4").label,
        "JDK (Eclipse Adoptium)"
    );
    assert_eq!(find(i, Runtime::Java, "1.8.0_402").label, "JRE");
    assert_eq!(i.iter().filter(|x| x.runtime == Runtime::Java).count(), 2);

    assert_eq!(find(i, Runtime::DotNet, "8.0.404").label, ".NET SDK");
    assert_eq!(find(i, Runtime::DotNet, "8.0.11").label, ".NET runtime");

    assert!(find(i, Runtime::Rust, "1.98.1").label.contains("stable"));
    find(i, Runtime::Rust, "1.85.0");
    find(i, Runtime::Go, "1.23.1");

    assert_eq!(find(i, Runtime::AndroidSdk, "34.0.0").label, "Build tools");
    find(i, Runtime::AndroidSdk, "android-34");
    find(i, Runtime::AndroidSdk, "26.1.10909125");

    assert!(find(i, Runtime::Go, "1.23.1").bytes.unwrap() > 0);
}

#[test]
fn newest_version_comes_first_within_a_runtime() {
    let m = machine();
    let u = m.root.join("u");
    for v in ["v9.0.0", "v22.4.0", "v18.19.1"] {
        put(&u.join(format!("Roaming/nvm/{v}/node.exe")), "x");
    }
    let inv = inventory_runtimes(&m.loc, &[]);
    let versions: Vec<&str> = inv.installed.iter().map(|i| i.version.as_str()).collect();
    assert_eq!(versions, ["22.4.0", "18.19.1", "9.0.0"]);
}

#[test]
fn relates_installs_to_project_requirements() {
    let m = machine();
    let u = m.root.join("u");
    put(&u.join("Roaming/nvm/v22.4.0/node.exe"), "x");
    put(&u.join("Roaming/nvm/v18.19.1/node.exe"), "x");
    put(&u.join(".pyenv/pyenv-win/versions/3.12.4/python.exe"), "x");
    put(&m.root.join("pf/Go/bin/go.exe"), "x");
    put(&m.root.join("pf/Go/VERSION"), "go1.23.1\n");

    let projects = vec![
        project(
            "D:\\p\\web",
            &[
                (Runtime::Node, ">=20", "package.json engines.node"),
                (Runtime::Node, "lts/*", ".nvmrc"),
            ],
        ),
        project("D:\\p\\old", &[(Runtime::Node, "16", ".nvmrc")]),
        project(
            "D:\\p\\api",
            &[(Runtime::Python, ">=3.13", "pyproject.toml requires-python")],
        ),
        project(
            "D:\\p\\svc",
            &[(Runtime::Go, "1.21", "go.mod go directive")],
        ),
        project(
            "D:\\p\\newgo",
            &[(Runtime::Go, "1.25", "go.mod go directive")],
        ),
        project(
            "D:\\p\\crab",
            &[(Runtime::Rust, "1.85", "Cargo.toml rust-version")],
        ),
        // The same project listed by two searches is compared once.
        project(
            "D:\\P\\WEB",
            &[(Runtime::Node, ">=99", "package.json engines.node")],
        ),
    ];
    let inv = inventory_runtimes(&m.loc, &projects);
    let i = &inv.installed;

    assert_eq!(inv.projects_read, 6);

    let n22 = find(i, Runtime::Node, "22.4.0");
    assert_eq!(n22.used_by.len(), 1);
    assert_eq!(n22.used_by[0].project, "D:\\p\\web");
    assert_eq!(n22.used_by[0].requirement, ">=20");
    assert!(
        find(i, Runtime::Node, "18.19.1").used_by.is_empty(),
        "lts/* is not understood, so it counts for nothing"
    );
    assert_eq!(
        find(i, Runtime::Go, "1.23.1").used_by[0].project,
        "D:\\p\\svc"
    );

    let unmet: Vec<(&str, Runtime)> = inv
        .unmet
        .iter()
        .map(|u| (u.project.as_str(), u.runtime))
        .collect();
    assert!(
        unmet.contains(&("D:\\p\\old", Runtime::Node)),
        "Node 16 is not installed"
    );
    assert!(
        unmet.contains(&("D:\\p\\api", Runtime::Python)),
        "3.12.4 < 3.13"
    );
    assert!(unmet.contains(&("D:\\p\\newgo", Runtime::Go)));
    assert!(
        unmet.contains(&("D:\\p\\crab", Runtime::Rust)),
        "nothing of that runtime is installed"
    );
    assert!(
        !unmet.iter().any(|(p, _)| *p == "D:\\p\\web"),
        "web is satisfied"
    );
    assert!(!unmet.iter().any(|(p, _)| *p == "D:\\p\\svc"));
    assert_eq!(inv.unmet.len(), 4);
}

#[test]
fn empty_machine_has_nothing() {
    let inv = inventory_runtimes(&Locations::default(), &[]);
    assert!(inv.installed.is_empty());
    assert!(inv.unmet.is_empty());
}
