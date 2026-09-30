//! Developer tool: scan a folder read-only and print a summary.
//!
//! `cargo run --release -p sentinel-scanner --example scan -- C:\ [threads]`

use std::path::PathBuf;

use sentinel_scanner::scan::{ScanControl, ScanOptions, ScanTree, scan};

fn gib(b: u64) -> f64 {
    b as f64 / 1024f64.powi(3)
}

fn main() {
    let mut args = std::env::args().skip(1);
    let root = PathBuf::from(args.next().unwrap_or_else(|| ".".into()));
    let threads = args.next().and_then(|t| t.parse().ok()).unwrap_or(4);
    let opts = ScanOptions {
        threads,
        ..ScanOptions::default()
    };
    let tree = match scan(&root, &opts, &ScanControl::new()) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("scan failed: {e}");
            std::process::exit(1);
        }
    };
    println!("{:#?}", tree.stats);
    println!("nodes in memory: {}", tree.nodes.len());
    println!("\nLargest folders under {}:", tree.root.display());
    for id in tree.children_by_size(ScanTree::ROOT).into_iter().take(12) {
        let n = &tree.nodes[id as usize];
        println!(
            "  {:>8.2} GiB  {}  {:?}",
            gib(n.total_bytes),
            n.name,
            n.status
        );
    }
    println!("\nLargest files:");
    for f in tree.largest_files.iter().take(5) {
        println!("  {:>8.2} GiB  {}", gib(f.bytes), f.path);
    }
}
