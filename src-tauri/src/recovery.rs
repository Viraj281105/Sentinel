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
use sentinel_quarantine::Quarantine;
use sentinel_safety::Policy;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::commands::error::{CommandError, ErrorKind};
use crate::commands::quarantine::{CleanupRunResponse, MAX_ITEMS, MAX_PROJECTS, run_with};
use crate::db::{Db, now_ms};

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
        // The planner takes the plain name; where a plugin wrote the cleanup type, the plan
        // says so on every item.
        let label = crate::plugins::display_name(&info);
        let found: Vec<Candidate> = candidates_from_preview(&pv)
            .into_iter()
            .map(|mut c| {
                c.source_name.clone_from(&label);
                c
            })
            .collect();
        considered.push(Considered {
            source: info.id.to_owned(),
            name: crate::plugins::display_name(&info),
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
    if !project_paths.is_empty() {
        let provider = ProjectArtifacts::for_projects(project_paths, now_ms);
        let boxed: Vec<Box<dyn CleanupProvider>> = vec![Box::new(provider)];
        let (c, mut cons) = gather(policy, &boxed, now_ms);
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
    })
}

/// One approved item of a plan: where it came from and which item it is.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApprovedItem {
    pub source: String,
    pub path: String,
}

/// The outcome for one source's items. Each source is its own quarantine operation with its
/// own audit records; one failing does not stop the others.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RunPart {
    pub source: String,
    pub source_name: String,
    pub run: Option<CleanupRunResponse>,
    /// Why nothing was moved for this source, if so.
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PlanRunResponse {
    pub parts: Vec<RunPart>,
}

const PROJECTS_SOURCE: &str = "project-artifacts";

