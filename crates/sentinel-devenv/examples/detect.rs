//! Developer tool: find projects under a folder (read-only) and print a summary.
//!
//! `cargo run --release -p sentinel-devenv --example detect -- D:\Projects`

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use sentinel_devenv::{DetectOptions, detect};

fn mib(b: u64) -> f64 {
    b as f64 / 1048576.0
}

fn main() {
    let root = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| ".".into()));
    let d = match detect(&root, DetectOptions::default(), &AtomicBool::new(false)) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    println!(
        "{} projects in {} folders ({} ms), truncated: {}, errors: {}",
        d.projects.len(),
        d.folders_visited,
        d.elapsed_ms,
        d.truncated,
        d.errors.len()
    );
    for p in &d.projects {
        let artifacts: u64 = p.artifacts.iter().filter_map(|a| a.bytes).sum();
        println!(
            "  {:<32} {:>9.1} MiB total {:>9.1} MiB artifacts  {:?} {:?} runtimes={:?}{}",
            p.name,
            mib(p.total_bytes.unwrap_or(0)),
            mib(artifacts),
            p.ecosystems,
            p.package_managers,
            p.runtimes
                .iter()
                .map(|r| format!("{:?} {}", r.runtime, r.version))
                .collect::<Vec<_>>(),
            if p.warnings.is_empty() {
                String::new()
            } else {
                format!("  warnings={:?}", p.warnings)
            }
        );
    }
}
