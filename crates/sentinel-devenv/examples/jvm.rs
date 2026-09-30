//! Developer tool: analyze this user's Maven repository and Gradle home, relating them to
//! projects found under a folder. Read-only.
//!
//! `cargo run -p sentinel-devenv --example jvm -- D:\Projects`

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use sentinel_devenv::{DetectOptions, analyze_jvm_caches, detect};

fn mib(b: u64) -> f64 {
    b as f64 / 1048576.0
}

fn main() {
    let profile = PathBuf::from(std::env::var_os("USERPROFILE").unwrap_or_default());
    let projects: Vec<PathBuf> = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .and_then(|root| {
            let opts = DetectOptions {
                measure: false,
                ..DetectOptions::default()
            };
            detect(&root, opts, &AtomicBool::new(false)).ok()
        })
        .map(|d| d.projects.iter().map(|p| PathBuf::from(&p.path)).collect())
        .unwrap_or_default();
    let a = analyze_jvm_caches(
        Some(&profile.join(".m2").join("repository")),
        Some(&profile.join(".gradle")),
        &projects,
    );
    println!(
        "{} projects read, warnings: {:?}",
        a.projects_read, a.warnings
    );
    match &a.maven {
        Some(m) => {
            println!(
                "Maven {}: {:.1} MiB, {} versions, {} multi-version artifacts ({:.1} MiB older), {} failed downloads, truncated {}",
                m.root,
                mib(m.total_bytes),
                m.artifact_versions,
                m.multi_version_artifacts,
                mib(m.older_versions_bytes),
                m.failed_downloads,
                m.truncated
            );
            for g in m.groups.iter().take(8) {
                println!(
                    "  {:>8.1} MiB  {} ({} versions)",
                    mib(g.bytes),
                    g.group,
                    g.versions
                );
            }
            for x in m
                .artifacts
                .iter()
                .filter(|x| x.versions.iter().any(|v| !v.declared_by.is_empty()))
                .take(8)
            {
                println!(
                    "  declared: {}:{} {:?}",
                    x.group,
                    x.artifact,
                    x.versions
                        .iter()
                        .map(|v| (&v.version, &v.declared_by))
                        .collect::<Vec<_>>()
                );
            }
        }
        None => println!("Maven: no local repository"),
    }
    match &a.gradle {
        Some(g) => println!(
            "Gradle {}: {:.1} MiB, {} versions",
            g.root,
            mib(g.total_bytes),
            g.versions.len()
        ),
        None => println!("Gradle: no Gradle home"),
    }
}
