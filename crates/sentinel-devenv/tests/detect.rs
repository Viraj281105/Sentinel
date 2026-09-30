//! Detection tests on synthetic project fixtures.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::AtomicBool;

use sentinel_devenv::{
    ArtifactKind, DetectOptions, Detection, Ecosystem, PackageManager, Project, Runtime, detect,
};

fn put(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn run(root: &Path) -> Detection {
    detect(root, DetectOptions::default(), &AtomicBool::new(false)).unwrap()
}

fn project<'a>(d: &'a Detection, folder: &str) -> &'a Project {
    d.projects
        .iter()
        .find(|p| Path::new(&p.path).file_name().unwrap() == folder)
        .unwrap_or_else(|| panic!("{folder} not detected: {:#?}", d.projects))
}

#[test]
fn detects_a_node_project_with_lockfile_runtime_and_artifacts() {
    let dir = tempfile::tempdir().unwrap();
    let web = dir.path().join("web");
    put(
        &web.join("package.json"),
        r#"{ "name": "shop-web", "engines": { "node": ">=20" } }"#,
    );
    put(&web.join("pnpm-lock.yaml"), "lockfileVersion: 9");
    put(&web.join(".nvmrc"), "22.4.0\n");
    put(
        &web.join("node_modules").join("react").join("index.js"),
        &"x".repeat(5000),
    );
    put(&web.join(".next").join("cache").join("f"), "x");
    put(&web.join("src").join("app.ts"), "export {}");
    fs::create_dir(web.join(".git")).unwrap();
    put(&web.join(".git").join("HEAD"), "ref: refs/heads/main");

    let d = run(dir.path());
    let p = project(&d, "web");
    assert_eq!(p.name, "shop-web");
    assert_eq!(p.ecosystems, [Ecosystem::Node]);
    assert_eq!(p.package_managers, [PackageManager::Pnpm]);
    assert!(p.git);
    let rts: Vec<_> = p
        .runtimes
        .iter()
        .map(|r| (r.runtime, r.version.as_str()))
        .collect();
    assert!(rts.contains(&(Runtime::Node, ">=20")));
    assert!(rts.contains(&(Runtime::Node, "22.4.0")));
    let kinds: Vec<_> = p.artifacts.iter().map(|a| a.kind).collect();
    assert!(kinds.contains(&ArtifactKind::NodeModules));
    assert!(kinds.contains(&ArtifactKind::NextBuild));
    let nm = p
        .artifacts
        .iter()
        .find(|a| a.kind == ArtifactKind::NodeModules)
        .unwrap();
    assert!(nm.bytes.unwrap() >= 4096);
    assert!(p.total_bytes.unwrap() >= nm.bytes.unwrap());
    assert!(p.last_activity_ms.is_some());
    assert!(p.markers.iter().any(|m| m == "pnpm-lock.yaml"));
}

#[test]
fn detects_python_rust_go_java_dotnet_ruby_and_docker() {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    put(
        &r.join("api").join("pyproject.toml"),
        "[project]\nname = \"api\"\nrequires-python = \">=3.11\"\n",
    );
    put(&r.join("api").join("uv.lock"), "");
    put(&r.join("api").join(".venv").join("pyvenv.cfg"), "home = x");
    put(&r.join("api").join("venv").join("not-a-venv.txt"), "");
    put(&r.join("api").join(".pytest_cache").join("x"), "");

    put(
        &r.join("tool").join("Cargo.toml"),
        "[package]\nname = \"tool\"\nrust-version = \"1.85\"\n",
    );
    put(
        &r.join("tool").join("rust-toolchain.toml"),
        "[toolchain]\nchannel = \"1.90\"\n",
    );
    put(
        &r.join("tool").join("target").join("debug").join("tool.exe"),
        "x",
    );

    put(
        &r.join("svc").join("go.mod"),
        "module example.com/svc\n\ngo 1.23\n",
    );
    put(&r.join("jvm").join("build.gradle.kts"), "");
    put(&r.join("jvm").join("build").join("libs").join("a.jar"), "x");
    put(&r.join("maven").join("pom.xml"), "<project/>");
    put(&r.join("net").join("App.csproj"), "<Project/>");
    put(
        &r.join("net").join("bin").join("Debug").join("App.dll"),
        "x",
    );
    put(&r.join("gem").join("Gemfile"), "");
    put(&r.join("box").join("Dockerfile"), "FROM scratch");

    let d = run(r);
    let api = project(&d, "api");
    assert_eq!(api.ecosystems, [Ecosystem::Python]);
    assert_eq!(api.package_managers, [PackageManager::Uv]);
    assert_eq!(api.runtimes[0].version, ">=3.11");
    let venvs: Vec<_> = api
        .artifacts
        .iter()
        .filter(|a| a.kind == ArtifactKind::PythonVenv)
        .collect();
    assert_eq!(venvs.len(), 1, "only the folder with pyvenv.cfg is a venv");
    assert!(venvs[0].path.ends_with(".venv"));

    let tool = project(&d, "tool");
    assert_eq!(tool.package_managers, [PackageManager::Cargo]);
    let rust: Vec<_> = tool.runtimes.iter().map(|r| r.version.as_str()).collect();
    assert_eq!(rust.len(), 2);
    assert!(rust.contains(&"1.85") && rust.contains(&"1.90"));
    assert!(
        tool.artifacts
            .iter()
            .any(|a| a.kind == ArtifactKind::CargoTarget)
    );

    let svc = project(&d, "svc");
    assert_eq!(svc.name, "example.com/svc");
    assert_eq!(svc.runtimes[0].runtime, Runtime::Go);
    assert_eq!(
        project(&d, "jvm").package_managers,
        [PackageManager::Gradle]
    );
    assert!(
        project(&d, "jvm")
            .artifacts
            .iter()
            .any(|a| a.kind == ArtifactKind::GradleBuild)
    );
    assert_eq!(
        project(&d, "maven").package_managers,
        [PackageManager::Maven]
    );
    assert_eq!(project(&d, "net").ecosystems, [Ecosystem::DotNet]);
    assert!(
        project(&d, "net")
            .artifacts
            .iter()
            .any(|a| a.kind == ArtifactKind::DotNetBuild)
    );
    assert_eq!(project(&d, "gem").ecosystems, [Ecosystem::Ruby]);
    assert_eq!(project(&d, "box").ecosystems, [Ecosystem::Docker]);
}

