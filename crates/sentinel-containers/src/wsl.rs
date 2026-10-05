//! WSL distributions from the registry, with the size of each virtual disk.
//!
//! Nothing is run and no virtual disk is opened: the size is the `.vhdx` file's size on
//! the host, which only grows until someone compacts it. Compacting, moving or removing a
//! distribution is never offered, since the disk holds the distribution's whole filesystem.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Disk file names WSL and Docker Desktop use inside a distribution's folder.
const DISK_NAMES: &[&str] = &["ext4.vhdx", "docker_data.vhdx"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WslDisk {
    pub path: String,
    #[ts(type = "number")]
    pub bytes: u64,
    /// When the disk file last changed.
    #[ts(type = "number | null")]
    pub modified_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WslDistro {
    pub name: String,
    /// WSL version, 1 or 2.
    #[ts(type = "number | null")]
    pub version: Option<u32>,
    pub is_default: bool,
    /// Created and managed by Docker Desktop.
    pub docker_desktop: bool,
    pub base_path: Option<String>,
    /// Virtual disks found for it (Docker Desktop has a system disk and a data disk).
    pub disks: Vec<WslDisk>,
    #[ts(type = "number")]
    pub disk_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WslInventory {
    /// Largest disk first.
    pub distros: Vec<WslDistro>,
    #[ts(type = "number")]
    pub total_disk_bytes: u64,
    pub warnings: Vec<String>,
}

/// A distribution as the registry records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawDistro {
    pub name: String,
    pub base_path: Option<String>,
    pub version: Option<u32>,
    pub is_default: bool,
}

/// `\\?\C:\x` -> `C:\x`.
fn clean_path(p: &str) -> PathBuf {
    PathBuf::from(p.trim().strip_prefix(r"\\?\").unwrap_or(p.trim()))
}

fn disk_in(dir: &Path) -> Vec<WslDisk> {
    DISK_NAMES
        .iter()
        .filter_map(|name| {
            let p = dir.join(name);
            let md = std::fs::metadata(&p)
                .ok()
                .filter(std::fs::Metadata::is_file)?;
            Some(WslDisk {
                path: p.display().to_string(),
                bytes: md.len(),
                modified_ms: md
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .and_then(|d| i64::try_from(d.as_millis()).ok()),
            })
        })
        .collect()
}

/// Disks of one distribution: those in its folder and, for Docker Desktop, the data disk
/// in the `disk` folder next to its `main` folder.
fn find_disks(base: &Path, docker_desktop: bool) -> Vec<WslDisk> {
    let mut disks = disk_in(base);
    if docker_desktop && let Some(sibling) = base.parent().map(|p| p.join("disk")) {
        disks.extend(disk_in(&sibling));
    }
    disks
}

/// Turn registry records into the inventory. Reads file metadata only.
pub fn interpret_wsl(raw: &[RawDistro], warnings: Vec<String>) -> WslInventory {
    let mut distros: Vec<WslDistro> = raw
        .iter()
        .filter(|r| !r.name.trim().is_empty())
        .map(|r| {
            let base = r.base_path.as_deref().map(clean_path);
            let docker_desktop = r.name.starts_with("docker-desktop");
            let disks = base
                .as_deref()
                .map(|b| find_disks(b, docker_desktop))
                .unwrap_or_default();
            WslDistro {
                name: r.name.trim().to_owned(),
                version: r.version,
                is_default: r.is_default,
                docker_desktop,
                base_path: base.as_ref().map(|b| b.display().to_string()),
                disk_bytes: disks.iter().map(|d| d.bytes).sum(),
                disks,
            }
        })
        .collect();
    distros.sort_by(|a, b| {
        b.disk_bytes
            .cmp(&a.disk_bytes)
            .then_with(|| a.name.cmp(&b.name))
    });
    let total_disk_bytes = distros.iter().map(|d| d.disk_bytes).sum();
    WslInventory {
        distros,
        total_disk_bytes,
        warnings,
    }
}

#[cfg(windows)]
pub fn inspect_wsl() -> WslInventory {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};

    let mut warnings = Vec::new();
    let mut raw = Vec::new();
    let path = r"Software\Microsoft\Windows\CurrentVersion\Lxss";
    match RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(path, KEY_READ) {
        Ok(root) => {
            let default: String = root.get_value("DefaultDistribution").unwrap_or_default();
            for guid in root.enum_keys().flatten().take(500) {
                let Ok(k) = root.open_subkey_with_flags(&guid, KEY_READ) else {
                    continue;
                };
                let Ok(name) = k.get_value::<String, _>("DistributionName") else {
                    continue;
                };
                raw.push(RawDistro {
                    name,
                    base_path: k.get_value::<String, _>("BasePath").ok(),
                    version: k.get_value::<u32, _>("Version").ok(),
                    is_default: !default.is_empty() && guid.eq_ignore_ascii_case(&default),
                });
            }
        }
        // No Lxss key simply means WSL has no distributions for this user.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => warnings.push(format!(
            "The WSL distribution list could not be opened: {e}"
        )),
    }
    let inv = interpret_wsl(&raw, warnings);
    tracing::info!(distros = inv.distros.len(), "wsl inspection finished");
    inv
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn raw(name: &str, base: Option<&Path>, default: bool) -> RawDistro {
        RawDistro {
            name: name.to_owned(),
            base_path: base.map(|b| format!(r"\\?\{}", b.display())),
            version: Some(2),
            is_default: default,
        }
    }

    #[test]
    fn sizes_each_distribution_disk_largest_first() {
        let dir = tempfile::tempdir().unwrap();
        let ubuntu = dir.path().join("ubuntu");
        let dd = dir.path().join("docker").join("main");
        let dd_data = dir.path().join("docker").join("disk");
        for d in [&ubuntu, &dd, &dd_data] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(ubuntu.join("ext4.vhdx"), vec![0u8; 5000]).unwrap();
        std::fs::write(dd.join("ext4.vhdx"), vec![0u8; 100]).unwrap();
        std::fs::write(dd_data.join("docker_data.vhdx"), vec![0u8; 9000]).unwrap();

        let inv = interpret_wsl(
            &[
                raw("Ubuntu", Some(&ubuntu), true),
                raw("docker-desktop", Some(&dd), false),
                raw("Ghost", Some(&dir.path().join("missing")), false),
                raw("  ", None, false),
            ],
            vec![],
        );
        assert_eq!(inv.distros.len(), 3, "nameless record dropped");
        // Docker Desktop's data disk lives in the sibling `disk` folder.
        assert_eq!(inv.distros[0].name, "docker-desktop");
        assert!(inv.distros[0].docker_desktop);
        assert_eq!(inv.distros[0].disks.len(), 2);
        assert_eq!(inv.distros[0].disk_bytes, 9100);
        assert_eq!(inv.distros[1].name, "Ubuntu");
        assert!(inv.distros[1].is_default && !inv.distros[1].docker_desktop);
        assert_eq!(inv.distros[1].disk_bytes, 5000);
        assert!(inv.distros[1].disks[0].modified_ms.is_some());
        assert!(
            inv.distros[2].disks.is_empty(),
            "missing folder: listed, no size"
        );
        assert_eq!(inv.total_disk_bytes, 14_100);
    }

    #[test]
    fn a_normal_distribution_does_not_borrow_a_sibling_disk() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let sibling = dir.path().join("disk");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&sibling).unwrap();
        std::fs::write(sibling.join("docker_data.vhdx"), b"x").unwrap();
        let inv = interpret_wsl(&[raw("Debian", Some(&a), false)], vec![]);
        assert!(inv.distros[0].disks.is_empty());
    }

    #[test]
    fn strips_the_extended_path_prefix() {
        assert_eq!(clean_path(r"\\?\C:\a\b"), PathBuf::from(r"C:\a\b"));
        assert_eq!(clean_path(r"C:\a"), PathBuf::from(r"C:\a"));
    }
}