/// Move the approved items of a plan into quarantine, one operation per source, through the
/// same executor the Cleanup page uses. The executor re-assesses every item immediately
/// before moving it, so a stale plan, or an item that changed, is left in place.
pub(crate) fn execute(
    q: &Quarantine,
    policy: &Policy,
    db: &Db,
    user: &str,
    providers: &[Box<dyn CleanupProvider>],
    project_paths: &[PathBuf],
    approved: &[ApprovedItem],
) -> Result<PlanRunResponse, CommandError> {
    if approved.is_empty() || approved.len() > MAX_ITEMS {
        return Err(CommandError::new(
            ErrorKind::InvalidInput,
            "Choose between 1 and 10,000 items to move.",
        ));
    }
    // Group by source, keeping the order items were approved in.
    let mut order: Vec<&str> = Vec::new();
    for a in approved {
        if !order.contains(&a.source.as_str()) {
            order.push(&a.source);
        }
    }
    let project_provider = (order.contains(&PROJECTS_SOURCE) && !project_paths.is_empty())
        .then(|| ProjectArtifacts::for_projects(project_paths, now_ms()));
    let mut parts = Vec::new();
    for source in order {
        let paths: Vec<String> = approved
            .iter()
            .filter(|a| a.source == source)
            .map(|a| a.path.clone())
            .collect();
        let provider: Option<&dyn CleanupProvider> = if source == PROJECTS_SOURCE {
            project_provider.as_ref().map(|p| p as &dyn CleanupProvider)
        } else {
            providers
                .iter()
                .find(|p| p.info().id == source)
                .map(|p| p.as_ref())
        };
        let refuse = |why: &str| RunPart {
            source: source.to_owned(),
            source_name: source.to_owned(),
            run: None,
            error: Some(why.to_owned()),
        };
        let Some(provider) = provider else {
            parts.push(refuse(if source == PROJECTS_SOURCE {
                "The projects this plan used were not provided, so nothing was moved."
            } else {
                "Sentinel has no cleanup type with this name, so nothing was moved."
            }));
            continue;
        };
        let info = provider.info();
        if !info.can_clean || info.risk > Risk::MediumRisk {
            // Plans never contain these; refuse rather than trust the caller.
            parts.push(refuse("This kind of item is never moved by a plan."));
            continue;
        }
        let mut part = RunPart {
            source: source.to_owned(),
            source_name: crate::plugins::display_name(&info),
            run: None,
            error: None,
        };
        match run_with(q, policy, db, user, provider, &paths) {
            Ok(r) => part.run = Some(r),
            Err(e) => part.error = Some(e.message),
        }
        parts.push(part);
    }
    Ok(PlanRunResponse { parts })
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
        let now = crate::db::now_ms();
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

    fn run_plan(
        dir: &tempfile::TempDir,
        q: &Quarantine,
        db: &Db,
        now: i64,
        approved: &[ApprovedItem],
    ) -> Result<PlanRunResponse, CommandError> {
        let policy = Policy::new(ProtectedSet::new());
        let _ = now;
        execute(q, &policy, db, "tester", &providers(dir), &[], approved)
    }

    fn item(source: &str, path: &std::path::Path) -> ApprovedItem {
        ApprovedItem {
            source: source.into(),
            path: path.display().to_string(),
        }
    }

    #[test]
    fn running_a_plan_moves_only_the_approved_eligible_items_into_quarantine_and_audits_it() {
        let (dir, now) = temp_with(&[("old.tmp", 200_000, 30), ("recent.tmp", 50_000, 0)]);
        let qdir = tempfile::tempdir().unwrap();
        let q = Quarantine::at(qdir.path().join("q"));
        let db = Db::in_memory();
        let policy = Policy::new(ProtectedSet::new());
        let r = build(&policy, &providers(&dir), &[], now, 100_000, Risk::Safe).unwrap();
        let approved: Vec<ApprovedItem> = r
            .plan
            .entries
            .iter()
            .map(|e| ApprovedItem {
                source: e.source.clone(),
                path: e.path.clone(),
            })
            .collect();
        assert_eq!(approved.len(), 1);
        let out = run_plan(&dir, &q, &db, now, &approved).unwrap();
        assert_eq!(out.parts.len(), 1);
        let run = out.parts[0].run.as_ref().unwrap();
        assert!(out.parts[0].error.is_none());
        assert_eq!(
            run.manifest
                .entries
                .iter()
                .filter(|e| matches!(e.status, sentinel_quarantine::EntryStatus::Quarantined))
                .count(),
            1
        );
        assert!(!dir.path().join("old.tmp").exists(), "moved out of TEMP");
        assert!(
            dir.path().join("recent.tmp").exists(),
            "not in the plan, untouched"
        );
        // Moved, not deleted: the item is in quarantine and the operation is audited.
        assert!(qdir.path().join("q").exists());
        assert!(!db.lock().audit_records(10, None).unwrap().is_empty());
    }

    #[test]
    fn an_item_that_became_ineligible_since_planning_is_left_in_place() {
        // Both were old when planned; one was touched before the plan was run.
        let (dir, now) = temp_with(&[("touched.tmp", 200_000, 30), ("untouched.tmp", 200_000, 30)]);
        let qdir = tempfile::tempdir().unwrap();
        let q = Quarantine::at(qdir.path().join("q"));
        let db = Db::in_memory();
        // Approved paths come from a preview, which spells them canonically; so do these (a
        // runner's temp folder can be spelled with short names).
        let base = sentinel_safety::CanonicalPath::resolve(dir.path())
            .unwrap()
            .as_path()
            .to_path_buf();
        let touched = base.join("touched.tmp");
        let untouched = base.join("untouched.tmp");
        std::fs::File::options()
            .write(true)
            .open(&touched)
            .unwrap()
            .set_modified(std::time::SystemTime::now())
            .unwrap();
        let out = run_plan(
            &dir,
            &q,
            &db,
            now,
            &[item("user-temp", &touched), item("user-temp", &untouched)],
        )
        .unwrap();
        assert!(out.parts[0].run.is_some());
        assert!(touched.exists(), "the changed item stays where it is");
        assert!(!untouched.exists(), "the control item does move");
    }

    #[test]
    fn unknown_sources_projects_not_provided_and_uncleanable_kinds_move_nothing() {
        let (dir, now) = temp_with(&[("old.tmp", 1_000, 30)]);
        let qdir = tempfile::tempdir().unwrap();
        let q = Quarantine::at(qdir.path().join("q"));
        let db = Db::in_memory();
        let target = dir.path().join("old.tmp");
        let out = run_plan(
            &dir,
            &q,
            &db,
            now,
            &[
                item("made-up", &target),
                item("project-artifacts", &target),
                item("pnpm-store", &target),
            ],
        )
        .unwrap();
        assert_eq!(out.parts.len(), 3);
        assert!(
            out.parts
                .iter()
                .all(|p| p.run.is_none() && p.error.is_some())
        );
        assert!(target.exists());
        assert!(db.lock().audit_records(10, None).unwrap().is_empty());
    }

    #[test]
    fn empty_and_oversized_approvals_are_refused() {
        let (dir, now) = temp_with(&[]);
        let qdir = tempfile::tempdir().unwrap();
        let q = Quarantine::at(qdir.path().join("q"));
        let db = Db::in_memory();
        assert_eq!(
            run_plan(&dir, &q, &db, now, &[]).unwrap_err().kind,
            ErrorKind::InvalidInput
        );
        let many: Vec<ApprovedItem> = (0..=MAX_ITEMS)
            .map(|i| ApprovedItem {
                source: "user-temp".into(),
                path: format!("x{i}"),
            })
            .collect();
        assert_eq!(
            run_plan(&dir, &q, &db, now, &many).unwrap_err().kind,
            ErrorKind::InvalidInput
        );
        assert!(!qdir.path().join("q").exists());
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

    /// Manual, read-only diagnostic: the plan this machine would get right now. Nothing is
    /// moved, no database is opened, nothing is confirmed.
    ///
    ///     cargo test -p sentinel-app real_plan -- --ignored --nocapture
    #[test]
    #[ignore = "reads this machine's real caches and projects; run by hand"]
    fn real_plan_for_this_machine_read_only() {
        use std::sync::atomic::AtomicBool;

        let policy = Policy::for_system();
        let providers = sentinel_cleanup::providers::builtin();
        let root =
            std::env::var("SENTINEL_PROJECTS_ROOT").unwrap_or_else(|_| r"D:\Projects".into());
        let detection = sentinel_devenv::detect(
            std::path::Path::new(&root),
            sentinel_devenv::DetectOptions::default(),
            &AtomicBool::new(false),
        )
        .unwrap();
        let projects = project_paths(&[detection]);
        println!(
            "projects with rebuildable folders under {root}: {}",
            projects.len()
        );

        for (label, target_mb, risk) in [
            ("SAFE ceiling, 1 GB", 1024u64, Risk::Safe),
            ("MEDIUM ceiling, 5 GB", 5 * 1024, Risk::MediumRisk),
        ] {
            let r = build(
                &policy,
                &providers,
                &projects,
                crate::db::now_ms(),
                target_mb * 1024 * 1024,
                risk,
            )
            .unwrap();
            let p = &r.plan;
            println!("\n=== {label} ===");
            println!(
                "target {} MB; planned {} MB in {} items; reaches target: {}; available at/below ceiling {} MB; above ceiling {} MB in {} items",
                target_mb,
                p.planned_bytes / 1_048_576,
                p.entries.len(),
                p.reaches_target,
                p.available_bytes / 1_048_576,
                p.above_ceiling_bytes / 1_048_576,
                p.above_ceiling_items
            );
            for c in &r.considered {
                println!(
                    "  considered: {} [{:?}] {} eligible items, {} MB {}",
                    c.name,
                    c.risk,
                    c.eligible_items,
                    c.eligible_bytes / 1_048_576,
                    c.note.as_deref().unwrap_or("")
                );
            }
            for (i, e) in p.entries.iter().take(12).enumerate() {
                println!(
                    "  {:>2}. {:>8} MB  {:?}  {}\n      {}",
                    i + 1,
                    e.bytes / 1_048_576,
                    e.risk,
                    e.path,
                    e.why
                );
            }
            if p.entries.len() > 12 {
                println!("  ... {} more", p.entries.len() - 12);
            }
            for n in &p.notes {
                println!("  note: {n}");
            }
            // Whatever the machine holds, the guarantees must hold.
            assert!(
                p.entries.iter().all(|e| e.risk <= risk),
                "nothing above the ceiling"
            );
            assert!(
                p.entries.iter().all(|e| policy
                    .check_protected(std::path::Path::new(&e.path))
                    .is_ok()),
                "nothing protected"
            );
            let mut paths: Vec<_> = p.entries.iter().map(|e| e.path.to_lowercase()).collect();
            paths.sort();
            paths.dedup();
            assert_eq!(paths.len(), p.entries.len(), "no item twice");
        }
    }
}
