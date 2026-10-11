//! Building a recovery plan from what the cleanup providers would offer right now.
//!
//! Every candidate comes from a provider's own dry-run preview, so it has already passed the
//! safety policy; the planner only chooses among them (docs/SAFETY_MODEL.md, "A planner given
//! a risk ceiling never includes candidates above it"). Nothing here changes any file, and a
//! plan is only a proposal: items are validated again when someone approves and runs them.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use sentinel_cleanup::providers::ProjectArtifacts;
use sentinel_cleanup::{
    Candidate, CleanupProvider, Plan, PreviewLimits, Risk, candidates_from_preview, plan, preview,
};
use sentinel_safety::Policy;
use serde::Serialize;
use ts_rs::TS;

use crate::commands::error::{CommandError, ErrorKind};
use crate::commands::quarantine::MAX_PROJECTS;

/// What one source offered, so a person can see what the plan was drawn from.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Considered {
    pub source: String,
    pub name: String,
    pub risk: Risk,
    #[ts(type = "number")]
    pub eligible_items: u64,
    #[ts(type = "number")]
    pub eligible_bytes: u64,
    /// Why this source may be incomplete or offers nothing.
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RecoveryPlan {
    pub plan: Plan,
    pub considered: Vec<Considered>,
    #[ts(type = "number")]
    pub generated_at_ms: i64,
    /// Project folders the project-artifact entries came from (for running those entries).
    pub projects: Vec<String>,
}

/// Preview every provider and collect its eligible items.
pub(crate) fn gather(
    policy: &Policy,
    providers: &[Box<dyn CleanupProvider>],
    now_ms: i64,
) -> (Vec<Candidate>, Vec<Considered>) {
    let mut candidates = Vec::new();
    let mut considered = Vec::new();
    for p in providers {
        let info = p.info();
        if !info.can_clean {
            continue; // analysis-only: nothing is ever eligible
        }
        let pv = preview(
            p.as_ref(),
            policy,
            now_ms,
            PreviewLimits::default(),
            &AtomicBool::new(false),
        );
        let found = candidates_from_preview(&pv);
        considered.push(Considered {
            source: info.id.to_owned(),
            name: info.name.to_owned(),
            risk: info.risk,
            eligible_items: found.len() as u64,
            eligible_bytes: found.iter().map(|c| c.bytes).sum(),
            note: pv
                .incomplete
                .then(|| "The preview stopped early, so there may be more.".to_owned()),
        });
        candidates.extend(found);
    }
    (candidates, considered)
}

/// Inactive projects' rebuildable folders, from the saved project searches. The provider
/// re-inspects each project and excludes active ones and anything Git tracks.
pub(crate) fn project_paths(searches: &[sentinel_devenv::Detection]) -> Vec<PathBuf> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for d in searches {
        for p in &d.projects {
            if !p.artifacts.is_empty() && seen.insert(p.path.to_lowercase()) {
                out.push(PathBuf::from(&p.path));
            }
        }
    }
    out.truncate(MAX_PROJECTS);
    out
}

