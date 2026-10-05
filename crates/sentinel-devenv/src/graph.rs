//! Dependency graph: which searched projects need which installed runtimes.
//!
//! Built only from what is already known (saved project searches and the runtime
//! inventory), so it reads nothing new from disk.
//!
//! What an edge means, stated plainly:
//! - `Requires`: the project's own files state a version and this install satisfies it.
//!   A project can be satisfied by several installs of one runtime; each gets an edge.
//! - `Unmet`: the project states a version that no installed version satisfies. It points
//!   at a placeholder node for the missing version.
//! - `Uses`: the project has files of that package manager (a lockfile or manifest).
//!
//! A project that states no runtime version is *not* linked to any install: Sentinel does
//! not guess which one it runs on. Such projects are listed separately.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::inventory::RuntimeInventory;
use crate::model::{PackageManager, Project, Runtime};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum NodeKind {
    Project,
    /// An installed runtime or SDK component.
    Runtime,
    /// A version a project asks for that is not installed.
    MissingRuntime,
    PackageManager,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum EdgeKind {
    Requires,
    Unmet,
    Uses,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GraphNode {
    pub id: String,
    pub kind: NodeKind,
    pub label: String,
    /// Path for projects and runtimes; the requirement for a missing runtime.
    pub detail: Option<String>,
    /// Install size for a runtime, whole-folder size for a project.
    #[ts(type = "number | null")]
    pub bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GraphEdge {
    pub from: String,
    pub to: String,
    pub kind: EdgeKind,
    /// For runtime edges: the requirement as written and the file that states it.
    pub note: Option<String>,
}

/// What depends on one installed runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RuntimeFootprint {
    /// Id of the runtime node.
    pub runtime: String,
    #[ts(type = "number | null")]
    pub install_bytes: Option<u64>,
    /// Projects whose stated requirement this install meets.
    pub dependents: Vec<String>,
    /// Rebuildable artifact folders (`node_modules`, `target`, venvs...) in those
    /// projects. A project met by several installs is counted under each.
    #[ts(type = "number")]
    pub dependent_artifact_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DependencyGraph {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    /// One per installed runtime, largest dependent footprint first.
    pub footprints: Vec<RuntimeFootprint>,
    /// Installs of Node.js, Python, Rust or Go that no project's stated requirement
    /// needs. Not proof they are unused: projects without a stated version may run on
    /// them.
    pub unreferenced_runtimes: Vec<String>,
    /// Projects (ids) that state no runtime version, so no install could be linked.
    pub projects_without_requirements: Vec<String>,
}

fn pm_name(pm: PackageManager) -> &'static str {
    match pm {
        PackageManager::Npm => "npm",
        PackageManager::Pnpm => "pnpm",
        PackageManager::Yarn => "Yarn",
        PackageManager::Bun => "Bun",
        PackageManager::Pip => "pip",
        PackageManager::Poetry => "Poetry",
        PackageManager::Pipenv => "Pipenv",
        PackageManager::Uv => "uv",
        PackageManager::Cargo => "Cargo",
        PackageManager::Maven => "Maven",
        PackageManager::Gradle => "Gradle",
        PackageManager::DotNet => ".NET CLI",
        PackageManager::GoModules => "Go modules",
        PackageManager::Bundler => "Bundler",
    }
}

fn runtime_name(r: Runtime) -> &'static str {
    match r {
        Runtime::Node => "Node.js",
        Runtime::Python => "Python",
        Runtime::Rust => "Rust",
        Runtime::Go => "Go",
        Runtime::Java => "Java",
        Runtime::DotNet => ".NET",
        Runtime::AndroidSdk => "Android SDK",
    }
}

fn project_id(path: &str) -> String {
    format!("project:{}", path.to_lowercase())
}

fn runtime_id(path: &str) -> String {
    format!("runtime:{}", path.to_lowercase())
}

/// Build the graph from the searched projects and an inventory computed from them.
pub fn build_dependency_graph(projects: &[Project], inv: &RuntimeInventory) -> DependencyGraph {
    let mut nodes: BTreeMap<String, GraphNode> = BTreeMap::new();
    let mut edges: Vec<GraphEdge> = Vec::new();
    let mut seen_projects: BTreeSet<String> = BTreeSet::new();
    let mut by_project: BTreeMap<String, &Project> = BTreeMap::new();

    for p in projects {
        let id = project_id(&p.path);
        if !seen_projects.insert(id.clone()) {
            continue;
        }
        nodes.insert(
            id.clone(),
            GraphNode {
                id: id.clone(),
                kind: NodeKind::Project,
                label: p.name.clone(),
                detail: Some(p.path.clone()),
                bytes: p.total_bytes,
            },
        );
        for pm in &p.package_managers {
            let pid = format!("pm:{}", pm_name(*pm).to_lowercase());
            nodes.entry(pid.clone()).or_insert_with(|| GraphNode {
                id: pid.clone(),
                kind: NodeKind::PackageManager,
                label: pm_name(*pm).to_owned(),
                detail: None,
                bytes: None,
            });
            edges.push(GraphEdge {
                from: id.clone(),
                to: pid,
                kind: EdgeKind::Uses,
                note: None,
            });
        }
        by_project.insert(id, p);
    }

    let artifact_bytes = |id: &str| -> u64 {
        by_project
            .get(id)
            .map_or(0, |p| p.artifacts.iter().filter_map(|a| a.bytes).sum())
    };

    let mut footprints = Vec::new();
    let mut unreferenced = Vec::new();
    for i in &inv.installed {
        let rid = runtime_id(&i.path);
        nodes.insert(
            rid.clone(),
            GraphNode {
                id: rid.clone(),
                kind: NodeKind::Runtime,
                label: format!("{} {}", runtime_name(i.runtime), i.version),
                detail: Some(i.path.clone()),
                bytes: i.bytes,
            },
        );
        let mut dependents: Vec<String> = Vec::new();
        for u in &i.used_by {
            let pid = project_id(&u.project);
            if !by_project.contains_key(&pid) {
                continue;
            }
            edges.push(GraphEdge {
                from: pid.clone(),
                to: rid.clone(),
                kind: EdgeKind::Requires,
                note: Some(format!("{} ({})", u.requirement, u.source)),
            });
            if !dependents.contains(&pid) {
                dependents.push(pid);
            }
        }
        let dependent_artifact_bytes = dependents.iter().map(|d| artifact_bytes(d)).sum();
        // Only runtimes whose requirements Sentinel parses can be judged unreferenced.
        if dependents.is_empty()
            && matches!(
                i.runtime,
                Runtime::Node | Runtime::Python | Runtime::Rust | Runtime::Go
            )
        {
            unreferenced.push(rid.clone());
        }
        footprints.push(RuntimeFootprint {
            runtime: rid,
            install_bytes: i.bytes,
            dependents,
            dependent_artifact_bytes,
        });
    }
    footprints.sort_by(|a, b| {
        b.dependent_artifact_bytes
            .cmp(&a.dependent_artifact_bytes)
            .then_with(|| a.runtime.cmp(&b.runtime))
    });

    for u in &inv.unmet {
        let pid = project_id(&u.project);
        if !by_project.contains_key(&pid) {
            continue;
        }
        let mid = format!(
            "missing:{}:{}",
            runtime_name(u.runtime).to_lowercase(),
            u.requirement.to_lowercase()
        );
        nodes.entry(mid.clone()).or_insert_with(|| GraphNode {
            id: mid.clone(),
            kind: NodeKind::MissingRuntime,
            label: format!(
                "{} {} (not installed)",
                runtime_name(u.runtime),
                u.requirement
            ),
            detail: Some(u.requirement.clone()),
            bytes: None,
        });
        edges.push(GraphEdge {
            from: pid,
            to: mid,
            kind: EdgeKind::Unmet,
            note: Some(format!("{} ({})", u.requirement, u.source)),
        });
    }

    let projects_without_requirements = by_project
        .iter()
        .filter(|(_, p)| p.runtimes.is_empty())
        .map(|(id, _)| id.clone())
        .collect();

    DependencyGraph {
        nodes: nodes.into_values().collect(),
        edges,
        footprints,
        unreferenced_runtimes: unreferenced,
        projects_without_requirements,
    }
}
