//! Package-manager download caches.
//!
//! Only the default cache locations are used. Overrides in `.npmrc` or environment
//! variables are deliberately ignored: `.npmrc` can hold credentials and is never read,
//! and an override could point a cache provider at an unrelated folder. As a second
//! line of defense, each cache only accepts the child folders it knows by name.

use std::path::{Path, PathBuf};

use sentinel_classify::Category;
use sentinel_safety::{Known, RiskLevel, known_folder};

use crate::{CleanupProvider, ProviderInfo};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheKind {
    Npm,
    Yarn,
    Pip,
    Pnpm,
}

/// `v` followed by digits, e.g. `v6`: Yarn and pnpm version folders.
fn version_folder(name: &str) -> bool {
    name.strip_prefix('v')
        .or_else(|| name.strip_prefix('V'))
        .is_some_and(|d| !d.is_empty() && d.len() <= 4 && d.chars().all(|c| c.is_ascii_digit()))
}

pub struct PackageCache {
    kind: CacheKind,
    root: Option<PathBuf>,
}

impl PackageCache {
    /// The cache of `kind` at its default location for this user.
    pub fn for_system(kind: CacheKind) -> Self {
        let local = known_folder(Known::LocalAppData);
        let root = local.map(|l| match kind {
            CacheKind::Npm => l.join("npm-cache"),
            CacheKind::Yarn => l.join("Yarn").join("Cache"),
            CacheKind::Pip => l.join("pip").join("cache"),
            CacheKind::Pnpm => l.join("pnpm").join("store"),
        });
        Self { kind, root }
    }

    /// A cache rooted somewhere else. For tests and fixtures.
    pub fn with_root(kind: CacheKind, root: PathBuf) -> Self {
        Self {
            kind,
            root: Some(root),
        }
    }
}

impl CleanupProvider for PackageCache {
    fn info(&self) -> ProviderInfo {
        let base = |id, name, description, on_removal| ProviderInfo {
            id,
            name,
            category: Category::PackageCaches,
            risk: RiskLevel::Safe.into(),
            description,
            on_removal,
            min_age_days: 1,
            can_clean: true,
            note: None,
        };
        match self.kind {
            CacheKind::Npm => base(
                "npm-cache",
                "npm download cache",
                "Packages npm has downloaded before (_cacache), packages run with npx, \
                 prebuilt native binaries and npm's own logs.",
                "npm downloads packages again the next time a project needs them, so the \
                 next install may be slower. Installed projects are not affected. Left alone \
                 if npm was used in the last day.",
            ),
            CacheKind::Yarn => base(
                "yarn-cache",
                "Yarn download cache",
                "Packages Yarn 1 has downloaded before.",
                "Yarn downloads packages again when a project needs them. Installed \
                 projects are not affected. Left alone if Yarn was used in the last day.",
            ),
            CacheKind::Pip => base(
                "pip-cache",
                "pip download cache",
                "Packages and wheels pip has downloaded before.",
                "pip downloads packages again when you next install them. Installed \
                 packages and virtual environments are not affected. Left alone if pip was \
                 used in the last day.",
            ),
            CacheKind::Pnpm => ProviderInfo {
                can_clean: false,
                note: Some(
                    "pnpm's store is hard-linked into every project's node_modules, so moving \
                     it would free little space. Run `pnpm store prune` to remove unused \
                     packages safely.",
                ),
                ..base(
                    "pnpm-store",
                    "pnpm package store",
                    "The content-addressable store pnpm links packages from.",
                    "Shown for its size only; Sentinel does not clean it.",
                )
            },
        }
    }

    fn roots(&self) -> Vec<PathBuf> {
        self.root.iter().cloned().collect()
    }

    fn is_candidate(&self, _root: &Path, name: &str) -> bool {
        let n = name.to_ascii_lowercase();
        match self.kind {
            CacheKind::Npm => matches!(n.as_str(), "_cacache" | "_npx" | "_logs" | "_prebuilds"),
            CacheKind::Pip => matches!(n.as_str(), "http" | "http-v2" | "wheels"),
            CacheKind::Yarn | CacheKind::Pnpm => version_folder(&n),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_known_children_are_candidates() {
        let npm = PackageCache::with_root(CacheKind::Npm, PathBuf::new());
        assert!(
            npm.is_candidate(Path::new(""), "_cacache") && npm.is_candidate(Path::new(""), "_NPX")
        );
        assert!(!npm.is_candidate(Path::new(""), "_update-notifier-last-checked"));
        assert!(!npm.is_candidate(Path::new(""), "my-project"));
        let pip = PackageCache::with_root(CacheKind::Pip, PathBuf::new());
        assert!(
            pip.is_candidate(Path::new(""), "http-v2")
                && !pip.is_candidate(Path::new(""), "selfcheck")
        );
        let yarn = PackageCache::with_root(CacheKind::Yarn, PathBuf::new());
        assert!(
            yarn.is_candidate(Path::new(""), "v6")
                && !yarn.is_candidate(Path::new(""), "v")
                && !yarn.is_candidate(Path::new(""), "vendor")
        );
    }

    #[test]
    fn pnpm_is_analysis_only_with_a_reason() {
        let info = PackageCache::with_root(CacheKind::Pnpm, PathBuf::new()).info();
        assert!(!info.can_clean);
        assert!(info.note.unwrap_or_default().contains("pnpm store prune"));
        assert!(
            PackageCache::with_root(CacheKind::Npm, PathBuf::new())
                .info()
                .can_clean
        );
    }

    #[test]
    fn default_locations_are_under_local_app_data() {
        for kind in [
            CacheKind::Npm,
            CacheKind::Yarn,
            CacheKind::Pip,
            CacheKind::Pnpm,
        ] {
            let roots = PackageCache::for_system(kind).roots();
            assert_eq!(roots.len(), 1);
            assert!(
                roots[0].to_string_lossy().contains(r"AppData\Local"),
                "{roots:?}"
            );
        }
    }
}
