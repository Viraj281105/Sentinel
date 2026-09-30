//! Developer tool: scan a folder read-only and print its category breakdown, plus the
//! largest folders that no rule classifies (candidates for new rules).
//!
//! `cargo run --release -p sentinel-classify --example breakdown -- C:\`

use std::path::PathBuf;

use sentinel_classify::Classifier;
use sentinel_scanner::scan::{ScanControl, ScanOptions, scan};

fn gib(b: u64) -> f64 {
    b as f64 / 1024f64.powi(3)
}

fn main() {
    let root = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| ".".into()));
    let tree = match scan(&root, &ScanOptions::default(), &ScanControl::new()) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("scan failed: {e}");
            std::process::exit(1);
        }
    };
    let c = Classifier::for_system();
    let total = tree.stats.total_bytes.max(1);
    println!(
        "Breakdown of {} ({:.1} GiB):",
        tree.root.display(),
        gib(total)
    );
    for b in c.breakdown(&tree) {
        println!(
            "  {:>8.2} GiB  {:>5.1}%  {:?}",
            gib(b.bytes),
            b.bytes as f64 * 100.0 / total as f64,
            b.category
        );
    }

    // Unclassified folders up to three levels deep, by size.
    let mut unknown: Vec<(u64, u64, PathBuf)> = Vec::new();
    let mut depth = vec![0u32; tree.nodes.len()];
    for (i, n) in tree.nodes.iter().enumerate() {
        if let Some(p) = n.parent {
            depth[i] = depth[p as usize] + 1;
        }
        if depth[i] == 0 || depth[i] > 3 || n.total_bytes < 256 * 1024 * 1024 {
            continue;
        }
        let Some(path) = tree.path_of(i as u32) else {
            continue;
        };
        if c.classify(&path).is_none() {
            unknown.push((n.total_bytes, n.own_bytes, path));
        }
    }
    unknown.sort_by_key(|u| std::cmp::Reverse(u.0));
    println!("\nLargest unclassified folders (total / own files):");
    for (t, o, p) in unknown.iter().take(20) {
        println!("  {:>8.2} / {:>6.2} GiB  {}", gib(*t), gib(*o), p.display());
    }
}
