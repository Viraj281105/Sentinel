//! Read-only inventory of installed development runtimes.
//!
//! Versions come from the install's own files (`release`, `VERSION`, `patchlevel.h`, the
//! rustup manifest) or from folder names that version managers use, so nothing is run.
//! The one exception is a standalone Node.js install, which carries no version file:
//! its resolved `node.exe` is run with the fixed argument `--version` and a 5 s limit.
//!
//! Detected: Node.js (nvm-windows, fnm, Volta, installer), Python (python.org installer,
//! pyenv-win, uv), Java (JDK/JRE folders, IDE and Gradle downloads, `JAVA_HOME`), .NET
//! (SDKs and the shared runtime), Rust (rustup), Go and the Android SDK. Windows Store
//! Python, scoop and winget-only layouts are not looked for.
//!
//! Installs are related to projects only through requirements the project files state
//! (see [`satisfies`]); a requirement that cannot be understood is never counted as met
//! or unmet.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use sentinel_scanner::dirent::read_dir;
use sentinel_scanner::scan::{ScanControl, ScanOptions, scan};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::model::{Project, Runtime};
use crate::parse::MAX_FILE_BYTES;
use crate::version::{Satisfies, parse_version, satisfies};

/// The rustup manifest lists every component of the toolchain and is a few hundred KB.
const MANIFEST_MAX_BYTES: u64 = 16 * 1024 * 1024;
const VERSION_PROBE_LIMIT: Duration = Duration::from_secs(5);

/// Where installs are looked for. Every path is optional; a missing one is skipped.
#[derive(Debug, Clone, Default)]
pub struct Locations {
    pub profile: Option<PathBuf>,
    pub local_app_data: Option<PathBuf>,
    pub app_data: Option<PathBuf>,
    pub program_files: Vec<PathBuf>,
    pub java_home: Option<PathBuf>,
    pub android_home: Option<PathBuf>,
    pub rustup_home: Option<PathBuf>,
    pub goroot: Option<PathBuf>,
    pub gopath: Option<PathBuf>,
    pub dotnet_root: Option<PathBuf>,
    pub nvm_home: Option<PathBuf>,
    pub fnm_dir: Option<PathBuf>,
    pub volta_home: Option<PathBuf>,
}

fn env_dir(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute() && p.is_dir())
}

impl Locations {
    /// The current user's standard locations and the environment overrides the tools honor.
    pub fn from_env() -> Self {
        let mut program_files: Vec<PathBuf> = ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"]
            .iter()
            .filter_map(|n| env_dir(n))
            .collect();
        program_files.dedup();
        Self {
            profile: env_dir("USERPROFILE"),
            local_app_data: env_dir("LOCALAPPDATA"),
            app_data: env_dir("APPDATA"),
            program_files,
            java_home: env_dir("JAVA_HOME"),
            android_home: env_dir("ANDROID_HOME").or_else(|| env_dir("ANDROID_SDK_ROOT")),
            rustup_home: env_dir("RUSTUP_HOME"),
            goroot: env_dir("GOROOT"),
            gopath: env_dir("GOPATH"),
            dotnet_root: env_dir("DOTNET_ROOT"),
            nvm_home: env_dir("NVM_HOME"),
            fnm_dir: env_dir("FNM_DIR"),
            volta_home: env_dir("VOLTA_HOME"),
        }
    }

    fn under_profile(&self, parts: &[&str]) -> Option<PathBuf> {
        join(self.profile.as_deref(), parts)
    }
    fn under_local(&self, parts: &[&str]) -> Option<PathBuf> {
        join(self.local_app_data.as_deref(), parts)
    }
    fn under_roaming(&self, parts: &[&str]) -> Option<PathBuf> {
        join(self.app_data.as_deref(), parts)
    }
}

fn join(base: Option<&Path>, parts: &[&str]) -> Option<PathBuf> {
    base.map(|b| parts.iter().fold(b.to_path_buf(), |p, s| p.join(s)))
}

