use std::path::{Path, PathBuf};

use crate::error::SafetyError;
use crate::known_folders::{Known, path_of};
use crate::path::{fold, is_within, normalize_loose};

/// Directory or file names that are protected wherever they appear.
const PROTECTED_NAMES: &[(&str, &str)] = &[
    (".git", "Git repository data"),
    (
        ".sentinel-quarantine",
        "Sentinel quarantine (restorable items)",
    ),
    (".ssh", "SSH keys and configuration"),
    (".gnupg", "GPG keys"),
    (".aws", "cloud credentials"),
    (".azure", "cloud credentials"),
    (".kube", "Kubernetes credentials"),
    (".npmrc", "package-manager credentials"),
    (".pypirc", "package-manager credentials"),
    (".netrc", "network credentials"),
    ("id_rsa", "private key"),
    ("id_ecdsa", "private key"),
    ("id_ed25519", "private key"),
];

/// File extensions (of the final path component) that are protected.
const PROTECTED_EXTENSIONS: &[(&str, &str)] = &[
    ("vhdx", "virtual disk (WSL distribution or Docker data)"),
    ("vhd", "virtual disk"),
    ("kdbx", "password database"),
    ("pfx", "certificate private key"),
    ("p12", "certificate private key"),
    ("ppk", "private key"),
    ("pem", "key or certificate"),
];

#[derive(Debug, Clone)]
struct Root {
    path: PathBuf,
    reason: String,
    exceptions: Vec<PathBuf>,
}

/// The set of locations that must never be acted upon.
///
/// Built-in name and extension rules are always active. Path roots are added by the
/// caller (see [`ProtectedSet::from_system`]). Protection always wins: nothing else in
/// the system can override it, except explicit `exceptions` declared at registration.
#[derive(Debug, Clone, Default)]
pub struct ProtectedSet {
    roots: Vec<Root>,
}

impl ProtectedSet {
    /// Only the built-in name/extension rules. Intended for tests and for building up
    /// a set explicitly.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registered path roots with the reason each is protected. The built-in name and
    /// extension rules apply everywhere and are not listed here.
    pub fn roots(&self) -> impl Iterator<Item = (&Path, &str)> {
        self.roots
            .iter()
            .map(|r| (r.path.as_path(), r.reason.as_str()))
    }

    /// Protect `path` and everything beneath it. The path need not exist yet.
    pub fn add_root(&mut self, path: &Path, reason: &str) {
        self.add_root_with_exceptions(path, reason, &[]);
    }

    /// Protect `path`, except for the listed sub-locations (e.g. `C:\Windows\Temp`).
    pub fn add_root_with_exceptions(&mut self, path: &Path, reason: &str, exceptions: &[PathBuf]) {
        self.roots.push(Root {
            path: normalize_loose(path),
            reason: reason.to_owned(),
            exceptions: exceptions.iter().map(|e| normalize_loose(e)).collect(),
        });
    }

    /// The default protection set for the current machine, derived from shell known
    /// folders (not environment variables) plus the OneDrive roots.
    pub fn from_system() -> Self {
        let mut set = Self::new();

        for (folder, reason) in [
            (Known::Desktop, "Desktop"),
            (Known::Documents, "Documents"),
            (Known::Downloads, "Downloads"),
            (Known::Pictures, "Pictures"),
            (Known::Videos, "Videos"),
            (Known::Music, "Music"),
            (Known::ProgramFiles, "installed applications"),
            (Known::ProgramFilesX86, "installed applications"),
        ] {
            if let Some(p) = path_of(folder) {
                set.add_root(&p, reason);
            }
        }

        // Folder redirection (e.g. Documents moved to OneDrive) leaves the conventional
        // profile folders behind, often still holding files. Protect both locations.
        if let Some(profile) = path_of(Known::Profile) {
            for name in [
                "Desktop",
                "Documents",
                "Downloads",
                "Pictures",
                "Videos",
                "Music",
            ] {
                set.add_root(&profile.join(name), name);
            }
        }

        if let Some(win) = path_of(Known::Windows) {
            let exceptions = [
                win.join("Temp"),
                win.join("SoftwareDistribution").join("Download"),
                win.join("Minidump"),
            ];
            set.add_root_with_exceptions(&win, "Windows system files", &exceptions);
        }

        if let Some(roaming) = path_of(Known::RoamingAppData) {
            for sub in [
                r"Microsoft\Credentials",
                r"Microsoft\Vault",
                r"Microsoft\Protect",
                r"Microsoft\SystemCertificates",
                r"Mozilla",
            ] {
                set.add_root(&roaming.join(sub), "credential store or browser profile");
            }
        }
        if let Some(local) = path_of(Known::LocalAppData) {
            for sub in [
                r"Microsoft\Credentials",
                r"Microsoft\Vault",
                r"Google\Chrome\User Data",
                r"Microsoft\Edge\User Data",
                r"BraveSoftware",
                r"Mozilla",
                r"Packages",
            ] {
                set.add_root(&local.join(sub), "credential store or application data");
            }
            set.add_root(&local.join("Docker"), "Docker data");
        }

        for var in ["OneDrive", "OneDriveConsumer", "OneDriveCommercial"] {
            if let Some(p) = std::env::var_os(var) {
                let p = PathBuf::from(p);
                if p.is_absolute() {
                    set.add_root(&p, "OneDrive");
                }
            }
        }
        set
    }

