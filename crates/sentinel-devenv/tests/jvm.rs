//! Maven and Gradle cache analysis on synthetic fixtures.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs::{self, OpenOptions};
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use sentinel_devenv::analyze_jvm_caches;

fn put(path: &Path, bytes: usize) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, vec![b'x'; bytes]).unwrap();
}

fn put_text(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn age(path: &Path, days: u64) {
    OpenOptions::new()
        .access_mode(0x0100)
        .custom_flags(0x0200_0000)
        .open(path)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(days * 86_400))
        .unwrap();
}

#[test]
fn analyzes_a_maven_repository_and_links_declared_versions() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("m2").join("repository");
    let lib = repo.join("com").join("example").join("lib");
    put(&lib.join("1.0").join("lib-1.0.jar"), 40_000);
    put(&lib.join("1.0").join("lib-1.0.pom"), 100);
    age(&lib.join("1.0").join("lib-1.0.jar"), 300);
    age(&lib.join("1.0").join("lib-1.0.pom"), 300);
    put(&lib.join("2.0").join("lib-2.0.jar"), 50_000);
    put(&lib.join("2.0").join("lib-2.0.pom"), 100);
    put(
        &repo
            .join("org")
            .join("other")
            .join("tool")
            .join("3.1")
            .join("tool-3.1.pom"),
        200,
    );
    // A failed download leaves only a marker behind.
    put(
        &repo
            .join("bad")
            .join("dep")
            .join("9.9")
            .join("dep-9.9.jar.lastUpdated"),
        50,
    );

    let project = dir.path().join("svc");
    put_text(
        &project.join("pom.xml"),
        r"<project><properties><lib.version>2.0</lib.version></properties>
          <dependencies><dependency><groupId>com.example</groupId><artifactId>lib</artifactId>
          <version>${lib.version}</version></dependency></dependencies></project>",
    );

    let a = analyze_jvm_caches(Some(&repo), None, std::slice::from_ref(&project));
    assert_eq!(a.projects_read, 1);
    let m = a.maven.unwrap();
    assert_eq!(m.artifact_versions, 3);
    assert_eq!(m.failed_downloads, 1);
    assert_eq!(m.multi_version_artifacts, 1);
    let lib = m.artifacts.iter().find(|x| x.artifact == "lib").unwrap();
    assert_eq!(lib.group, "com.example");
    assert_eq!(
        lib.versions[0].version, "2.0",
        "most recently downloaded first"
    );
    assert_eq!(lib.versions[0].declared_by, [project.display().to_string()]);
    assert!(lib.versions[1].declared_by.is_empty());
    assert_eq!(m.older_versions_bytes, lib.versions[1].bytes);
    assert!(m.groups.iter().any(|g| g.group == "com.example"));
    assert!(!m.truncated);
}

#[test]
fn analyzes_a_gradle_home_and_wrapper_use() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("gradle-home");
    let dists = home.join("wrapper").join("dists");
    put(
        &dists
            .join("gradle-8.5-bin")
            .join("abc")
            .join("gradle-8.5")
            .join("lib")
            .join("a.jar"),
        30_000,
    );
    put(
        &dists
            .join("gradle-7.6-all")
            .join("def")
            .join("gradle-7.6")
            .join("lib")
            .join("a.jar"),
        60_000,
    );
    let caches = home.join("caches");
    put(&caches.join("8.5").join("kotlin-dsl").join("x"), 10_000);
    put(&caches.join("7.6").join("kotlin-dsl").join("x"), 20_000);
    put(&caches.join("transforms-4").join("t"), 5_000);
    put(
        &caches
            .join("modules-2")
            .join("files-2.1")
            .join("org.slf4j")
            .join("slf4j-api")
            .join("2.0.9")
            .join("0f00")
            .join("slf4j-api-2.0.9.jar"),
        8_000,
    );

    let project: PathBuf = dir.path().join("app");
    put_text(
        &project
            .join("gradle")
            .join("wrapper")
            .join("gradle-wrapper.properties"),
        "distributionUrl=https\\://services.gradle.org/distributions/gradle-8.5-bin.zip\n",
    );
    put_text(
        &project.join("build.gradle.kts"),
        r#"dependencies { implementation("org.slf4j:slf4j-api:2.0.9") }"#,
    );

    let a = analyze_jvm_caches(None, Some(&home), std::slice::from_ref(&project));
    let g = a.gradle.unwrap();
    let who = project.display().to_string();
    let find = |v: &str, what: &str| {
        g.versions
            .iter()
            .find(|x| x.version == v && x.what.contains(what))
            .unwrap_or_else(|| panic!("{v} {what}: {:#?}", g.versions))
    };
    assert_eq!(find("8.5", "wrapper").used_by, std::slice::from_ref(&who));
    assert!(
        find("7.6", "wrapper").used_by.is_empty(),
        "no searched project uses 7.6"
    );
    assert_eq!(
        find("8.5", "per-version").used_by,
        std::slice::from_ref(&who)
    );
    assert!(find("7.6", "per-version").used_by.is_empty());
    assert!(g.shared.iter().any(|s| s.group == "transforms-4"));
    let modules = g.modules.unwrap();
    let slf4j = modules
        .artifacts
        .iter()
        .find(|x| x.artifact == "slf4j-api")
        .unwrap();
    assert_eq!(slf4j.group, "org.slf4j");
    assert_eq!(slf4j.versions[0].declared_by, [who]);
    assert!(g.total_bytes >= 130_000);
}

#[test]
fn missing_caches_are_reported_as_absent() {
    let dir = tempfile::tempdir().unwrap();
    let a = analyze_jvm_caches(
        Some(&dir.path().join("nope")),
        Some(&dir.path().join("nope2")),
        &[],
    );
    assert!(a.maven.is_none() && a.gradle.is_none());
    assert_eq!(a.projects_read, 0);
}
