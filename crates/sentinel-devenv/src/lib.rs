//! Read-only detection of development projects.
//!
//! [`detect`] walks a folder looking for project marker files (`package.json`,
//! `Cargo.toml`, `pyproject.toml`, ...). For each project it records ecosystems, package
//! managers (from lockfiles), required runtime versions, Git presence, an approximate
//! last-activity time, and rebuildable artifact folders (`node_modules`, virtual
//! environments, `target`, ...) with their sizes.
//!
//! Project files are parsed with size limits and never executed: no package manager,
//! build tool, script or Git hook is run.

mod model;
mod parse;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use sentinel_scanner::dirent::{RawEntry, read_dir};
use sentinel_scanner::scan::{ScanControl, ScanOptions, ScanTree, scan};

pub use model::{
    Artifact, ArtifactKind, Detection, Ecosystem, PackageManager, Project, Runtime,
    RuntimeRequirement,
};

/// Folder names never searched for projects: dependency stores, VCS data and caches.
const SKIP_ANYWHERE: &[&str] = &[
    "node_modules",
    ".git",
    ".hg",
    ".svn",
    ".venv",
    "__pycache__",
    ".tox",
    ".gradle",
    ".next",
    ".nuxt",
    ".cache",
    "appdata",
    "$recycle.bin",
    "system volume information",
];

/// Folders directly under a drive root that are never searched.
const SKIP_AT_DRIVE_ROOT: &[&str] = &[
    "windows",
    "program files",
    "program files (x86)",
    "programdata",
    "recovery",
];

#[derive(Debug, Clone, Copy)]
pub struct DetectOptions {
    pub max_depth: u32,
    pub max_folders: u64,
    /// Measure project and artifact sizes with the scanner.
    pub measure: bool,
}

