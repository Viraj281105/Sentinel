//! Dependency graph on synthetic installs and projects.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::Path;

use sentinel_devenv::{
    Artifact, ArtifactKind, EdgeKind, Locations, NodeKind, PackageManager, Project, Runtime,
    RuntimeRequirement, build_dependency_graph, inventory_runtimes,
};

fn put(path: &Path) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, "x").unwrap();
}

fn project(path: &str, reqs: &[(Runtime, &str, &str)], artifact_bytes: u64) -> Project {
    Project {
        path: path.to_owned(),
        name: path.rsplit('\\').next().unwrap().to_owned(),
        ecosystems: vec![],
        package_managers: vec![PackageManager::Npm],
        markers: vec![],
        runtimes: reqs
            .iter()
            .map(|(runtime, version, source)| RuntimeRequirement {
                runtime: *runtime,
                version: (*version).to_owned(),
                source: (*source).to_owned(),
            })
            .collect(),
        git: false,
        last_activity_ms: None,
        total_bytes: Some(artifact_bytes * 2),
        artifacts: vec![Artifact {
            kind: ArtifactKind::NodeModules,
            path: format!("{path}\\node_modules"),
            bytes: Some(artifact_bytes),
            restored_by: String::new(),
        }],
        warnings: vec![],
    }
}

#[test]
fn links_projects_to_the_installs_that_meet_their_requirements() {
    let dir = tempfile::tempdir().unwrap();
    let u = dir.path().join("u");
    put(&u.join("Roaming/nvm/v22.4.0/node.exe"));
    put(&u.join("Roaming/nvm/v18.19.1/node.exe"));
    put(&u.join("Roaming/nvm/v20.0.0/node.exe"));
    put(&u.join(".pyenv/pyenv-win/versions/3.12.4/python.exe"));
    let loc = Locations {
        profile: Some(u.clone()),
        local_app_data: Some(u.join("Local")),
        app_data: Some(u.join("Roaming")),
        ..Locations::default()
    };

    let projects = vec![
        project(
            "D:\\p\\web",
            &[(Runtime::Node, ">=20", "package.json engines.node")],
            1000,
        ),
        project("D:\\p\\api", &[(Runtime::Node, "22", ".nvmrc")], 500),
        project("D:\\p\\old", &[(Runtime::Node, "16", ".nvmrc")], 70),
        project("D:\\p\\plain", &[], 9),
        project("D:\\P\\WEB", &[(Runtime::Node, ">=99", "x")], 1), // duplicate path, ignored
    ];
    let inv = inventory_runtimes(&loc, &projects);
    let g = build_dependency_graph(&projects, &inv);

    let node = |label: &str| g.nodes.iter().find(|n| n.label == label).unwrap();
    let n22 = node("Node.js 22.4.0");
    let n20 = node("Node.js 20.0.0");
    let n18 = node("Node.js 18.19.1");
    assert_eq!(n22.kind, NodeKind::Runtime);

    let requires = |to: &str| -> Vec<&str> {
        g.edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Requires && e.to == to)
            .map(|e| e.from.as_str())
            .collect()
    };
    let mut on22 = requires(&n22.id);
    on22.sort_unstable();
    assert_eq!(on22, ["project:d:\\p\\api", "project:d:\\p\\web"]);
    assert_eq!(
        requires(&n20.id),
        ["project:d:\\p\\web"],
        "20 meets >=20 but not 22"
    );
    assert!(requires(&n18.id).is_empty());

    // Footprint: artifacts of the projects that depend on the install.
    let fp = |id: &str| g.footprints.iter().find(|f| f.runtime == id).unwrap();
    assert_eq!(fp(&n22.id).dependent_artifact_bytes, 1500);
    assert_eq!(fp(&n20.id).dependent_artifact_bytes, 1000);
    assert_eq!(g.footprints[0].runtime, n22.id, "largest footprint first");

    // Installs nothing states a need for; Python is judged too, but not Java/.NET.
    assert!(g.unreferenced_runtimes.contains(&n18.id));
    assert!(g.unreferenced_runtimes.contains(&node("Python 3.12.4").id));
    assert!(!g.unreferenced_runtimes.contains(&n22.id));

    // The unmet requirement points at a placeholder node.
    let unmet: Vec<_> = g
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Unmet)
        .collect();
    assert_eq!(unmet.len(), 1);
    assert_eq!(unmet[0].from, "project:d:\\p\\old");
    let missing = g.nodes.iter().find(|n| n.id == unmet[0].to).unwrap();
    assert_eq!(missing.kind, NodeKind::MissingRuntime);
    assert!(missing.label.contains("16") && missing.label.contains("not installed"));

    // A project with no stated version is listed, never linked to an install.
    assert_eq!(g.projects_without_requirements, ["project:d:\\p\\plain"]);
    assert!(
        g.edges
            .iter()
            .all(|e| e.from != "project:d:\\p\\plain" || e.kind == EdgeKind::Uses)
    );

    // Package managers are shared nodes: one npm node for four projects.
    let npm: Vec<_> = g
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::PackageManager)
        .collect();
    assert_eq!(npm.len(), 1);
    assert_eq!(
        g.edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Uses && e.to == npm[0].id)
            .count(),
        4
    );
}

#[test]
fn empty_inputs_give_an_empty_graph() {
    let inv = inventory_runtimes(&Locations::default(), &[]);
    let g = build_dependency_graph(&[], &inv);
    assert!(g.nodes.is_empty() && g.edges.is_empty() && g.footprints.is_empty());
}
