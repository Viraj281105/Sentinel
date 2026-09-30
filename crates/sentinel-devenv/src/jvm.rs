//! Read-only analysis of the Maven local repository and the Gradle user home.
//!
//! What can and cannot be known, stated plainly:
//! - Maven and Gradle do not update a cached file's timestamp when a build uses it, and
//!   Windows does not track last access by default, so only *download* dates are known.
//! - Most cached artifacts are transitive dependencies that only a real build resolves.
//!   Sentinel runs no builds, so "declared by" lists only projects that name an exact
//!   version directly (in `pom.xml`, or as a literal `group:artifact:version` string in
//!   a Gradle build file). Absence from that list does not mean an artifact is unused.
//! - A project's Gradle wrapper version is read reliably from
//!   `gradle/wrapper/gradle-wrapper.properties`.
//!
//! Maven's `settings.xml` is never read (it can hold server passwords).

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use sentinel_scanner::dirent::{RawEntry, read_dir};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::parse::MAX_FILE_BYTES;

/// Directory entries examined per cache before the analysis is marked truncated.
const ENTRY_BUDGET: u64 = 2_000_000;
/// Artifacts returned in full; the rest are summarized.
const MAX_ARTIFACTS: usize = 300;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CachedVersion {
    pub version: String,
    #[ts(type = "number")]
    pub bytes: u64,
    /// Newest file time in the version folder: when it was downloaded, not last used.
    #[ts(type = "number | null")]
    pub downloaded_ms: Option<i64>,
    /// Projects that name this exact version directly.
    pub declared_by: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CachedArtifact {
    pub group: String,
    pub artifact: String,
    /// Most recently downloaded first.
    pub versions: Vec<CachedVersion>,
    #[ts(type = "number")]
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GroupSize {
    pub group: String,
    #[ts(type = "number")]
    pub bytes: u64,
    #[ts(type = "number")]
    pub versions: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ArtifactCache {
    pub root: String,
    #[ts(type = "number")]
    pub total_bytes: u64,
    #[ts(type = "number")]
    pub artifact_versions: u64,
    /// Largest groups first (top 20).
    pub groups: Vec<GroupSize>,
    /// Largest artifacts first (at most 300).
    pub artifacts: Vec<CachedArtifact>,
    /// Artifacts not listed individually.
    #[ts(type = "number")]
    pub artifacts_not_listed: u64,
    /// Artifacts with more than one version cached.
    #[ts(type = "number")]
    pub multi_version_artifacts: u64,
    /// Bytes in versions other than each artifact's most recently downloaded one.
    #[ts(type = "number")]
    pub older_versions_bytes: u64,
    /// Maven `*.lastUpdated` markers left by failed downloads.
    #[ts(type = "number")]
    pub failed_downloads: u64,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GradleVersionUse {
    pub version: String,
    /// What this entry is, e.g. "wrapper distribution (bin)" or "per-version cache".
    pub what: String,
    pub path: String,
    #[ts(type = "number")]
    pub bytes: u64,
    /// Searched projects whose wrapper uses this Gradle version.
    pub used_by: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GradleHome {
    pub root: String,
    #[ts(type = "number")]
    pub total_bytes: u64,
    pub modules: Option<ArtifactCache>,
    /// Wrapper distributions and per-Gradle-version caches.
    pub versions: Vec<GradleVersionUse>,
    /// Other shared cache folders (transforms, build cache, jars) with sizes.
    pub shared: Vec<GroupSize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct JvmAnalysis {
    pub maven: Option<ArtifactCache>,
    pub gradle: Option<GradleHome>,
    /// Projects whose build files were read for declared versions.
    #[ts(type = "number")]
    pub projects_read: u64,
    pub warnings: Vec<String>,
}

/// Exact coordinates declared by projects, and Gradle wrapper versions they use.
#[derive(Default)]
struct Declared {
    coords: HashMap<(String, String, String), Vec<String>>,
    gradle: HashMap<String, Vec<String>>,
}

fn read_small(path: &Path) -> Option<String> {
    let md = fs::metadata(path).ok()?;
    (md.len() <= MAX_FILE_BYTES)
        .then(|| fs::read_to_string(path).ok())
        .flatten()
}

fn coord_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 200
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+'))
}

/// `group:artifact:version` string literals in a Gradle build file.
pub(crate) fn gradle_literals(text: &str) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for quote in ['"', '\''] {
        for (i, part) in text.split(quote).enumerate() {
            if i % 2 == 0 {
                continue; // outside quotes
            }
            let bits: Vec<&str> = part.split(':').collect();
            if bits.len() == 3 && bits.iter().all(|b| coord_ok(b)) {
                out.push((bits[0].into(), bits[1].into(), bits[2].into()));
            }
        }
    }
    out
}

fn child<'a, 'i>(n: roxmltree::Node<'a, 'i>, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
    n.children()
        .find(|c| c.is_element() && c.tag_name().name() == name)
}

fn text_of(n: roxmltree::Node<'_, '_>, name: &str) -> Option<String> {
    child(n, name)
        .and_then(|c| c.text())
        .map(|t| t.trim().to_owned())
}

/// Direct dependencies with resolvable versions from a `pom.xml`.
pub(crate) fn pom_dependencies(text: &str) -> Vec<(String, String, String)> {
    let Ok(doc) = roxmltree::Document::parse(text) else {
        return Vec::new();
    };
    let root = doc.root_element();
    let mut props: HashMap<String, String> = HashMap::new();
    if let Some(p) = child(root, "properties") {
        for c in p.children().filter(roxmltree::Node::is_element) {
            if let Some(t) = c.text() {
                props.insert(c.tag_name().name().to_owned(), t.trim().to_owned());
            }
        }
    }
    let project_version = text_of(root, "version")
        .or_else(|| child(root, "parent").and_then(|p| text_of(p, "version")));
    if let Some(v) = &project_version {
        props.insert("project.version".into(), v.clone());
    }
    let resolve = |v: &str| -> Option<String> {
        let v = v.trim();
        let resolved = match v.strip_prefix("${").and_then(|r| r.strip_suffix('}')) {
            Some(name) => props.get(name)?.clone(),
            None => v.to_owned(),
        };
        coord_ok(&resolved).then_some(resolved)
    };
    let mut out = Vec::new();
    for deps_parent in [Some(root), child(root, "dependencyManagement")]
        .into_iter()
        .flatten()
    {
        let Some(deps) = child(deps_parent, "dependencies") else {
            continue;
        };
        for d in deps
            .children()
            .filter(|c| c.is_element() && c.tag_name().name() == "dependency")
        {
            if let (Some(g), Some(a), Some(v)) = (
                text_of(d, "groupId"),
                text_of(d, "artifactId"),
                text_of(d, "version").and_then(|v| resolve(&v)),
            ) && coord_ok(&g)
                && coord_ok(&a)
            {
                out.push((g, a, v));
            }
        }
    }
    out
}

/// The Gradle version a project's wrapper downloads, from `distributionUrl`.
pub(crate) fn wrapper_version(properties: &str) -> Option<String> {
    let url = properties
        .lines()
        .find_map(|l| l.trim().strip_prefix("distributionUrl"))?;
    let file = url.rsplit(['/', '\\']).next()?;
    let v = file
        .strip_prefix("gradle-")?
        .strip_suffix(".zip")?
        .rsplit_once('-')?
        .0;
    coord_ok(v).then(|| v.to_owned())
}

fn read_declarations(projects: &[PathBuf], warnings: &mut Vec<String>) -> (Declared, u64) {
    let mut d = Declared::default();
    let mut read = 0;
    for p in projects {
        let name = p.display().to_string();
        let mut any = false;
        if let Some(t) = read_small(&p.join("pom.xml")) {
            any = true;
            if roxmltree::Document::parse(&t).is_err() {
                warnings.push(format!("{name}: pom.xml could not be parsed"));
            }
            for c in pom_dependencies(&t) {
                d.coords.entry(c).or_default().push(name.clone());
            }
        }
        for f in ["build.gradle", "build.gradle.kts"] {
            if let Some(t) = read_small(&p.join(f)) {
                any = true;
                for c in gradle_literals(&t) {
                    d.coords.entry(c).or_default().push(name.clone());
                }
            }
        }
        let props = p
            .join("gradle")
            .join("wrapper")
            .join("gradle-wrapper.properties");
        if let Some(v) = read_small(&props).as_deref().and_then(wrapper_version) {
            any = true;
            d.gradle.entry(v).or_default().push(name.clone());
        }
        read += u64::from(any);
    }
    (d, read)
}

struct Budget {
    left: u64,
    truncated: bool,
}

impl Budget {
    fn list(&mut self, dir: &Path) -> Vec<RawEntry> {
        if self.left == 0 {
            self.truncated = true;
            return Vec::new();
        }
        let entries = read_dir(dir).unwrap_or_default();
        self.left = self.left.saturating_sub(entries.len() as u64);
        entries
    }
}

/// Allocated bytes and newest file time under `dir`, without following links.
fn measure(dir: &Path, budget: &mut Budget, failed: &mut u64) -> (u64, Option<i64>) {
    let mut bytes = 0;
    let mut newest = None;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in budget.list(&d) {
            if e.is_reparse_point() {
                continue;
            }
            if e.is_dir() {
                stack.push(d.join(&e.name));
            } else {
                bytes += e.allocated_bytes;
                newest = newest.max(Some(e.modified_ms));
                if e.name.ends_with(".lastUpdated") {
                    *failed += 1;
                }
            }
        }
    }
    (bytes, newest)
}

type Versions = BTreeMap<(String, String), Vec<CachedVersion>>;

fn summarize(root: &Path, found: Versions, failed: u64, truncated: bool) -> ArtifactCache {
    let mut groups: HashMap<String, (u64, u64)> = HashMap::new();
    let mut artifacts: Vec<CachedArtifact> = found
        .into_iter()
        .map(|((group, artifact), mut versions)| {
            versions.sort_by_key(|v| std::cmp::Reverse(v.downloaded_ms));
            let bytes = versions.iter().map(|v| v.bytes).sum();
            let g = groups.entry(group.clone()).or_default();
            g.0 += bytes;
            g.1 += versions.len() as u64;
            CachedArtifact {
                group,
                artifact,
                versions,
                bytes,
            }
        })
        .collect();
    artifacts.sort_by_key(|a| std::cmp::Reverse(a.bytes));
    let total_bytes = artifacts.iter().map(|a| a.bytes).sum();
    let artifact_versions = artifacts.iter().map(|a| a.versions.len() as u64).sum();
    let multi = artifacts.iter().filter(|a| a.versions.len() > 1).count() as u64;
    let older = artifacts
        .iter()
        .flat_map(|a| a.versions.iter().skip(1))
        .map(|v| v.bytes)
        .sum();
    let mut groups: Vec<GroupSize> = groups
        .into_iter()
        .map(|(group, (bytes, versions))| GroupSize {
            group,
            bytes,
            versions,
        })
        .collect();
    groups.sort_by_key(|g| std::cmp::Reverse(g.bytes));
    groups.truncate(20);
    let not_listed = artifacts.len().saturating_sub(MAX_ARTIFACTS) as u64;
    artifacts.truncate(MAX_ARTIFACTS);
    ArtifactCache {
        root: root.display().to_string(),
        total_bytes,
        artifact_versions,
        groups,
        artifacts,
        artifacts_not_listed: not_listed,
        multi_version_artifacts: multi,
        older_versions_bytes: older,
        failed_downloads: failed,
        truncated,
    }
}

/// Walk a Maven repository: `group/path/artifact/version/artifact-version.pom|jar`.
fn maven(root: &Path, declared: &Declared) -> ArtifactCache {
    let mut budget = Budget {
        left: ENTRY_BUDGET,
        truncated: false,
    };
    let mut failed = 0;
    let mut found: Versions = BTreeMap::new();
    let mut stack: Vec<(PathBuf, Vec<String>)> = vec![(root.to_path_buf(), Vec::new())];
    while let Some((dir, parts)) = stack.pop() {
        let entries = budget.list(&dir);
        let is_version = parts.len() >= 2 && {
            let artifact = &parts[parts.len() - 2];
            let version = &parts[parts.len() - 1];
            let stem = format!("{artifact}-{version}");
            entries.iter().any(|e| {
                !e.is_dir()
                    && e.name.starts_with(&stem)
                    && (e.name.ends_with(".pom") || e.name.ends_with(".jar"))
            })
        };
        if is_version {
            let (bytes, newest) = measure(&dir, &mut budget, &mut failed);
            let version = parts[parts.len() - 1].clone();
            let artifact = parts[parts.len() - 2].clone();
            let group = parts[..parts.len() - 2].join(".");
            let declared_by = declared
                .coords
                .get(&(group.clone(), artifact.clone(), version.clone()))
                .cloned()
                .unwrap_or_default();
            found
                .entry((group, artifact))
                .or_default()
                .push(CachedVersion {
                    version,
                    bytes,
                    downloaded_ms: newest,
                    declared_by,
                });
            continue;
        }
        for e in entries {
            if e.is_reparse_point() {
                continue;
            }
            if e.is_dir() {
                let mut p = parts.clone();
                p.push(e.name.clone());
                stack.push((dir.join(&e.name), p));
            } else if e.name.ends_with(".lastUpdated") {
                failed += 1;
            }
        }
    }
    summarize(root, found, failed, budget.truncated)
}

/// Gradle's `caches/modules-2/files-2.1/group/artifact/version/hash/file`.
fn gradle_modules(root: &Path, declared: &Declared) -> ArtifactCache {
    let mut budget = Budget {
        left: ENTRY_BUDGET,
        truncated: false,
    };
    let mut failed = 0;
    let mut found: Versions = BTreeMap::new();
    let dirs = |b: &mut Budget, p: &Path| -> Vec<String> {
        b.list(p)
            .into_iter()
            .filter(|e| e.is_dir() && !e.is_reparse_point())
            .map(|e| e.name)
            .collect()
    };
    for group in dirs(&mut budget, root) {
        let gp = root.join(&group);
        for artifact in dirs(&mut budget, &gp) {
            let ap = gp.join(&artifact);
            for version in dirs(&mut budget, &ap) {
                let (bytes, newest) = measure(&ap.join(&version), &mut budget, &mut failed);
                let declared_by = declared
                    .coords
                    .get(&(group.clone(), artifact.clone(), version.clone()))
                    .cloned()
                    .unwrap_or_default();
                found
                    .entry((group.clone(), artifact.clone()))
                    .or_default()
                    .push(CachedVersion {
                        version,
                        bytes,
                        downloaded_ms: newest,
                        declared_by,
                    });
            }
        }
    }
    summarize(root, found, 0, budget.truncated)
}

fn looks_like_version(name: &str) -> bool {
    let mut parts = name.split('.');
    parts
        .next()
        .is_some_and(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
        && parts.all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
        && name.contains('.')
}

fn gradle(home: &Path, declared: &Declared) -> GradleHome {
    let mut budget = Budget {
        left: ENTRY_BUDGET,
        truncated: false,
    };
    let mut failed = 0;
    let mut versions = Vec::new();
    let mut shared = Vec::new();
    let mut total = 0;
    let used = |v: &str| declared.gradle.get(v).cloned().unwrap_or_default();

    let dists = home.join("wrapper").join("dists");
    for e in budget.list(&dists) {
        if !e.is_dir() || e.is_reparse_point() {
            continue;
        }
        // gradle-8.5-bin, gradle-8.5-all
        let Some((v, kind)) = e
            .name
            .strip_prefix("gradle-")
            .and_then(|r| r.rsplit_once('-'))
        else {
            continue;
        };
        let path = dists.join(&e.name);
        let (bytes, _) = measure(&path, &mut budget, &mut failed);
        total += bytes;
        versions.push(GradleVersionUse {
            version: v.to_owned(),
            what: format!("wrapper distribution ({kind})"),
            path: path.display().to_string(),
            bytes,
            used_by: used(v),
        });
    }

    let caches = home.join("caches");
    let mut modules = None;
    for e in budget.list(&caches) {
        if !e.is_dir() || e.is_reparse_point() {
            continue;
        }
        let path = caches.join(&e.name);
        if e.name == "modules-2" {
            let m = gradle_modules(&path.join("files-2.1"), declared);
            let (bytes, _) = measure(&path, &mut budget, &mut failed);
            total += bytes;
            shared.push(GroupSize {
                group: e.name.clone(),
                bytes,
                versions: m.artifact_versions,
            });
            modules = Some(m);
            continue;
        }
        let (bytes, _) = measure(&path, &mut budget, &mut failed);
        total += bytes;
        if looks_like_version(&e.name) {
            versions.push(GradleVersionUse {
                version: e.name.clone(),
                what: "per-version cache".into(),
                path: path.display().to_string(),
                bytes,
                used_by: used(&e.name),
            });
        } else {
            shared.push(GroupSize {
                group: e.name.clone(),
                bytes,
                versions: 0,
            });
        }
    }
    versions.sort_by_key(|v| std::cmp::Reverse(v.bytes));
    shared.sort_by_key(|s| std::cmp::Reverse(s.bytes));
    GradleHome {
        root: home.display().to_string(),
        total_bytes: total,
        modules,
        versions,
        shared,
    }
}

/// Analyze the Maven repository and Gradle home (either may be absent) and relate them
/// to `projects`. Read-only.
pub fn analyze_jvm_caches(
    maven_repo: Option<&Path>,
    gradle_home: Option<&Path>,
    projects: &[PathBuf],
) -> JvmAnalysis {
    let mut warnings = Vec::new();
    let (declared, projects_read) = read_declarations(projects, &mut warnings);
    let maven = maven_repo
        .filter(|p| p.is_dir())
        .map(|p| maven(p, &declared));
    let gradle = gradle_home
        .filter(|p| p.is_dir())
        .map(|p| gradle(p, &declared));
    JvmAnalysis {
        maven,
        gradle,
        projects_read,
        warnings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_gradle_coordinate_literals_only() {
        let text = r#"
            implementation("org.slf4j:slf4j-api:2.0.9")
            implementation 'com.google.guava:guava:33.0.0-jre'
            implementation(libs.foo)
            println "a:b"
            val x = "not a coordinate: really"
        "#;
        let got = gradle_literals(text);
        assert!(got.contains(&("org.slf4j".into(), "slf4j-api".into(), "2.0.9".into())));
        assert!(got.contains(&(
            "com.google.guava".into(),
            "guava".into(),
            "33.0.0-jre".into()
        )));
        assert_eq!(got.len(), 2, "{got:?}");
    }

    #[test]
    fn reads_pom_dependencies_with_property_versions() {
        let pom = r#"<project xmlns="http://maven.apache.org/POM/4.0.0">
            <version>1.2.3</version>
            <properties><jackson.version>2.17.0</jackson.version></properties>
            <dependencies>
              <dependency><groupId>com.fasterxml.jackson.core</groupId>
                <artifactId>jackson-databind</artifactId><version>${jackson.version}</version></dependency>
              <dependency><groupId>org.me</groupId><artifactId>self</artifactId>
                <version>${project.version}</version></dependency>
              <dependency><groupId>junit</groupId><artifactId>junit</artifactId></dependency>
              <dependency><groupId>x</groupId><artifactId>y</artifactId>
                <version>${undefined}</version></dependency>
            </dependencies>
          </project>"#;
        let got = pom_dependencies(pom);
        assert_eq!(
            got,
            [
                (
                    "com.fasterxml.jackson.core".into(),
                    "jackson-databind".into(),
                    "2.17.0".into()
                ),
                ("org.me".into(), "self".into(), "1.2.3".into()),
            ]
        );
        assert!(pom_dependencies("<not xml").is_empty());
    }

    #[test]
    fn reads_the_wrapper_distribution_version() {
        let p = "distributionBase=GRADLE_USER_HOME\ndistributionUrl=https\\://services.gradle.org/distributions/gradle-8.5-bin.zip\n";
        assert_eq!(wrapper_version(p).as_deref(), Some("8.5"));
        assert_eq!(
            wrapper_version("distributionUrl=https://x/gradle-7.6.1-all.zip").as_deref(),
            Some("7.6.1")
        );
        assert_eq!(wrapper_version("nothing"), None);
    }

    #[test]
    fn recognizes_gradle_version_folder_names() {
        for v in ["8.5", "7.6.1", "8.10-rc-1"] {
            assert!(looks_like_version(v), "{v}");
        }
        for v in ["modules-2", "transforms-4", "jars-9", "build-cache-1", "8"] {
            assert!(!looks_like_version(v), "{v}");
        }
    }
}
