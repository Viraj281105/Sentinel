//! Deterministic storage classification.
//!
//! Maps paths to the categories from the product directive using a fixed rule table
//! (see `rules.rs`). No heuristics, no guessing and no AI: a path that no rule matches
//! is `Unknown`, and every classification names the rule and the reason behind it.
//!
//! Matching walks the path from the drive root. At each component a location rule
//! (known folder + relative path) is tried first, then a name rule, then, directly under
//! a drive root, a root rule. The deepest match wins; deeper folders inherit it.

mod rules;

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use sentinel_safety::{Known, known_folder};
use sentinel_scanner::scan::ScanTree;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub use rules::Anchor;
use rules::{LOCATION_RULES, NAME_RULES, ROOT_RULES};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Category {
    Windows,
    Applications,
    DeveloperTools,
    DeveloperDependencies,
    PackageCaches,
    BuildArtifacts,
    Containers,
    Wsl,
    Browsers,
    Games,
    Logs,
    TemporaryFiles,
    UserData,
    Unknown,
}

impl Category {
    pub const ALL: [Category; 14] = [
        Self::Windows,
        Self::Applications,
        Self::DeveloperTools,
        Self::DeveloperDependencies,
        Self::PackageCaches,
        Self::BuildArtifacts,
        Self::Containers,
        Self::Wsl,
        Self::Browsers,
        Self::Games,
        Self::Logs,
        Self::TemporaryFiles,
        Self::UserData,
        Self::Unknown,
    ];

    /// Stable storage key (same as the serialized form).
    pub fn key(self) -> &'static str {
        match self {
            Self::Windows => "windows",
            Self::Applications => "applications",
            Self::DeveloperTools => "developerTools",
            Self::DeveloperDependencies => "developerDependencies",
            Self::PackageCaches => "packageCaches",
            Self::BuildArtifacts => "buildArtifacts",
            Self::Containers => "containers",
            Self::Wsl => "wsl",
            Self::Browsers => "browsers",
            Self::Games => "games",
            Self::Logs => "logs",
            Self::TemporaryFiles => "temporaryFiles",
            Self::UserData => "userData",
            Self::Unknown => "unknown",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.key() == key)
    }
}

/// Why a path was put in a category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Classification {
    pub category: Category,
    pub rule: &'static str,
    pub reason: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CategoryBytes {
    pub category: Category,
    #[ts(type = "number")]
    pub bytes: u64,
}