/// A requirement from a searched project that an install satisfies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RuntimeUse {
    pub project: String,
    /// As written in the project file.
    pub requirement: String,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct InstalledRuntime {
    pub runtime: Runtime,
    pub version: String,
    /// What this entry is, e.g. "JDK (Eclipse Adoptium)", ".NET SDK", "NDK".
    pub label: String,
    /// How it got there, e.g. "nvm-windows", "python.org installer", "rustup".
    pub installed_by: String,
    pub path: String,
    /// Space used on disk; `null` if it could not be measured.
    #[ts(type = "number | null")]
    pub bytes: Option<u64>,
    /// Searched projects whose stated requirement this version meets.
    pub used_by: Vec<RuntimeUse>,
}

/// A project requirement that no installed version meets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UnmetRequirement {
    pub project: String,
    pub runtime: Runtime,
    pub requirement: String,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RuntimeInventory {
    /// Grouped by runtime, newest version first.
    pub installed: Vec<InstalledRuntime>,
    pub unmet: Vec<UnmetRequirement>,
    /// Distinct projects whose requirements were compared.
    #[ts(type = "number")]
    pub projects_read: u64,
    pub warnings: Vec<String>,
}

fn entry(
    runtime: Runtime,
    version: impl Into<String>,
    label: impl Into<String>,
    installed_by: &str,
    path: &Path,
) -> InstalledRuntime {
    InstalledRuntime {
        runtime,
        version: version.into(),
        label: label.into(),
        installed_by: installed_by.to_owned(),
        path: path.display().to_string(),
        bytes: None,
        used_by: Vec::new(),
    }
}

