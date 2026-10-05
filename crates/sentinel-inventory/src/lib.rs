//! Read-only inventory of installed desktop software.
//!
//! Source: the three Windows "Uninstall" registry locations (machine 64-bit, machine
//! 32-bit view, current user). Only display metadata is read. Uninstall and modify command
//! lines are never read or returned, and nothing is written or run.
//!
//! Microsoft Store (packaged, MSIX) apps are read from the current user's package
//! repository in the registry, again without running anything. That repository lists
//! only packages registered for this user, and gives no publisher name or size
//! (`WindowsApps` is not readable). Winget is not asked: it would mean running a package
//! manager. Portable software that registers nothing is invisible to both sources.
//!
//! Entries are skipped when they are not user-facing software: no display name, marked
//! `SystemComponent`, a patch or update (`ParentKeyName`, `ReleaseType`).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Scope {
    /// Installed for all users (64-bit view).
    Machine,
    /// Installed for all users by a 32-bit installer.
    Machine32,
    /// Installed for the current user only.
    User,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct InstalledSoftware {
    pub name: String,
    pub version: Option<String>,
    pub publisher: Option<String>,
    pub scope: Scope,
    /// `YYYY-MM-DD` as the installer recorded it; installers often leave it out.
    pub install_date: Option<String>,
    /// Size the installer reported. Many installers report none or a rough figure.
    #[ts(type = "number | null")]
    pub reported_bytes: Option<u64>,
    pub install_location: Option<String>,
    /// Installed through Windows Installer (MSI).
    pub msi: bool,
    /// Name of the registry key, which identifies the entry.
    pub key: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum StoreKind {
    App,
    /// Shared libraries other apps depend on (VC++ libraries, WinUI, .NET Native...).
    Framework,
    /// Part of Windows itself (installed under the Windows folder).
    System,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct StoreApp {
    /// The app's display name, or its package identity when Windows only gave a
    /// resource reference.
    pub name: String,
    /// Package identity name, e.g. `Microsoft.WindowsCalculator`.
    pub identity: String,
    pub version: String,
    pub architecture: String,
    pub kind: StoreKind,
    pub root_folder: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SoftwareInventory {
    /// Alphabetical by name.
    pub software: Vec<InstalledSoftware>,
    /// Store packages registered for this user: apps, then frameworks, then system
    /// packages, each alphabetical.
    pub store_apps: Vec<StoreApp>,
    /// Package entries left out: language and scale resource packs, older versions of a
    /// package, and keys that are not package names.
    #[ts(type = "number")]
    pub store_skipped: u64,
    /// Entries left out as updates, system components or nameless.
    #[ts(type = "number")]
    pub skipped: u64,
    /// Sum of reported sizes; entries without one count as zero.
    #[ts(type = "number")]
    pub reported_bytes: u64,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Text(String),
    Number(u32),
}

/// One uninstall key as read from the registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawEntry {
    pub key: String,
    pub scope: Scope,
    pub values: BTreeMap<String, Value>,
}

impl RawEntry {
    fn text(&self, name: &str) -> Option<String> {
        match self.values.get(name)? {
            Value::Text(s) => {
                let s = s.trim();
                (!s.is_empty() && s.len() <= 512).then(|| s.to_owned())
            }
            Value::Number(_) => None,
        }
    }
    fn number(&self, name: &str) -> Option<u32> {
        match self.values.get(name)? {
            Value::Number(n) => Some(*n),
            Value::Text(s) => s.trim().parse().ok(),
        }
    }
}

/// `20250131` -> `2025-01-31`; anything else is dropped rather than guessed.
fn install_date(s: &str) -> Option<String> {
    let b = s.as_bytes();
    if b.len() != 8 || !b.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let (y, m, d) = (&s[..4], &s[4..6], &s[6..]);
    let (mi, di): (u32, u32) = (m.parse().ok()?, d.parse().ok()?);
    ((1..=12).contains(&mi) && (1..=31).contains(&di) && y >= "1990")
        .then(|| format!("{y}-{m}-{d}"))
}

fn interpret_one(e: &RawEntry) -> Option<InstalledSoftware> {
    let name = e.text("DisplayName")?;
    if e.number("SystemComponent") == Some(1)
        || e.text("ParentKeyName").is_some()
        || e.text("ParentDisplayName").is_some()
        || e.text("ReleaseType").is_some_and(|r| {
            matches!(
                r.to_lowercase().as_str(),
                "update" | "security update" | "hotfix" | "update rollup" | "service pack"
            )
        })
    {
        return None;
    }
    Some(InstalledSoftware {
        name,
        version: e.text("DisplayVersion"),
        publisher: e.text("Publisher"),
        scope: e.scope,
        install_date: e.text("InstallDate").and_then(|d| install_date(&d)),
        // EstimatedSize is in KiB.
        reported_bytes: e
            .number("EstimatedSize")
            .filter(|&k| k > 0)
            .map(|k| u64::from(k) * 1024),
        install_location: e
            .text("InstallLocation")
            .map(|p| p.trim_matches('"').to_owned())
            .filter(|p| !p.is_empty()),
        msi: e.number("WindowsInstaller") == Some(1),
        key: e.key.clone(),
    })
}

/// Turn raw registry entries into the inventory. Pure; this is where the rules live.
///
/// The same program is often registered in more than one place (for example both
/// registry views); entries with the same name, version and publisher are merged,
/// keeping the machine-wide 64-bit one.
pub fn interpret(entries: &[RawEntry], warnings: Vec<String>) -> SoftwareInventory {
    let mut skipped = 0u64;
    let mut found: Vec<InstalledSoftware> = Vec::new();
    for e in entries {
        match interpret_one(e) {
            Some(s) => found.push(s),
            None => skipped += 1,
        }
    }
    found.sort_by(|a, b| (a.scope, &a.key).cmp(&(b.scope, &b.key)));
    let mut seen = BTreeSet::new();
    let mut software: Vec<InstalledSoftware> = found
        .into_iter()
        .filter(|s| {
            seen.insert((
                s.name.to_lowercase(),
                s.version.clone(),
                s.publisher.as_ref().map(|p| p.to_lowercase()),
            ))
        })
        .collect();
    software.sort_by_cached_key(|s| (s.name.to_lowercase(), s.key.clone()));
    let reported_bytes = software.iter().filter_map(|s| s.reported_bytes).sum();
    SoftwareInventory {
        software,
        store_apps: Vec::new(),
        store_skipped: 0,
        skipped,
        reported_bytes,
        warnings,
    }
}

/// A Store package key: its full name and the values read from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawPackage {
    pub full_name: String,
    pub display_name: Option<String>,
    pub root_folder: Option<String>,
}

const FRAMEWORK_MARKERS: &[&str] = &[
    "vclibs",
    "net.native",
    "ui.xaml",
    "windowsappruntime",
    "winappruntime",
    "directxruntime",
    "services.store.engagement",
    "microsoft.net.",
    "windows.sdk",
];

fn version_key(v: &str) -> Vec<u64> {
    v.split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

/// `Name_1.2.3.4_x64__8wekyb3d8bbwe` -> (name, version, architecture, resource id).
fn split_full_name(full: &str) -> Option<(&str, &str, &str, &str)> {
    let p: Vec<&str> = full.split('_').collect();
    (p.len() == 5 && !p[0].is_empty() && !p[1].is_empty() && p[4].len() >= 8)
        .then(|| (p[0], p[1], p[2], p[3]))
}

/// Turn raw Store package keys into apps. Pure.
pub fn interpret_store(packages: &[RawPackage]) -> (Vec<StoreApp>, u64) {
    let mut skipped = 0u64;
    let mut newest: BTreeMap<(String, String), StoreApp> = BTreeMap::new();
    for p in packages {
        let Some((identity, version, arch, resource)) = split_full_name(&p.full_name) else {
            skipped += 1;
            continue;
        };
        // Language and scale packs ("split.scale-150", "language-fr") are parts of an app.
        if !resource.is_empty() && resource != "neutral" {
            skipped += 1;
            continue;
        }
        let root = p
            .root_folder
            .as_ref()
            .map(|r| r.trim().to_owned())
            .filter(|r| !r.is_empty());
        let lower_root = root.as_deref().unwrap_or("").to_lowercase();
        let lower_id = identity.to_lowercase();
        let kind = if lower_root.contains(r":\windows\") {
            StoreKind::System
        } else if FRAMEWORK_MARKERS.iter().any(|m| lower_id.contains(m)) {
            StoreKind::Framework
        } else {
            StoreKind::App
        };
        let name = p
            .display_name
            .as_deref()
            .map(str::trim)
            .filter(|n| {
                !n.is_empty()
                    && n.len() <= 200
                    && !n.starts_with("@{")
                    && !n.starts_with("ms-resource:")
            })
            .unwrap_or(identity)
            .to_owned();
        let app = StoreApp {
            name,
            identity: identity.to_owned(),
            version: version.to_owned(),
            architecture: arch.to_owned(),
            kind,
            root_folder: root,
        };
        let key = (lower_id, arch.to_lowercase());
        match newest.get(&key) {
            Some(old) if version_key(&old.version) >= version_key(&app.version) => skipped += 1,
            Some(_) => {
                skipped += 1;
                newest.insert(key, app);
            }
            None => {
                newest.insert(key, app);
            }
        }
    }
    let mut apps: Vec<StoreApp> = newest.into_values().collect();
    apps.sort_by_cached_key(|a| (a.kind, a.name.to_lowercase(), a.identity.clone()));
    (apps, skipped)
}

#[cfg(windows)]
mod registry {
    use std::collections::BTreeMap;

    use winreg::RegKey;
    use winreg::enums::{
        HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY,
    };

    use super::{RawEntry, RawPackage, Scope, Value};

    const UNINSTALL: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";
    /// Values worth reading; nothing else (in particular no uninstall command lines).
    const WANTED: &[&str] = &[
        "DisplayName",
        "DisplayVersion",
        "Publisher",
        "InstallDate",
        "InstallLocation",
        "EstimatedSize",
        "WindowsInstaller",
        "SystemComponent",
        "ParentKeyName",
        "ParentDisplayName",
        "ReleaseType",
    ];
    /// Keys examined per location, a guard against a pathological registry.
    const MAX_KEYS: usize = 20_000;

    fn read_value(key: &RegKey, name: &str) -> Option<Value> {
        key.get_value::<u32, _>(name)
            .map(Value::Number)
            .or_else(|_| key.get_value::<String, _>(name).map(Value::Text))
            .ok()
    }

    const PACKAGES: &str = r"Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\Repository\Packages";

    pub(super) fn read_packages(warnings: &mut Vec<String>) -> Vec<RawPackage> {
        let root =
            match RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(PACKAGES, KEY_READ) {
                Ok(k) => k,
                Err(e) => {
                    warnings.push(format!("The Store package list could not be opened: {e}"));
                    return Vec::new();
                }
            };
        let mut out = Vec::new();
        for (i, name) in root.enum_keys().flatten().enumerate() {
            if i >= MAX_KEYS {
                warnings.push(format!(
                    "The Store package list was cut off at {MAX_KEYS} entries."
                ));
                break;
            }
            let Ok(sub) = root.open_subkey_with_flags(&name, KEY_READ) else {
                continue;
            };
            out.push(RawPackage {
                full_name: name,
                display_name: sub.get_value::<String, _>("DisplayName").ok(),
                root_folder: sub.get_value::<String, _>("PackageRootFolder").ok(),
            });
        }
        out
    }

    pub(super) fn read_all(warnings: &mut Vec<String>) -> Vec<RawEntry> {
        let mut out = Vec::new();
        for (hive, flags, scope, label) in [
            (
                HKEY_LOCAL_MACHINE,
                KEY_WOW64_64KEY,
                Scope::Machine,
                "machine",
            ),
            (
                HKEY_LOCAL_MACHINE,
                KEY_WOW64_32KEY,
                Scope::Machine32,
                "machine (32-bit)",
            ),
            (
                HKEY_CURRENT_USER,
                KEY_WOW64_64KEY,
                Scope::User,
                "current user",
            ),
        ] {
            let root =
                match RegKey::predef(hive).open_subkey_with_flags(UNINSTALL, KEY_READ | flags) {
                    Ok(k) => k,
                    Err(e) => {
                        warnings.push(format!(
                            "The {label} uninstall list could not be opened: {e}"
                        ));
                        continue;
                    }
                };
            for (i, name) in root.enum_keys().flatten().enumerate() {
                if i >= MAX_KEYS {
                    warnings.push(format!(
                        "The {label} uninstall list was cut off at {MAX_KEYS} entries."
                    ));
                    break;
                }
                let Ok(sub) = root.open_subkey_with_flags(&name, KEY_READ | flags) else {
                    continue;
                };
                let values: BTreeMap<String, Value> = WANTED
                    .iter()
                    .filter_map(|w| read_value(&sub, w).map(|v| ((*w).to_owned(), v)))
                    .collect();
                out.push(RawEntry {
                    key: name,
                    scope,
                    values,
                });
            }
        }
        out
    }
}

/// List installed software from the uninstall registry. Read-only.
#[cfg(windows)]
pub fn inventory_software() -> SoftwareInventory {
    let mut warnings = Vec::new();
    let entries = registry::read_all(&mut warnings);
    let mut inv = interpret(&entries, warnings);
    let packages = registry::read_packages(&mut inv.warnings);
    (inv.store_apps, inv.store_skipped) = interpret_store(&packages);
    tracing::info!(
        store = inv.store_apps.len(),
        software = inv.software.len(),
        skipped = inv.skipped,
        "software inventory finished"
    );
    inv
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn raw(key: &str, scope: Scope, vals: &[(&str, Value)]) -> RawEntry {
        RawEntry {
            key: key.to_owned(),
            scope,
            values: vals
                .iter()
                .map(|(k, v)| ((*k).to_owned(), v.clone()))
                .collect(),
        }
    }
    fn t(s: &str) -> Value {
        Value::Text(s.to_owned())
    }

    #[test]
    fn reads_display_metadata() {
        let e = raw(
            "{ABC}",
            Scope::Machine,
            &[
                ("DisplayName", t("  Example App ")),
                ("DisplayVersion", t("2.1.0")),
                ("Publisher", t("Example Inc")),
                ("InstallDate", t("20250131")),
                ("EstimatedSize", Value::Number(2048)),
                ("InstallLocation", t("\"C:\\Program Files\\Example\"")),
                ("WindowsInstaller", Value::Number(1)),
            ],
        );
        let inv = interpret(&[e], vec![]);
        let s = &inv.software[0];
        assert_eq!(s.name, "Example App");
        assert_eq!(s.version.as_deref(), Some("2.1.0"));
        assert_eq!(s.install_date.as_deref(), Some("2025-01-31"));
        assert_eq!(s.reported_bytes, Some(2 * 1024 * 1024));
        assert_eq!(
            s.install_location.as_deref(),
            Some(r"C:\Program Files\Example")
        );
        assert!(s.msi);
        assert_eq!(inv.reported_bytes, 2 * 1024 * 1024);
    }

    #[test]
    fn skips_updates_components_and_nameless_entries() {
        let entries = [
            raw(
                "a",
                Scope::Machine,
                &[
                    ("SystemComponent", Value::Number(1)),
                    ("DisplayName", t("Hidden")),
                ],
            ),
            raw(
                "b",
                Scope::Machine,
                &[
                    ("DisplayName", t("KB5001")),
                    ("ParentKeyName", t("OperatingSystem")),
                ],
            ),
            raw(
                "c",
                Scope::Machine,
                &[
                    ("DisplayName", t("Patch")),
                    ("ReleaseType", t("Security Update")),
                ],
            ),
            raw("d", Scope::Machine, &[("Publisher", t("No name"))]),
            raw("e", Scope::Machine, &[("DisplayName", t("   "))]),
            raw("f", Scope::Machine, &[("DisplayName", t("Real"))]),
        ];
        let inv = interpret(&entries, vec![]);
        assert_eq!(inv.software.len(), 1);
        assert_eq!(inv.software[0].name, "Real");
        assert_eq!(inv.skipped, 5);
    }

    #[test]
    fn merges_the_same_program_from_both_views_keeping_the_64_bit_one() {
        let vals = [
            ("DisplayName", t("Tool")),
            ("DisplayVersion", t("1.0")),
            ("Publisher", t("Acme")),
        ];
        let entries = [
            raw("tool32", Scope::Machine32, &vals),
            raw("tool64", Scope::Machine, &vals),
            raw(
                "tool-user",
                Scope::User,
                &[
                    ("DisplayName", t("Tool")),
                    ("DisplayVersion", t("2.0")),
                    ("Publisher", t("Acme")),
                ],
            ),
        ];
        let inv = interpret(&entries, vec![]);
        assert_eq!(
            inv.software.len(),
            2,
            "different version is a different entry"
        );
        let keys: Vec<&str> = inv.software.iter().map(|s| s.key.as_str()).collect();
        assert!(keys.contains(&"tool64") && !keys.contains(&"tool32"));
        assert!(keys.contains(&"tool-user"));
    }

    #[test]
    fn bad_values_are_dropped_not_guessed() {
        assert_eq!(install_date("20251340"), None);
        assert_eq!(install_date("2025-01-31"), None);
        assert_eq!(install_date("19000101"), None);
        let e = raw(
            "x",
            Scope::User,
            &[("DisplayName", t("X")), ("EstimatedSize", Value::Number(0))],
        );
        assert_eq!(interpret(&[e], vec![]).software[0].reported_bytes, None);
    }

    #[test]
    fn sorted_by_name_ignoring_case() {
        let entries = [
            raw("1", Scope::Machine, &[("DisplayName", t("zeta"))]),
            raw("2", Scope::Machine, &[("DisplayName", t("Alpha"))]),
        ];
        let names: Vec<_> = interpret(&entries, vec![])
            .software
            .into_iter()
            .map(|s| s.name)
            .collect();
        assert_eq!(names, ["Alpha", "zeta"]);
    }

    fn pkg(full: &str, display: Option<&str>, root: &str) -> RawPackage {
        RawPackage {
            full_name: full.to_owned(),
            display_name: display.map(str::to_owned),
            root_folder: Some(root.to_owned()),
        }
    }
    const WA: &str = r"C:\Program Files\WindowsApps\x";

    #[test]
    fn store_names_kinds_and_resource_references() {
        let (apps, skipped) = interpret_store(&[
            pkg(
                "5319275A.WhatsAppDesktop_2.2638.102.0_x64__cv1g1gvanyjgm",
                Some("WhatsApp"),
                WA,
            ),
            pkg(
                "B9ECED6F.ArmouryCrate_6.5.14.0_x64__qmba6cd70vzyy",
                Some("@{B9ECED6F.ArmouryCrate_6.5?ms-resource://x/y}"),
                WA,
            ),
            pkg(
                "Microsoft.VCLibs.140.00_14.0.33519.0_x64__8wekyb3d8bbwe",
                Some("Microsoft Visual C++ 2015 UWP Runtime Package"),
                WA,
            ),
            pkg(
                "Microsoft.AccountsControl_10.0.26100.1_neutral__cw5n1h2txyewy",
                Some("ms-resource:AppName"),
                r"C:\Windows\SystemApps\Microsoft.AccountsControl",
            ),
        ]);
        assert_eq!(skipped, 0);
        let by = |id: &str| apps.iter().find(|a| a.identity == id).unwrap();
        assert_eq!(by("5319275A.WhatsAppDesktop").name, "WhatsApp");
        assert_eq!(by("B9ECED6F.ArmouryCrate").name, "B9ECED6F.ArmouryCrate");
        assert_eq!(by("5319275A.WhatsAppDesktop").kind, StoreKind::App);
        assert_eq!(by("Microsoft.VCLibs.140.00").kind, StoreKind::Framework);
        assert_eq!(by("Microsoft.AccountsControl").kind, StoreKind::System);
        assert_eq!(apps.last().unwrap().kind, StoreKind::System);
    }

    #[test]
    fn store_skips_resource_packs_and_keeps_the_newest_version() {
        let (apps, skipped) = interpret_store(&[
            pkg("Vendor.App_1.0.0.0_x64__abcdefgh1234", Some("App"), WA),
            pkg("Vendor.App_1.10.0.0_x64__abcdefgh1234", Some("App"), WA),
            pkg("Vendor.App_1.9.0.0_x64__abcdefgh1234", Some("App"), WA),
            pkg(
                "Vendor.App_1.10.0.0_neutral_split.scale-150_abcdefgh1234",
                Some("App"),
                WA,
            ),
            pkg("Vendor.App_1.0.0.0_x86__abcdefgh1234", Some("App"), WA),
            pkg("not-a-package-name", None, WA),
        ]);
        assert_eq!(apps.len(), 2, "x64 and x86 are separate");
        let x64 = apps.iter().find(|a| a.architecture == "x64").unwrap();
        assert_eq!(x64.version, "1.10.0.0", "1.10 is newer than 1.9");
        assert_eq!(skipped, 4);
    }
}