impl Default for DetectOptions {
    fn default() -> Self {
        Self {
            max_depth: 10,
            max_folders: 300_000,
            measure: true,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DetectError {
    #[error("{0} is not a folder that can be searched")]
    NotAFolder(PathBuf),
}

struct Walk<'a> {
    opts: DetectOptions,
    cancel: &'a AtomicBool,
    visited: u64,
    truncated: bool,
    errors: Vec<String>,
    projects: Vec<Project>,
}

fn has_ext(entries: &[RawEntry], exts: &[&str]) -> bool {
    entries.iter().any(|e| {
        !e.is_dir()
            && Path::new(&e.name)
                .extension()
                .is_some_and(|x| exts.iter().any(|w| x.eq_ignore_ascii_case(w)))
    })
}

/// Recognize a project in `dir` from its entries. Returns `None` if nothing marks it.
fn recognize(dir: &Path, entries: &[RawEntry]) -> Option<Project> {
    let files: BTreeSet<String> = entries
        .iter()
        .filter(|e| !e.is_dir())
        .map(|e| e.name.to_lowercase())
        .collect();
    let dirs: BTreeSet<String> = entries
        .iter()
        .filter(|e| e.is_dir())
        .map(|e| e.name.to_lowercase())
        .collect();
    let f = |n: &str| files.contains(n);

    let mut eco = BTreeSet::new();
    let mut pm = BTreeSet::new();
    let mut facts = parse::Facts::default();

    if f("package.json") {
        eco.insert(Ecosystem::Node);
        for (lock, m) in [
            ("package-lock.json", PackageManager::Npm),
            ("pnpm-lock.yaml", PackageManager::Pnpm),
            ("yarn.lock", PackageManager::Yarn),
            ("bun.lockb", PackageManager::Bun),
            ("bun.lock", PackageManager::Bun),
        ] {
            if f(lock) {
                pm.insert(m);
            }
        }
        parse::package_json(dir, &mut facts);
    }
    let python_markers = [
        "pyproject.toml",
        "requirements.txt",
        "setup.py",
        "pipfile",
        "poetry.lock",
        "uv.lock",
    ];
    if python_markers.iter().any(|m| f(m)) {
        eco.insert(Ecosystem::Python);
        for (lock, m) in [
            ("poetry.lock", PackageManager::Poetry),
            ("pipfile", PackageManager::Pipenv),
            ("uv.lock", PackageManager::Uv),
            ("requirements.txt", PackageManager::Pip),
        ] {
            if f(lock) {
                pm.insert(m);
            }
        }
        if f("pyproject.toml") {
            parse::pyproject(dir, &mut facts);
        }
    }
    if f("cargo.toml") {
        eco.insert(Ecosystem::Rust);
        pm.insert(PackageManager::Cargo);
        parse::cargo_toml(dir, &mut facts);
    }
    if f("pom.xml") {
        eco.insert(Ecosystem::Java);
        pm.insert(PackageManager::Maven);
    }
    if [
        "build.gradle",
        "build.gradle.kts",
        "settings.gradle",
        "settings.gradle.kts",
    ]
    .iter()
    .any(|m| f(m))
    {
        eco.insert(Ecosystem::Java);
        pm.insert(PackageManager::Gradle);
    }
    if has_ext(entries, &["csproj", "fsproj", "vbproj", "sln"]) {
        eco.insert(Ecosystem::DotNet);
        pm.insert(PackageManager::DotNet);
    }
    if f("go.mod") {
        eco.insert(Ecosystem::Go);
        pm.insert(PackageManager::GoModules);
        parse::go_mod(dir, &mut facts);
    }
    if f("gemfile") {
        eco.insert(Ecosystem::Ruby);
        pm.insert(PackageManager::Bundler);
    }
    if [
        "dockerfile",
        "docker-compose.yml",
        "docker-compose.yaml",
        "compose.yml",
        "compose.yaml",
    ]
    .iter()
    .any(|m| f(m))
    {
        eco.insert(Ecosystem::Docker);
    }
    if eco.is_empty() {
        return None;
    }

    for (file, rt) in [
        (".nvmrc", Runtime::Node),
        (".node-version", Runtime::Node),
        (".python-version", Runtime::Python),
        ("rust-toolchain", Runtime::Rust),
    ] {
        if f(file) {
            parse::version_file(dir, file, rt, &mut facts);
        }
    }
    if f("rust-toolchain.toml") {
        parse::rust_toolchain_toml(dir, &mut facts);
    }

    let mut artifacts = Vec::new();
    let mut add = |name: &str, kind: ArtifactKind| {
        if let Some(e) = entries
            .iter()
            .find(|e| e.is_dir() && !e.is_reparse_point() && e.name.eq_ignore_ascii_case(name))
        {
            artifacts.push(Artifact {
                kind,
                path: dir.join(&e.name).display().to_string(),
                bytes: None,
                restored_by: kind.restored_by().to_owned(),
            });
        }
    };
    if eco.contains(&Ecosystem::Node) {
        add("node_modules", ArtifactKind::NodeModules);
        add(".next", ArtifactKind::NextBuild);
    }
    if eco.contains(&Ecosystem::Python) {
        // A virtual environment is identified by its pyvenv.cfg, whatever the folder is
        // called (`.venv`, `venv`, or a custom name such as the project's own).
        let venvs: Vec<String> = entries
            .iter()
            .filter(|e| e.is_dir() && !e.is_reparse_point())
            .filter(|e| dir.join(&e.name).join("pyvenv.cfg").is_file())
            .map(|e| e.name.clone())
            .collect();
        for venv in &venvs {
            add(venv, ArtifactKind::PythonVenv);
        }
        for cache in [".pytest_cache", ".mypy_cache", ".ruff_cache", ".tox"] {
            add(cache, ArtifactKind::PythonToolCache);
        }
    }
    if eco.contains(&Ecosystem::Rust) {
        add("target", ArtifactKind::CargoTarget);
    }
    if pm.contains(&PackageManager::Gradle) {
        add("build", ArtifactKind::GradleBuild);
        add(".gradle", ArtifactKind::GradleProjectCache);
    }
    if eco.contains(&Ecosystem::DotNet) {
        add("bin", ArtifactKind::DotNetBuild);
        add("obj", ArtifactKind::DotNetBuild);
    }

    // Last activity: top-level entries other than artifact folders, plus Git metadata.
    let artifact_names: BTreeSet<String> = artifacts
        .iter()
        .filter_map(|a| Path::new(&a.path).file_name())
        .map(|n| n.to_string_lossy().to_lowercase())
        .collect();
    let mut last = entries
        .iter()
        .filter(|e| !artifact_names.contains(&e.name.to_lowercase()) && e.name != ".git")
        .map(|e| e.modified_ms)
        .max();
    let git_dir = dir.join(".git");
    let git = dirs.contains(".git") || files.contains(".git");
    if git_dir.is_dir()
        && let Ok(git_entries) = read_dir(&git_dir)
    {
        for e in git_entries {
            if matches!(
                e.name.as_str(),
                "HEAD" | "index" | "FETCH_HEAD" | "ORIG_HEAD"
            ) {
                last = last.max(Some(e.modified_ms));
            }
        }
    }

    let markers: Vec<String> = entries
        .iter()
        .filter(|e| !e.is_dir() && is_marker(&e.name))
        .map(|e| e.name.clone())
        .collect();
    let name = facts.name.unwrap_or_else(|| {
        dir.file_name().map_or_else(
            || dir.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        )
    });
    Some(Project {
        path: dir.display().to_string(),
        name,
        ecosystems: eco.into_iter().collect(),
        package_managers: pm.into_iter().collect(),
        markers,
        runtimes: facts.runtimes,
        git,
        last_activity_ms: last,
        total_bytes: None,
        artifacts,
        warnings: facts.warnings,
    })
}

fn is_marker(name: &str) -> bool {
    const MARKERS: &[&str] = &[
        "package.json",
        "package-lock.json",
        "pnpm-lock.yaml",
        "yarn.lock",
        "bun.lockb",
        "bun.lock",
        "pyproject.toml",
        "requirements.txt",
        "setup.py",
        "pipfile",
        "poetry.lock",
        "uv.lock",
        "cargo.toml",
        "cargo.lock",
        "pom.xml",
        "build.gradle",
        "build.gradle.kts",
        "settings.gradle",
        "settings.gradle.kts",
        "go.mod",
        "gemfile",
        "dockerfile",
        "docker-compose.yml",
        "docker-compose.yaml",
        "compose.yml",
        "compose.yaml",
    ];
    let lower = name.to_lowercase();
    MARKERS.contains(&lower.as_str())
        || Path::new(&lower).extension().is_some_and(|x| {
            ["csproj", "fsproj", "vbproj", "sln"]
                .iter()
                .any(|w| x == *w)
        })
}

fn is_drive_root(p: &Path) -> bool {
    p.parent().is_none()
}

impl Walk<'_> {
    fn visit(&mut self, dir: &Path, depth: u32) {
        if self.cancel.load(Ordering::Relaxed) {
            return;
        }
        if self.visited >= self.opts.max_folders || depth > self.opts.max_depth {
            self.truncated = true;
            return;
        }
        self.visited += 1;
        let entries = match read_dir(dir) {
            Ok(e) => e,
            Err(err) => {
                // Unreadable folders are normal (permissions); keep a short list.
                if self.errors.len() < 50 {
                    self.errors.push(format!("{}: {err}", dir.display()));
                }
                return;
            }
        };
        let project = recognize(dir, &entries);
        let artifact_names: BTreeSet<String> = project
            .iter()
            .flat_map(|p| &p.artifacts)
            .filter_map(|a| Path::new(&a.path).file_name())
            .map(|n| n.to_string_lossy().to_lowercase())
            .collect();
        if let Some(p) = project {
            self.projects.push(p);
        }
        let at_drive_root = is_drive_root(dir);
        for e in &entries {
            if !e.is_dir() || e.is_reparse_point() {
                continue;
            }
            let lower = e.name.to_lowercase();
            if SKIP_ANYWHERE.contains(&lower.as_str())
                || artifact_names.contains(&lower)
                || (at_drive_root && SKIP_AT_DRIVE_ROOT.contains(&lower.as_str()))
            {
                continue;
            }
            self.visit(&dir.join(&e.name), depth + 1);
        }
    }
}

fn measure(p: &mut Project, cancel: &AtomicBool) {
    if cancel.load(Ordering::Relaxed) {
        return;
    }
    let opts = ScanOptions {
        threads: 2,
        largest_files: 0,
        ..ScanOptions::default()
    };
    let Ok(tree) = scan(Path::new(&p.path), &opts, &ScanControl::new()) else {
        return;
    };
    p.total_bytes = Some(tree.stats.total_bytes);
    let root = &tree.nodes[ScanTree::ROOT as usize];
    for a in &mut p.artifacts {
        let name = Path::new(&a.path)
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase());
        a.bytes = root
            .children
            .iter()
            .map(|&c| &tree.nodes[c as usize])
            .find(|n| Some(n.name.to_lowercase()) == name)
            .map(|n| n.total_bytes);
    }
}

/// Find projects under `root`. Read-only.
pub fn detect(
    root: &Path,
    opts: DetectOptions,
    cancel: &AtomicBool,
) -> Result<Detection, DetectError> {
    let started = Instant::now();
    if !root.is_dir() {
        return Err(DetectError::NotAFolder(root.to_path_buf()));
    }
    let mut w = Walk {
        opts,
        cancel,
        visited: 0,
        truncated: false,
        errors: Vec::new(),
        projects: Vec::new(),
    };
    w.visit(root, 0);
    if opts.measure {
        for p in &mut w.projects {
            measure(p, cancel);
        }
    }
    w.projects.sort_by_key(|p| p.path.to_lowercase());
    let d = Detection {
        root: root.display().to_string(),
        projects: w.projects,
        folders_visited: w.visited,
        truncated: w.truncated,
        cancelled: cancel.load(Ordering::Relaxed),
        errors: w.errors,
        elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    };
    tracing::info!(
        root = %d.root,
        projects = d.projects.len(),
        folders = d.folders_visited,
        truncated = d.truncated,
        elapsed_ms = d.elapsed_ms,
        "project detection finished"
    );
    Ok(d)
}