/// Subfolders of `dir` (not links), as `(name, path)`. Empty if `dir` cannot be read.
fn subdirs(dir: &Path) -> Vec<(String, PathBuf)> {
    read_dir(dir)
        .map(|entries| {
            entries
                .into_iter()
                .filter(|e| e.is_dir() && !e.is_reparse_point())
                .map(|e| {
                    let p = dir.join(&e.name);
                    (e.name, p)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn read_text(path: &Path, limit: u64) -> Option<String> {
    let md = fs::metadata(path).ok()?;
    (md.is_file() && md.len() <= limit)
        .then(|| fs::read_to_string(path).ok())
        .flatten()
}

/// `v22.4.0` / `3.12.4-win32` -> `22.4.0` / `3.12.4`.
fn dotted(s: &str) -> Option<String> {
    parse_version(s).map(|v| v.iter().map(u64::to_string).collect::<Vec<_>>().join("."))
}

fn node(loc: &Locations, out: &mut Vec<InstalledRuntime>) {
    let nvm = loc.nvm_home.clone().or_else(|| loc.under_roaming(&["nvm"]));
    let fnm = loc
        .fnm_dir
        .clone()
        .or_else(|| loc.under_local(&["fnm"]))
        .map(|p| p.join("node-versions"));
    let volta = loc
        .volta_home
        .clone()
        .or_else(|| loc.under_local(&["Volta"]))
        .map(|p| p.join("tools").join("image").join("node"));

    // (folder holding one subfolder per version, folder inside it that holds node.exe, manager)
    let managed = [
        (nvm, None, "nvm-windows"),
        (fnm, Some("installation"), "fnm"),
        (volta, None, "Volta"),
    ];
    for (root, inner, by) in managed {
        let Some(root) = root else { continue };
        for (name, p) in subdirs(&root) {
            let home = inner.map_or_else(|| p.clone(), |i| p.join(i));
            if let Some(v) = dotted(&name)
                && home.join("node.exe").is_file()
            {
                out.push(entry(Runtime::Node, v, "Node.js", by, &home));
            }
        }
    }
    for pf in &loc.program_files {
        let home = pf.join("nodejs");
        let exe = home.join("node.exe");
        if exe.is_file()
            && let Some(v) = probe_version(&exe)
        {
            out.push(entry(Runtime::Node, v, "Node.js", "installer", &home));
        }
    }
}

/// Run `exe --version` (fixed argument, no shell, 5 s limit) and read a version from it.
fn probe_version(exe: &Path) -> Option<String> {
    let mut cmd = Command::new(exe);
    cmd.arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = cmd.spawn().ok()?;
    let deadline = Instant::now() + VERSION_PROBE_LIMIT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let out = child.wait_with_output().ok()?;
    let head = &out.stdout[..out.stdout.len().min(64)];
    dotted(String::from_utf8_lossy(head).trim())
}

/// `PY_VERSION "3.12.4"` from an install's `include\patchlevel.h`.
fn patchlevel(dir: &Path) -> Option<String> {
    let text = read_text(&dir.join("include").join("patchlevel.h"), MAX_FILE_BYTES)?;
    text.lines()
        .find(|l| l.trim_start().starts_with("#define PY_VERSION "))
        .and_then(|l| l.split('"').nth(1))
        .and_then(dotted)
}

/// `Python312` -> `3.12`, `Python313-arm64` -> `3.13`.
fn python_folder_version(name: &str) -> Option<String> {
    let digits: String = name
        .strip_prefix("Python")?
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    (digits.len() >= 2).then(|| format!("{}.{}", &digits[..1], &digits[1..]))
}

fn python(loc: &Locations, out: &mut Vec<InstalledRuntime>) {
    let mut roots: Vec<(PathBuf, &str)> = Vec::new();
    if let Some(p) = loc.under_local(&["Programs", "Python"]) {
        roots.push((p, "python.org installer"));
    }
    roots.extend(
        loc.program_files
            .iter()
            .map(|p| (p.clone(), "python.org installer")),
    );
    if let Some(p) = loc.under_profile(&[".pyenv", "pyenv-win", "versions"]) {
        roots.push((p, "pyenv-win"));
    }
    if let Some(p) = loc.under_roaming(&["uv", "python"]) {
        roots.push((p, "uv"));
    }
    for (root, by) in roots {
        for (name, home) in subdirs(&root) {
            if !home.join("python.exe").is_file() {
                continue;
            }
            let version = match by {
                "pyenv-win" => dotted(&name),
                "uv" => name
                    .strip_prefix("cpython-")
                    .and_then(|r| r.split('-').next())
                    .and_then(dotted),
                _ => patchlevel(&home).or_else(|| python_folder_version(&name)),
            };
            if let Some(v) = version {
                out.push(entry(Runtime::Python, v, "Python", by, &home));
            }
        }
    }
}

/// `KEY="value"` lines of a JDK `release` file.
fn release_value(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let (k, v) = l.split_once('=')?;
        (k.trim() == key).then(|| v.trim().trim_matches('"').to_owned())
    })
}

fn java(loc: &Locations, out: &mut Vec<InstalledRuntime>) {
    const VENDOR_FOLDERS: &[&str] = &[
        "Java",
        "Eclipse Adoptium",
        "Eclipse Foundation",
        "Microsoft",
        "Zulu",
        "Amazon Corretto",
        "BellSoft",
        "Semeru",
    ];
    let mut homes: Vec<(PathBuf, &str)> = Vec::new();
    for pf in &loc.program_files {
        for v in VENDOR_FOLDERS {
            homes.extend(
                subdirs(&pf.join(v))
                    .into_iter()
                    .map(|(_, p)| (p, "installer")),
            );
        }
        for studio in [
            pf.join("Android").join("Android Studio").join("jbr"),
            pf.join("Android Studio").join("jbr"),
        ] {
            homes.push((studio, "Android Studio"));
        }
    }
    for (parts, by) in [
        (&[".jdks"][..], "IDE download"),
        (&[".gradle", "jdks"][..], "Gradle toolchain download"),
    ] {
        if let Some(root) = loc.under_profile(parts) {
            homes.extend(subdirs(&root).into_iter().map(|(_, p)| (p, by)));
        }
    }
    if let Some(h) = &loc.java_home {
        homes.push((h.clone(), "JAVA_HOME"));
    }
    for (home, by) in homes {
        if !home.join("bin").join("java.exe").is_file() {
            continue;
        }
        let Some(text) = read_text(&home.join("release"), MAX_FILE_BYTES) else {
            continue;
        };
        let Some(version) = release_value(&text, "JAVA_VERSION").filter(|v| !v.is_empty()) else {
            continue;
        };
        let kind = if home.join("bin").join("javac.exe").is_file() {
            "JDK"
        } else {
            "JRE"
        };
        let label = match release_value(&text, "IMPLEMENTOR").filter(|v| !v.is_empty()) {
            Some(vendor) => format!("{kind} ({vendor})"),
            None => kind.to_owned(),
        };
        out.push(entry(Runtime::Java, version, label, by, &home));
    }
}

fn dotnet(loc: &Locations, out: &mut Vec<InstalledRuntime>) {
    let mut roots: Vec<PathBuf> = loc.program_files.iter().map(|p| p.join("dotnet")).collect();
    roots.extend(loc.dotnet_root.clone());
    for root in roots {
        for (folder, what) in [
            (root.join("sdk"), ".NET SDK"),
            (
                root.join("shared").join("Microsoft.NETCore.App"),
                ".NET runtime",
            ),
        ] {
            for (name, p) in subdirs(&folder) {
                if parse_version(&name).is_some() {
                    out.push(entry(Runtime::DotNet, name, what, "installer", &p));
                }
            }
        }
    }
}

/// `pkg.rustc.version` from a rustup channel manifest: `"1.98.1 (abc 2026-01-01)"`.
fn manifest_rustc_version(text: &str) -> Option<String> {
    let mut in_rustc = false;
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            in_rustc = l == "[pkg.rustc]";
        } else if in_rustc && let Some(rest) = l.strip_prefix("version") {
            let v = rest
                .trim_start()
                .strip_prefix('=')?
                .trim()
                .trim_matches('"');
            return v.split_whitespace().next().map(str::to_owned);
        }
    }
    None
}

fn rust(loc: &Locations, out: &mut Vec<InstalledRuntime>) {
    let Some(home) = loc
        .rustup_home
        .clone()
        .or_else(|| loc.under_profile(&[".rustup"]))
    else {
        return;
    };
    for (name, p) in subdirs(&home.join("toolchains")) {
        let manifest = p
            .join("lib")
            .join("rustlib")
            .join("multirust-channel-manifest.toml");
        let version = read_text(&manifest, MANIFEST_MAX_BYTES)
            .and_then(|t| manifest_rustc_version(&t))
            .or_else(|| dotted(&name))
            .unwrap_or_else(|| "unknown".to_owned());
        out.push(entry(
            Runtime::Rust,
            version,
            format!("Rust toolchain ({name})"),
            "rustup",
            &p,
        ));
    }
}

fn go(loc: &Locations, out: &mut Vec<InstalledRuntime>) {
    let mut homes: Vec<(PathBuf, &str)> = loc
        .program_files
        .iter()
        .map(|p| (p.join("Go"), "installer"))
        .collect();
    if let Some(g) = &loc.goroot {
        homes.push((g.clone(), "GOROOT"));
    }
    if let Some(root) = loc.under_profile(&["sdk"]) {
        homes.extend(
            subdirs(&root)
                .into_iter()
                .filter(|(n, _)| n.starts_with("go"))
                .map(|(_, p)| (p, "go install golang.org/dl")),
        );
    }
    let gopath = loc.gopath.clone().or_else(|| loc.under_profile(&["go"]));
    if let Some(root) = gopath.map(|g| g.join("pkg").join("mod").join("golang.org")) {
        homes.extend(
            subdirs(&root)
                .into_iter()
                .filter(|(n, _)| n.starts_with("toolchain@"))
                .map(|(_, p)| (p, "Go toolchain download")),
        );
    }
    for (home, by) in homes {
        if !home.join("bin").join("go.exe").is_file() {
            continue;
        }
        let version =
            read_text(&home.join("VERSION"), 4096).and_then(|t| t.lines().next().and_then(dotted));
        if let Some(v) = version {
            out.push(entry(Runtime::Go, v, "Go", by, &home));
        }
    }
}

fn android(loc: &Locations, out: &mut Vec<InstalledRuntime>) {
    let Some(sdk) = loc
        .android_home
        .clone()
        .or_else(|| loc.under_local(&["Android", "Sdk"]))
    else {
        return;
    };
    for (folder, what) in [
        ("platforms", "Android platform"),
        ("build-tools", "Build tools"),
        ("ndk", "NDK"),
        ("cmake", "CMake"),
        ("system-images", "Emulator system image"),
    ] {
        for (name, p) in subdirs(&sdk.join(folder)) {
            out.push(entry(Runtime::AndroidSdk, name, what, "Android SDK", &p));
        }
    }
}

fn folder_bytes(path: &Path) -> Option<u64> {
    let opts = ScanOptions {
        threads: 2,
        largest_files: 0,
        ..ScanOptions::default()
    };
    scan(path, &opts, &ScanControl::new())
        .ok()
        .map(|t| t.stats.total_bytes)
}

fn is_minimum(source: &str) -> bool {
    source.starts_with("Cargo.toml") || source.starts_with("go.mod")
}

/// Find installed runtimes under `loc` and relate them to the requirements in `projects`.
/// Read-only.
pub fn inventory_runtimes(loc: &Locations, projects: &[Project]) -> RuntimeInventory {
    let mut found = Vec::new();
    node(loc, &mut found);
    python(loc, &mut found);
    java(loc, &mut found);
    dotnet(loc, &mut found);
    rust(loc, &mut found);
    go(loc, &mut found);
    android(loc, &mut found);

    let mut seen = BTreeSet::new();
    let mut installed: Vec<InstalledRuntime> = found
        .into_iter()
        .filter(|i| seen.insert(i.path.to_lowercase()))
        .collect();
    installed.sort_by_cached_key(|i| {
        (
            i.runtime,
            std::cmp::Reverse(parse_version(&i.version)),
            i.path.to_lowercase(),
        )
    });
    for i in &mut installed {
        i.bytes = folder_bytes(Path::new(&i.path));
    }

    let mut unmet = Vec::new();
    let mut project_paths = BTreeSet::new();
    for p in projects {
        if !project_paths.insert(p.path.to_lowercase()) {
            continue;
        }
        for req in &p.runtimes {
            let minimum = is_minimum(&req.source);
            // A requirement that is not understood is skipped, never reported as unmet.
            if satisfies(&req.version, "0", minimum) == Satisfies::Unknown {
                continue;
            }
            let (mut met, mut unknown) = (false, false);
            for i in installed.iter_mut().filter(|i| i.runtime == req.runtime) {
                match satisfies(&req.version, &i.version, minimum) {
                    Satisfies::Yes => {
                        met = true;
                        i.used_by.push(RuntimeUse {
                            project: p.path.clone(),
                            requirement: req.version.clone(),
                            source: req.source.clone(),
                        });
                    }
                    Satisfies::Unknown => unknown = true,
                    Satisfies::No => {}
                }
            }
            if !met && !unknown {
                unmet.push(UnmetRequirement {
                    project: p.path.clone(),
                    runtime: req.runtime,
                    requirement: req.version.clone(),
                    source: req.source.clone(),
                });
            }
        }
    }

    let tracing_total = installed.len();
    tracing::info!(
        installed = tracing_total,
        unmet = unmet.len(),
        projects = project_paths.len(),
        "runtime inventory finished"
    );
    RuntimeInventory {
        installed,
        unmet,
        projects_read: project_paths.len() as u64,
        warnings: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_folder_names() {
        assert_eq!(python_folder_version("Python312").as_deref(), Some("3.12"));
        assert_eq!(
            python_folder_version("Python313-arm64").as_deref(),
            Some("3.13")
        );
        assert_eq!(python_folder_version("Python3"), None);
        assert_eq!(python_folder_version("Tools"), None);
    }

    #[test]
    fn reads_rustc_version_from_manifest() {
        let m = "[pkg.cargo]\nversion = \"0.99.0 (x 2026)\"\n[pkg.rustc]\nversion = \"1.98.1 (abc 2026-01-01)\"\n";
        assert_eq!(manifest_rustc_version(m).as_deref(), Some("1.98.1"));
        assert_eq!(
            manifest_rustc_version("[pkg.cargo]\nversion = \"1\"\n"),
            None
        );
    }

    #[test]
    fn reads_release_file() {
        let r = "IMPLEMENTOR=\"Eclipse Adoptium\"\nJAVA_VERSION=\"21.0.4\"\n";
        assert_eq!(release_value(r, "JAVA_VERSION").as_deref(), Some("21.0.4"));
        assert_eq!(
            release_value(r, "IMPLEMENTOR").as_deref(),
            Some("Eclipse Adoptium")
        );
        assert_eq!(release_value(r, "NOPE"), None);
    }
}
