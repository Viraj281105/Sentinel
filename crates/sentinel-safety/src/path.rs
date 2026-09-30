use std::ffi::OsStr;
use std::fs;
use std::os::windows::fs::MetadataExt;
use std::path::{Component, Path, PathBuf, Prefix};

use crate::error::SafetyError;

const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

const RESERVED_NAMES: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Case-fold a path component for comparison (NTFS is case-insensitive).
///
/// Uses Unicode lowercase as an approximation of the NTFS upcase table; the
/// approximation errs toward treating more names as equal, which is the safe
/// direction for protection checks.
pub(crate) fn fold(s: &OsStr) -> String {
    s.to_string_lossy().to_lowercase()
}

/// True if `path` is `root` or lies beneath it. Component-wise and case-insensitive;
/// never a string prefix test.
pub fn is_within(path: &Path, root: &Path) -> bool {
    let mut p = path.components();
    for r in root.components() {
        match p.next() {
            Some(c) if fold(c.as_os_str()) == fold(r.as_os_str()) => {}
            _ => return false,
        }
    }
    true
}

/// True if `path` itself (not what it points to) is a reparse point
/// (symlink, junction, cloud placeholder, ...).
pub fn is_reparse_point(path: &Path) -> Result<bool, SafetyError> {
    let md = fs::symlink_metadata(path).map_err(|e| SafetyError::io(path, e))?;
    Ok(md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
}

/// Remove the `\\?\` verbatim prefix when it is a plain disk path.
fn strip_verbatim(path: &Path) -> PathBuf {
    let mut comps = path.components();
    if let Some(Component::Prefix(p)) = comps.next()
        && let Prefix::VerbatimDisk(letter) = p.kind()
    {
        let mut out = PathBuf::from(format!("{}:", char::from(letter)));
        out.push(Component::RootDir);
        out.extend(comps.filter(|c| !matches!(c, Component::RootDir)));
        return out;
    }
    path.to_path_buf()
}

/// Reject path shapes Sentinel refuses to reason about.
///
/// Rejected: relative paths; `.` and `..`; non-disk prefixes (UNC, device namespace,
/// non-disk verbatim); alternate data streams; reserved device names; trailing dot or
/// space; wildcard/illegal characters; non-Unicode names.
pub(crate) fn check_lexical(path: &Path) -> Result<(), SafetyError> {
    let mut saw_prefix = false;
    let mut saw_root = false;
    for c in path.components() {
        match c {
            Component::Prefix(p) => {
                if !matches!(p.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)) {
                    return Err(SafetyError::invalid(
                        path,
                        "network, device and non-disk paths are not supported",
                    ));
                }
                saw_prefix = true;
            }
            Component::RootDir => saw_root = true,
            Component::CurDir | Component::ParentDir => {
                return Err(SafetyError::invalid(
                    path,
                    "dot and dot-dot components are not allowed",
                ));
            }
            Component::Normal(name) => check_name(path, name)?,
        }
    }
    if !(saw_prefix && saw_root) {
        return Err(SafetyError::invalid(path, "path must be absolute"));
    }
    Ok(())
}

fn check_name(path: &Path, name: &OsStr) -> Result<(), SafetyError> {
    let Some(s) = name.to_str() else {
        return Err(SafetyError::invalid(path, "name is not valid Unicode"));
    };
    if s.contains(':') {
        return Err(SafetyError::invalid(
            path,
            "alternate data streams are not allowed",
        ));
    }
    if s.chars()
        .any(|c| c.is_control() || matches!(c, '<' | '>' | '"' | '|' | '?' | '*' | '/'))
    {
        return Err(SafetyError::invalid(
            path,
            "name contains illegal characters",
        ));
    }
    if s.ends_with('.') || s.ends_with(' ') {
        return Err(SafetyError::invalid(
            path,
            "names ending in a dot or space are not allowed",
        ));
    }
    let stem = s.split('.').next().unwrap_or(s).to_lowercase();
    if RESERVED_NAMES.contains(&stem.as_str()) {
        return Err(SafetyError::invalid(path, "reserved device name"));
    }
    Ok(())
}

/// An absolute, existing, fully resolved path in plain-disk form (no `\\?\`).
///
/// Only constructible inside this crate, so holding one proves the path passed
/// lexical checks and existed when it was built.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CanonicalPath(PathBuf);

impl CanonicalPath {
    /// Resolve `path`, **following** symlinks/junctions and expanding 8.3 names.
    ///
    /// Use this for inspecting where something really is (e.g. building protected
    /// roots). Destructive validation uses [`crate::Policy::validate`], which refuses
    /// links instead of following them.
    pub fn resolve(path: &Path) -> Result<Self, SafetyError> {
        check_lexical(path)?;
        let canon = fs::canonicalize(path).map_err(|e| SafetyError::io(path, e))?;
        let plain = strip_verbatim(&canon);
        check_lexical(&plain)?;
        Ok(Self(plain))
    }

    /// Wrap a path already verified by this crate's validation.
    pub(crate) fn from_verified(path: PathBuf) -> Self {
        Self(path)
    }

    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for CanonicalPath {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

/// Best-effort normalization for paths that may not exist (protected-root registration).
pub(crate) fn normalize_loose(path: &Path) -> PathBuf {
    match fs::canonicalize(path) {
        Ok(c) => strip_verbatim(&c),
        Err(_) => strip_verbatim(path),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn within_is_component_wise_not_string_prefix() {
        assert!(is_within(Path::new(r"C:\a\b"), Path::new(r"C:\a")));
        assert!(!is_within(Path::new(r"C:\ab"), Path::new(r"C:\a")));
        assert!(is_within(Path::new(r"C:\A\B"), Path::new(r"c:\a")));
        assert!(is_within(Path::new(r"C:\a"), Path::new(r"C:\a")));
    }

    #[test]
    fn lexical_rejections() {
        for bad in [
            r"relative\path",
            r"C:\a\..\b",
            r"C:\a\file.txt:stream",
            r"C:\a\CON",
            r"C:\a\nul.txt",
            r"C:\a\name.",
            r"C:\a\name ",
            r"C:\a\b*c",
            r"\\server\share\x",
            r"\\.\PhysicalDrive0",
            r"\\?\GLOBALROOT\Device\x",
            r"C:rel",
        ] {
            assert!(
                check_lexical(Path::new(bad)).is_err(),
                "{bad} should be rejected"
            );
        }
        assert!(check_lexical(Path::new(r"C:\ok\name.txt")).is_ok());
        assert!(check_lexical(Path::new(r"\\?\C:\ok\name.txt")).is_ok());
    }

    #[test]
    fn strips_verbatim_disk_prefix() {
        assert_eq!(
            strip_verbatim(Path::new(r"\\?\C:\a\b")),
            PathBuf::from(r"C:\a\b")
        );
    }
}