#[test]
fn ambiguous_folders_are_not_artifacts_without_their_project_type() {
    let dir = tempfile::tempdir().unwrap();
    // A Node project with folders named like other ecosystems' build output.
    put(&dir.path().join("site").join("package.json"), "{}");
    put(
        &dir.path().join("site").join("target").join("keep.txt"),
        "x",
    );
    put(&dir.path().join("site").join("build").join("keep.txt"), "x");
    put(&dir.path().join("site").join("bin").join("keep.txt"), "x");
    let d = run(dir.path());
    let site = project(&d, "site");
    assert!(site.artifacts.is_empty(), "{:?}", site.artifacts);
}

#[test]
fn finds_nested_projects_but_never_inside_dependencies() {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    put(&r.join("mono").join("package.json"), r#"{"name":"mono"}"#);
    put(
        &r.join("mono")
            .join("packages")
            .join("ui")
            .join("package.json"),
        r#"{"name":"ui"}"#,
    );
    // Packages inside node_modules have package.json too; they are not projects.
    put(
        &r.join("mono")
            .join("node_modules")
            .join("lodash")
            .join("package.json"),
        r#"{"name":"lodash"}"#,
    );
    put(&r.join("rs").join("Cargo.toml"), "[package]\nname=\"rs\"\n");
    put(
        &r.join("rs").join("target").join("x").join("Cargo.toml"),
        "",
    );
    let d = run(r);
    let names: Vec<_> = d.projects.iter().map(|p| p.name.as_str()).collect();
    assert!(names.contains(&"mono") && names.contains(&"ui") && names.contains(&"rs"));
    assert!(!names.contains(&"lodash"));
    assert_eq!(d.projects.len(), 3, "{names:?}");
}

#[test]
fn malformed_and_oversized_files_become_warnings_not_failures() {
    let dir = tempfile::tempdir().unwrap();
    put(&dir.path().join("bad").join("package.json"), "{ not json");
    put(&dir.path().join("bad").join("pyproject.toml"), "[[[");
    put(
        &dir.path().join("big").join("package.json"),
        &format!("{{\"name\":\"{}\"}}", "a".repeat(1_100_000)),
    );
    let d = run(dir.path());
    let bad = project(&d, "bad");
    assert_eq!(bad.name, "bad", "falls back to the folder name");
    assert_eq!(bad.warnings.len(), 2, "{:?}", bad.warnings);
    let big = project(&d, "big");
    assert!(big.warnings[0].contains("larger than 1 MB"));
}

#[test]
fn does_not_follow_junctions_or_run_anything() {
    let dir = tempfile::tempdir().unwrap();
    let outside = dir.path().join("outside");
    put(
        &outside.join("secret-proj").join("package.json"),
        r#"{"name":"secret"}"#,
    );
    let root = dir.path().join("root");
    fs::create_dir(&root).unwrap();
    let out = Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(root.join("link"))
        .arg(&outside)
        .output()
        .unwrap();
    assert!(out.status.success());
    // A package.json whose scripts would do something if anything ran them.
    let marker = dir.path().join("ran.txt");
    put(
        &root.join("evil").join("package.json"),
        &format!(
            r#"{{"name":"evil","scripts":{{"preinstall":"echo x > {}"}}}}"#,
            marker.display().to_string().replace('\\', "/")
        ),
    );
    let d = run(&root);
    let names: Vec<_> = d.projects.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["evil"]);
    assert!(!marker.exists());
}

#[test]
fn limits_and_cancellation_are_reported() {
    let dir = tempfile::tempdir().unwrap();
    put(
        &dir.path()
            .join("a")
            .join("b")
            .join("c")
            .join("package.json"),
        "{}",
    );
    let shallow = DetectOptions {
        max_depth: 1,
        ..DetectOptions::default()
    };
    let d = detect(dir.path(), shallow, &AtomicBool::new(false)).unwrap();
    assert!(d.truncated);
    assert!(d.projects.is_empty());
    let d = detect(dir.path(), DetectOptions::default(), &AtomicBool::new(true)).unwrap();
    assert!(d.cancelled);
    assert!(
        detect(
            &dir.path().join("nope"),
            DetectOptions::default(),
            &AtomicBool::new(false)
        )
        .is_err()
    );
}

#[test]
fn virtual_environments_are_found_by_pyvenv_cfg_whatever_their_name() {
    let dir = tempfile::tempdir().unwrap();
    let app = dir.path().join("app");
    put(&app.join("requirements.txt"), "flask");
    put(&app.join("app").join("pyvenv.cfg"), "home = x");
    put(&app.join("app").join("Lib").join("site.py"), "x");
    put(&app.join("src").join("main.py"), "x");
    let d = run(dir.path());
    let p = project(&d, "app");
    let venvs: Vec<_> = p
        .artifacts
        .iter()
        .filter(|a| a.kind == ArtifactKind::PythonVenv)
        .map(|a| a.path.as_str())
        .collect();
    assert_eq!(venvs.len(), 1);
    assert!(venvs[0].ends_with(r"app\app"));
}
