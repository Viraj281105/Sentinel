//! Read-only inventory of installed desktop software.
//!
//! Source: the three Windows "Uninstall" registry locations (machine 64-bit, machine
//! 32-bit view, current user). Only display metadata is read. Uninstall and modify command
//! lines are never read or returned, and nothing is written or run.
//!
//! Not covered yet: winget and Microsoft Store (packaged) apps, which need a package
//! manager to be asked and are a separate milestone. Portable software that registers
//! nothing is invisible to this source.
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SoftwareInventory {
    /// Alphabetical by name.
    pub software: Vec<InstalledSoftware>,
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
        skipped,
        reported_bytes,
        warnings,
    }
}

#[cfg(windows)]
mod registry {
    use std::collections::BTreeMap;

    use winreg::RegKey;
    use winreg::enums::{
        HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY,
    };

    use super::{RawEntry, Scope, Value};

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
    let inv = interpret(&entries, warnings);
    tracing::info!(
        software = inv.software.len(),
        skipped = inv.skipped,
        "software inventory finished"
    );
    inv
}

#[cfg(test)]
mod tests {
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
}