    /// Fail if `path` is, is inside, or contains a protected location.
    pub(crate) fn check(&self, path: &Path) -> Result<(), SafetyError> {
        self.check_inner(path, true)
    }

    /// Like [`check`](Self::check) but tolerates protected locations *beneath* `path`.
    /// Used for provider roots, whose protected children are guarded when individual
    /// targets are validated.
    pub(crate) fn check_location(&self, path: &Path) -> Result<(), SafetyError> {
        self.check_inner(path, false)
    }

    fn check_inner(&self, path: &Path, reject_containing: bool) -> Result<(), SafetyError> {
        for c in path.components() {
            let name = fold(c.as_os_str());
            if let Some((_, why)) = PROTECTED_NAMES.iter().find(|(n, _)| *n == name) {
                return Err(protected(path, why));
            }
            if name == ".env" || name.starts_with(".env.") {
                return Err(protected(path, "environment file that may hold secrets"));
            }
        }
        if let Some(ext) = path.extension() {
            let ext = fold(ext);
            if let Some((_, why)) = PROTECTED_EXTENSIONS.iter().find(|(e, _)| *e == ext) {
                return Err(protected(path, why));
            }
        }

        for root in &self.roots {
            if is_within(path, &root.path) {
                if root.exceptions.iter().any(|e| is_within(path, e)) {
                    continue;
                }
                return Err(protected(path, &root.reason));
            }
            if reject_containing && is_within(&root.path, path) {
                return Err(SafetyError::ContainsProtected {
                    path: path.to_path_buf(),
                    protected: root.path.clone(),
                    reason: root.reason.clone(),
                });
            }
        }
        Ok(())
    }
}

fn protected(path: &Path, reason: &str) -> SafetyError {
    SafetyError::Protected {
        path: path.to_path_buf(),
        reason: reason.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_rules_apply_anywhere_in_path() {
        let set = ProtectedSet::new();
        for p in [
            r"C:\proj\.git\objects\ab",
            r"C:\Proj\.GIT",
            r"C:\u\.ssh\config",
            r"C:\x\.env",
            r"C:\x\.env.local",
            r"C:\x\disk.VHDX",
            r"C:\x\keys.kdbx",
            r"D:\.sentinel-quarantine\op-1\file",
        ] {
            assert!(set.check(Path::new(p)).is_err(), "{p}");
        }
        assert!(set.check(Path::new(r"C:\x\.environment")).is_ok());
        assert!(set.check(Path::new(r"C:\x\readme.txt")).is_ok());
    }

    #[test]
    fn root_protects_subtree_and_ancestors_but_not_siblings() {
        let mut set = ProtectedSet::new();
        set.add_root(Path::new(r"C:\Users\u\Documents"), "Documents");
        assert!(set.check(Path::new(r"C:\Users\u\Documents")).is_err());
        assert!(set.check(Path::new(r"C:\Users\u\documents\a\b")).is_err());
        assert!(matches!(
            set.check(Path::new(r"C:\Users\u")),
            Err(SafetyError::ContainsProtected { .. })
        ));
        assert!(matches!(
            set.check(Path::new(r"C:\")),
            Err(SafetyError::ContainsProtected { .. })
        ));
        assert!(set.check(Path::new(r"C:\Users\u\Documents2")).is_ok());
        assert!(
            set.check(Path::new(r"C:\Users\u\AppData\Local\Temp"))
                .is_ok()
        );
    }

    #[test]
    fn exceptions_carve_out_of_a_root() {
        let mut set = ProtectedSet::new();
        set.add_root_with_exceptions(
            Path::new(r"C:\Windows"),
            "system",
            &[PathBuf::from(r"C:\Windows\Temp")],
        );
        assert!(set.check(Path::new(r"C:\Windows\Temp\a.tmp")).is_ok());
        assert!(set.check(Path::new(r"C:\Windows\System32")).is_err());
        assert!(set.check(Path::new(r"C:\Windows")).is_err());
    }
}
