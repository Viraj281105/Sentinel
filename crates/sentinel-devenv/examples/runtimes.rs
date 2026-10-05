//! Developer tool: list the runtimes installed for this user, relating them to projects
//! found under a folder. Read-only (a standalone Node.js install is asked its version).
//!
//! `cargo run -p sentinel-devenv --example runtimes -- D:\Projects`

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use sentinel_devenv::{DetectOptions, Locations, detect, inventory_runtimes};

fn mib(b: Option<u64>) -> String {
    b.map_or_else(|| "?".into(), |b| format!("{:.1} MB", b as f64 / 1048576.0))
}

fn main() {
    let projects = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .and_then(|root| {
            let opts = DetectOptions {
                measure: false,
                ..DetectOptions::default()
            };
            detect(&root, opts, &AtomicBool::new(false)).ok()
        })
        .map(|d| d.projects)
        .unwrap_or_default();
    let inv = inventory_runtimes(&Locations::from_env(), &projects);
    println!(
        "{} installs, {} projects compared",
        inv.installed.len(),
        inv.projects_read
    );
    for i in &inv.installed {
        println!(
            "{:?} {} [{}] via {} {} {}",
            i.runtime,
            i.version,
            i.label,
            i.installed_by,
            mib(i.bytes),
            i.path
        );
        for u in &i.used_by {
            println!("    used by {} ({} {})", u.project, u.source, u.requirement);
        }
    }
    for u in &inv.unmet {
        println!(
            "UNMET {:?} {} ({}) in {}",
            u.runtime, u.requirement, u.source, u.project
        );
    }
}