pub(crate) fn build(
    policy: &Policy,
    providers: &[Box<dyn CleanupProvider>],
    project_paths: &[PathBuf],
    now_ms: i64,
    target_bytes: u64,
    max_risk: Risk,
) -> Result<RecoveryPlan, CommandError> {
    // Reject a bad request before doing any directory walking.
    plan(&[], target_bytes, max_risk)
        .map_err(|e| CommandError::new(ErrorKind::InvalidInput, format!("{e}.")))?;
    let (mut candidates, mut considered) = gather(policy, providers, now_ms);
    let mut projects = Vec::new();
    if !project_paths.is_empty() {
        let provider = ProjectArtifacts::for_projects(project_paths, now_ms);
        let boxed: Vec<Box<dyn CleanupProvider>> = vec![Box::new(provider)];
        let (c, mut cons) = gather(policy, &boxed, now_ms);
        if c.iter().any(|x| x.bytes > 0) {
            projects = project_paths
                .iter()
                .map(|p| p.display().to_string())
                .collect();
        }
        candidates.extend(c);
        considered.append(&mut cons);
    } else {
        considered.push(Considered {
            source: "project-artifacts".into(),
            name: "Rebuildable folders in inactive projects".into(),
            risk: Risk::MediumRisk,
            eligible_items: 0,
            eligible_bytes: 0,
            note: Some(
                "No projects have been searched yet. Search a folder on the Projects page to include them."
                    .into(),
            ),
        });
    }
    let plan = plan(&candidates, target_bytes, max_risk)
        .map_err(|e| CommandError::new(ErrorKind::InvalidInput, format!("{e}.")))?;
    Ok(RecoveryPlan {
        plan,
        considered,
        generated_at_ms: now_ms,
        projects,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use sentinel_cleanup::providers::UserTemp;
    use sentinel_safety::ProtectedSet;

    use super::*;

    const DAY_MS: i64 = 86_400_000;

    fn temp_with(files: &[(&str, usize, i64)]) -> (tempfile::TempDir, i64) {
        let dir = tempfile::tempdir().unwrap();
        let now = 1_900_000_000_000i64;
        for (name, size, age_days) in files {
            let p = dir.path().join(name);
            std::fs::write(&p, vec![1u8; *size]).unwrap();
            let t = std::time::UNIX_EPOCH
                + std::time::Duration::from_millis((now - age_days * DAY_MS) as u64);
            std::fs::File::options()
                .write(true)
                .open(&p)
                .unwrap()
                .set_modified(t)
                .unwrap();
        }
        (dir, now)
    }

    fn providers(dir: &tempfile::TempDir) -> Vec<Box<dyn CleanupProvider>> {
        vec![Box::new(UserTemp::with_root(dir.path().to_path_buf()))]
    }

    #[test]
    fn plans_from_eligible_items_only_and_explains_each() {
        let (dir, now) = temp_with(&[("old.tmp", 200_000, 30), ("recent.tmp", 900_000, 0)]);
        let policy = Policy::new(ProtectedSet::new());
        let r = build(&policy, &providers(&dir), &[], now, 100_000, Risk::Safe).unwrap();
        assert!(r.plan.reaches_target);
        assert_eq!(r.plan.entries.len(), 1);
        assert!(r.plan.entries[0].path.ends_with("old.tmp"));
        assert!(!r.plan.entries[0].why.is_empty());
        // The too-recent file is not in the plan, nor in what was available.
        assert!(r.plan.available_bytes < 900_000);
        assert_eq!(r.considered[0].eligible_items, 1);
        assert!(
            r.considered
                .iter()
                .any(|c| c.source == "project-artifacts" && c.note.is_some())
        );
    }

    #[test]
    fn a_target_beyond_what_exists_is_a_short_plan_not_an_error() {
        let (dir, now) = temp_with(&[("old.tmp", 10_000, 30)]);
        let policy = Policy::new(ProtectedSet::new());
        let r = build(
            &policy,
            &providers(&dir),
            &[],
            now,
            50_000_000_000,
            Risk::Safe,
        )
        .unwrap();
        assert!(!r.plan.reaches_target);
        assert!(r.plan.shortfall_bytes > 0);
        assert!(!r.plan.notes.is_empty());
    }

    #[test]
    fn bad_requests_are_refused_before_any_walking() {
        let (dir, now) = temp_with(&[]);
        let policy = Policy::new(ProtectedSet::new());
        for (target, risk) in [(0, Risk::Safe), (10, Risk::HighRisk), (10, Risk::Protected)] {
            let e = build(&policy, &providers(&dir), &[], now, target, risk).unwrap_err();
            assert_eq!(e.kind, ErrorKind::InvalidInput);
        }
    }
}