/// Case-insensitive lookup key for a path: `c:\users\me`. A drive root is `c:`.
fn key_of(path: &Path) -> String {
    let mut key = String::new();
    for c in path.components() {
        match c {
            Component::Prefix(p) => key.push_str(&p.as_os_str().to_string_lossy().to_lowercase()),
            Component::Normal(n) => {
                key.push('\\');
                key.push_str(&n.to_string_lossy().to_lowercase());
            }
            Component::RootDir | Component::CurDir | Component::ParentDir => {}
        }
    }
    key.trim_start_matches(r"\\?\").to_owned()
}

fn is_drive_root_key(key: &str) -> bool {
    key.len() == 2 && key.ends_with(':')
}

pub struct Classifier {
    locations: HashMap<String, Classification>,
    names: HashMap<String, Classification>,
}

impl Classifier {
    /// Build a classifier for the given anchor locations. Rules whose anchor is absent
    /// are skipped.
    pub fn new(anchors: &HashMap<Anchor, PathBuf>) -> Self {
        let mut locations = HashMap::new();
        for r in LOCATION_RULES {
            let Some(base) = anchors.get(&r.anchor) else {
                continue;
            };
            let mut p = base.clone();
            p.extend(r.rel);
            locations.entry(key_of(&p)).or_insert(Classification {
                category: r.category,
                rule: r.id,
                reason: r.reason,
            });
        }
        let names = NAME_RULES
            .iter()
            .map(|r| {
                (
                    r.name.to_lowercase(),
                    Classification {
                        category: r.category,
                        rule: r.id,
                        reason: r.reason,
                    },
                )
            })
            .collect();
        Self { locations, names }
    }

    /// Classifier for this machine's known folders (plus `CARGO_HOME`/`RUSTUP_HOME`).
    pub fn for_system() -> Self {
        Self::new(&system_anchors())
    }

    /// Classify the entry `name` inside the folder whose key is `parent_key`, ignoring
    /// inheritance. Returns the entry's key and its own match, if any.
    fn match_entry(&self, parent_key: &str, name: &str) -> (String, Option<Classification>) {
        let key = format!("{parent_key}\\{}", name.to_lowercase());
        let m = self
            .locations
            .get(&key)
            .or_else(|| self.names.get(&name.to_lowercase()))
            .copied()
            .or_else(|| {
                is_drive_root_key(parent_key)
                    .then(|| ROOT_RULES.iter().find(|r| (r.matches)(name)))
                    .flatten()
                    .map(|r| Classification {
                        category: r.category,
                        rule: r.id,
                        reason: r.reason,
                    })
            });
        (key, m)
    }

    /// Classify a folder or file path. `None` means no rule applies (Unknown).
    pub fn classify(&self, path: &Path) -> Option<Classification> {
        let mut key = String::new();
        let mut current = None;
        for c in path.components() {
            match c {
                Component::Prefix(p) => {
                    key = p.as_os_str().to_string_lossy().to_lowercase();
                    key = key.trim_start_matches(r"\\?\").to_owned();
                }
                Component::Normal(n) => {
                    let (k, m) = self.match_entry(&key, &n.to_string_lossy());
                    key = k;
                    if m.is_some() {
                        current = m;
                    }
                }
                Component::RootDir | Component::CurDir | Component::ParentDir => {}
            }
        }
        current
    }

    /// Bytes per category over a whole scan tree: each folder's own files are counted
    /// in its effective category, except that each of the scan's largest files is moved
    /// to its own category when a rule classifies the file differently from its folder
    /// (e.g. `pagefile.sys` in an unclassified drive root). The result sums to the
    /// tree's total and is sorted largest first; empty categories are omitted.
    pub fn breakdown(&self, tree: &ScanTree) -> Vec<CategoryBytes> {
        let n = tree.nodes.len();
        let mut keys: Vec<String> = Vec::with_capacity(n);
        let mut effective: Vec<Option<Classification>> = Vec::with_capacity(n);
        let mut totals: HashMap<Category, u64> = HashMap::new();
        for (i, node) in tree.nodes.iter().enumerate() {
            let (key, eff) = match node.parent {
                None => (key_of(&tree.root), self.classify(&tree.root)),
                Some(p) => {
                    let p = p as usize;
                    let (k, m) = self.match_entry(&keys[p], &node.name);
                    (k, m.or(effective[p]))
                }
            };
            debug_assert_eq!(keys.len(), i);
            let cat = eff.map_or(Category::Unknown, |c| c.category);
            *totals.entry(cat).or_default() += node.own_bytes;
            keys.push(key);
            effective.push(eff);
        }
        let cat_of = |c: Option<Classification>| c.map_or(Category::Unknown, |c| c.category);
        for f in &tree.largest_files {
            let path = Path::new(&f.path);
            let file_cat = cat_of(self.classify(path));
            let folder_cat = cat_of(path.parent().and_then(|p| self.classify(p)));
            if file_cat != folder_cat {
                let from = totals.entry(folder_cat).or_default();
                let moved = f.bytes.min(*from);
                *from -= moved;
                *totals.entry(file_cat).or_default() += moved;
            }
        }
        let mut out: Vec<_> = totals
            .into_iter()
            .filter(|&(_, b)| b > 0)
            .map(|(category, bytes)| CategoryBytes { category, bytes })
            .collect();
        out.sort_by(|a, b| b.bytes.cmp(&a.bytes).then(a.category.cmp(&b.category)));
        out
    }
}

/// Anchor locations for this machine. Known folders come from the shell API; Cargo and
/// rustup homes honor their environment variables because the tools themselves do.
pub fn system_anchors() -> HashMap<Anchor, PathBuf> {
    let mut a = HashMap::new();
    for (anchor, known) in [
        (Anchor::Windows, Known::Windows),
        (Anchor::ProgramFiles, Known::ProgramFiles),
        (Anchor::ProgramFilesX86, Known::ProgramFilesX86),
        (Anchor::ProgramData, Known::ProgramData),
        (Anchor::Profile, Known::Profile),
        (Anchor::LocalAppData, Known::LocalAppData),
        (Anchor::RoamingAppData, Known::RoamingAppData),
    ] {
        if let Some(p) = known_folder(known) {
            a.insert(anchor, p);
        }
    }
    let env_or = |var: &str, default: &str| {
        std::env::var_os(var)
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| a.get(&Anchor::Profile).map(|p| p.join(default)))
    };
    let cargo = env_or("CARGO_HOME", ".cargo");
    let rustup = env_or("RUSTUP_HOME", ".rustup");
    if let Some(p) = cargo {
        a.insert(Anchor::CargoHome, p);
    }
    if let Some(p) = rustup {
        a.insert(Anchor::RustupHome, p);
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_ids_are_unique_and_keys_roundtrip() {
        let mut ids: Vec<&str> = LOCATION_RULES.iter().map(|r| r.id).collect();
        ids.extend(NAME_RULES.iter().map(|r| r.id));
        ids.extend(ROOT_RULES.iter().map(|r| r.id));
        let n = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), n, "duplicate rule id");
        for c in Category::ALL {
            assert_eq!(Category::from_key(c.key()), Some(c));
        }
    }

    #[test]
    fn location_rule_paths_are_unique_per_anchor() {
        let mut seen = std::collections::HashSet::new();
        for r in LOCATION_RULES {
            let key = format!("{:?}/{}", r.anchor, r.rel.join("/").to_lowercase());
            assert!(seen.insert(key.clone()), "two rules for {key}");
        }
    }

    #[test]
    fn keys_are_case_insensitive_and_verbatim_free() {
        assert_eq!(key_of(Path::new(r"C:\Users\Me")), r"c:\users\me");
        assert_eq!(key_of(Path::new(r"\\?\C:\Users\Me")), r"c:\users\me");
        assert_eq!(key_of(Path::new(r"D:\")), "d:");
        assert!(is_drive_root_key("d:"));
    }
}
