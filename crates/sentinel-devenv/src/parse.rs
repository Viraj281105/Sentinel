//! Reading project files. Files are only read and parsed, never executed, and anything
//! larger than [`MAX_FILE_BYTES`] is ignored.

use std::fs;
use std::path::Path;

use crate::model::{Runtime, RuntimeRequirement};

pub(crate) const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// What a project's own files say about it.
#[derive(Default)]
pub(crate) struct Facts {
    pub name: Option<String>,
    pub runtimes: Vec<RuntimeRequirement>,
    pub warnings: Vec<String>,
}

fn read_limited(path: &Path, facts: &mut Facts) -> Option<String> {
    let file = path.file_name()?.to_string_lossy().into_owned();
    match fs::metadata(path) {
        Ok(md) if md.len() > MAX_FILE_BYTES => {
            facts
                .warnings
                .push(format!("{file} is larger than 1 MB and was not read"));
            None
        }
        Ok(_) => match fs::read_to_string(path) {
            Ok(s) => Some(s),
            Err(e) => {
                facts
                    .warnings
                    .push(format!("{file} could not be read: {e}"));
                None
            }
        },
        Err(_) => None,
    }
}

fn requirement(runtime: Runtime, version: &str, source: &str) -> Option<RuntimeRequirement> {
    let v = version.trim();
    // Guard against absurd values from a malformed or hostile file.
    (!v.is_empty() && v.len() <= 64).then(|| RuntimeRequirement {
        runtime,
        version: v.to_owned(),
        source: source.to_owned(),
    })
}

fn bounded_name(s: &str) -> Option<String> {
    let s = s.trim();
    (!s.is_empty() && s.len() <= 214).then(|| s.to_owned())
}

pub(crate) fn package_json(dir: &Path, facts: &mut Facts) {
    let Some(text) = read_limited(&dir.join("package.json"), facts) else {
        return;
    };
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(v) => {
            if facts.name.is_none() {
                facts.name = v["name"].as_str().and_then(bounded_name);
            }
            if let Some(r) = v["engines"]["node"]
                .as_str()
                .and_then(|n| requirement(Runtime::Node, n, "package.json engines.node"))
            {
                facts.runtimes.push(r);
            }
        }
        Err(e) => facts
            .warnings
            .push(format!("package.json is not valid JSON: {e}")),
    }
}

pub(crate) fn cargo_toml(dir: &Path, facts: &mut Facts) {
    let Some(text) = read_limited(&dir.join("Cargo.toml"), facts) else {
        return;
    };
    match text.parse::<toml::Table>() {
        Ok(t) => {
            let pkg = t.get("package").and_then(|p| p.as_table());
            if facts.name.is_none() {
                facts.name = pkg
                    .and_then(|p| p.get("name"))
                    .and_then(|n| n.as_str())
                    .and_then(bounded_name);
            }
            if let Some(r) = pkg
                .and_then(|p| p.get("rust-version"))
                .and_then(|v| v.as_str())
                .and_then(|v| requirement(Runtime::Rust, v, "Cargo.toml rust-version"))
            {
                facts.runtimes.push(r);
            }
        }
        Err(e) => facts
            .warnings
            .push(format!("Cargo.toml could not be parsed: {e}")),
    }
}

pub(crate) fn pyproject(dir: &Path, facts: &mut Facts) {
    let Some(text) = read_limited(&dir.join("pyproject.toml"), facts) else {
        return;
    };
    match text.parse::<toml::Table>() {
        Ok(t) => {
            let project = t.get("project").and_then(|p| p.as_table());
            let poetry = t
                .get("tool")
                .and_then(|x| x.get("poetry"))
                .and_then(|p| p.as_table());
            if facts.name.is_none() {
                facts.name = project
                    .and_then(|p| p.get("name"))
                    .or_else(|| poetry.and_then(|p| p.get("name")))
                    .and_then(|n| n.as_str())
                    .and_then(bounded_name);
            }
            if let Some(r) = project
                .and_then(|p| p.get("requires-python"))
                .and_then(|v| v.as_str())
                .and_then(|v| requirement(Runtime::Python, v, "pyproject.toml requires-python"))
            {
                facts.runtimes.push(r);
            }
        }
        Err(e) => facts
            .warnings
            .push(format!("pyproject.toml could not be parsed: {e}")),
    }
}

pub(crate) fn go_mod(dir: &Path, facts: &mut Facts) {
    let Some(text) = read_limited(&dir.join("go.mod"), facts) else {
        return;
    };
    for line in text.lines().map(str::trim) {
        if let Some(module) = line.strip_prefix("module ")
            && facts.name.is_none()
        {
            facts.name = bounded_name(module);
        }
        if let Some(v) = line.strip_prefix("go ")
            && let Some(r) = requirement(Runtime::Go, v, "go.mod go directive")
        {
            facts.runtimes.push(r);
        }
    }
}

/// Single-line version files: `.nvmrc`, `.node-version`, `.python-version`,
/// `rust-toolchain`.
pub(crate) fn version_file(dir: &Path, file: &str, runtime: Runtime, facts: &mut Facts) {
    let Some(text) = read_limited(&dir.join(file), facts) else {
        return;
    };
    if let Some(r) = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .and_then(|l| requirement(runtime, l, file))
    {
        facts.runtimes.push(r);
    }
}

pub(crate) fn rust_toolchain_toml(dir: &Path, facts: &mut Facts) {
    let Some(text) = read_limited(&dir.join("rust-toolchain.toml"), facts) else {
        return;
    };
    match text.parse::<toml::Table>() {
        Ok(t) => {
            if let Some(r) = t
                .get("toolchain")
                .and_then(|x| x.get("channel"))
                .and_then(|c| c.as_str())
                .and_then(|c| requirement(Runtime::Rust, c, "rust-toolchain.toml"))
            {
                facts.runtimes.push(r);
            }
        }
        Err(e) => facts
            .warnings
            .push(format!("rust-toolchain.toml could not be parsed: {e}")),
    }
}
