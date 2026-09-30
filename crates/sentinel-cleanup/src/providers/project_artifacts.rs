//! Rebuildable folders (`node_modules`, virtual environments, build output) of projects
//! that have not changed for [`INACTIVE_DAYS`].
//!
//! The provider is built for an explicit list of project folders and re-inspects each
//! one from disk when built (saved search results may be stale):
//! - a folder that is no longer a project, or a project changed within the inactivity
//!   period, is excluded with the reason;
//! - an artifact folder that Git tracks anything under is excluded as protected;
//! - if Git's index cannot be read reliably, the folder is excluded too.
//!
//! Only the artifact folders the project detector identified are candidates.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use sentinel_classify::Category;
use sentinel_devenv::{inspect, tracks_anything_under};
use sentinel_safety::{CanonicalPath, RiskLevel};

use crate::{CleanupProvider, Exclusion, ProviderInfo};

/// A project counts as inactive when nothing in it changed for this many days.
pub const INACTIVE_DAYS: u32 = 90;
const DAY_MS: i64 = 24 * 60 * 60 * 1000;

pub struct ProjectArtifacts {
    roots: Vec<PathBuf>,
    /// Lower-cased canonical project path -> lower-cased artifact folder names.
    candidates: HashMap<String, HashSet<String>>,
    exclusions: Vec<Exclusion>,
}

fn key(p: &Path) -> String {
    p.display().to_string().to_lowercase()
}

impl ProjectArtifacts {
    /// Re-inspect `projects` now and keep only inactive ones' untracked artifacts.
    pub fn for_projects(projects: &[PathBuf], now_ms: i64) -> Self {
        let mut s = Self {
            roots: Vec::new(),
            candidates: HashMap::new(),
            exclusions: Vec::new(),
        };
        let mut exclude = |path: &Path, reason: String, protected: bool| {
            s.exclusions.push(Exclusion {
                path: path.display().to_string(),
                reason,
                protected,
            });
        };
        let mut kept: Vec<(PathBuf, HashSet<String>)> = Vec::new();
        for requested in projects {
            let Ok(canon) = CanonicalPath::resolve(requested) else {
                exclude(requested, "the folder no longer exists".into(), false);
                continue;
            };
            let dir = canon.as_path().to_path_buf();
            let Some(project) = inspect(&dir) else {
                exclude(&dir, "it is no longer a project".into(), false);
                continue;
            };
            match project.last_activity_ms {
                Some(t) if now_ms - t >= i64::from(INACTIVE_DAYS) * DAY_MS => {}
                Some(t) => {
                    let days = (now_ms - t).max(0) / DAY_MS;
                    exclude(
                        &dir,
                        format!(
                            "the project changed {days} days ago; only projects unchanged for \
                             {INACTIVE_DAYS} days are cleaned"
                        ),
                        false,
                    );
                    continue;
                }
                None => {
                    exclude(
                        &dir,
                        "when the project last changed is unknown".into(),
                        false,
                    );
                    continue;
                }
            }
            let mut names = HashSet::new();
            for a in &project.artifacts {
                let path = PathBuf::from(&a.path);
                match tracks_anything_under(&path) {
                    Ok(false) => {
                        if let Some(n) = path.file_name() {
                            names.insert(n.to_string_lossy().to_lowercase());
                        }
                    }
                    Ok(true) => exclude(&path, "Git tracks files in this folder".into(), true),
                    Err(e) => exclude(
                        &path,
                        format!("could not confirm that Git does not track it: {e}"),
                        false,
                    ),
                }
            }
            if !names.is_empty() {
                kept.push((dir, names));
            }
        }
        for (dir, names) in kept {
            s.candidates.insert(key(&dir), names);
            s.roots.push(dir);
        }
        s
    }
}

impl CleanupProvider for ProjectArtifacts {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: "project-artifacts",
            name: "Rebuildable folders in inactive projects",
            category: Category::DeveloperDependencies,
            risk: RiskLevel::MediumRisk.into(),
            description: "node_modules, Python virtual environments and build output inside \
                          projects nothing has changed in for 90 days.",
            on_removal: "The project needs a reinstall or rebuild before it runs again \
                         (npm install, recreate the virtual environment, cargo build, and so \
                         on). Your source code is never touched.",
            min_age_days: INACTIVE_DAYS,
            can_clean: true,
            note: Some(
                "Folders Git tracks are never touched, and projects changed in the last 90 \
                 days are skipped.",
            ),
        }
    }

    fn roots(&self) -> Vec<PathBuf> {
        self.roots.clone()
    }

    fn is_candidate(&self, root: &Path, name: &str) -> bool {
        self.candidates
            .get(&key(root))
            .is_some_and(|n| n.contains(&name.to_lowercase()))
    }

    fn exclusions(&self) -> Vec<Exclusion> {
        self.exclusions.clone()
    }
}
