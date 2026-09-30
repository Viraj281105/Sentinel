//! Developer tool: find projects under a folder and dry-run the inactive-project cleanup
//! for all of them. Read-only.
//!
//! `cargo run -p sentinel-cleanup --example project_preview -- D:\Projects`

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::{SystemTime, UNIX_EPOCH};

use sentinel_cleanup::providers::ProjectArtifacts;
use sentinel_cleanup::{PreviewLimits, preview};
use sentinel_devenv::{DetectOptions, detect};
use sentinel_safety::Policy;

fn main() {
    let root = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| ".".into()));
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let opts = DetectOptions {
        measure: false,
        ..DetectOptions::default()
    };
    let found = match detect(&root, opts, &AtomicBool::new(false)) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    let paths: Vec<PathBuf> = found
        .projects
        .iter()
        .map(|p| PathBuf::from(&p.path))
        .collect();
    let provider = ProjectArtifacts::for_projects(&paths, now);
    let pv = preview(
        &provider,
        &Policy::for_system(),
        now,
        PreviewLimits::default(),
        &AtomicBool::new(false),
    );
    println!(
        "{} projects; {} eligible items, {:.1} MiB",
        paths.len(),
        pv.eligible_items,
        pv.eligible_bytes as f64 / 1048576.0
    );
    for i in &pv.items {
        println!(
            "  {:>9.1} MiB  {:?}  {}",
            i.bytes as f64 / 1048576.0,
            i.decision,
            i.path
        );
    }
}
