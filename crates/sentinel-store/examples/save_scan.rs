//! Developer tool: scan a folder read-only, save it to a throwaway database in the
//! system temp directory, and report timings and database size.
//!
//! `cargo run --release -p sentinel-store --example save_scan -- C:\`

use std::path::PathBuf;
use std::time::Instant;

use sentinel_scanner::scan::{ScanControl, ScanOptions, scan};
use sentinel_store::{Retention, Store};

fn main() {
    let root = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| ".".into()));
    let tree = match scan(&root, &ScanOptions::default(), &ScanControl::new()) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("scan failed: {e}");
            std::process::exit(1);
        }
    };
    let db = std::env::temp_dir().join(format!("sentinel-bench-{}.db", std::process::id()));
    let run = || -> Result<(), sentinel_store::StoreError> {
        let mut store = Store::open(&db)?;
        let t = Instant::now();
        let id = store.save_scan(&tree, 0, 1, Retention::default())?;
        let save = t.elapsed();
        let t = Instant::now();
        let kids = store.children(id, 0, 200)?;
        let list = t.elapsed();
        println!(
            "scanned nodes: {}, save: {save:?}, root listing ({} children): {list:?}",
            tree.nodes.len(),
            kids.len()
        );
        Ok(())
    };
    if let Err(e) = run() {
        eprintln!("store failed: {e}");
    }
    let size: u64 = ["", "-wal", "-shm"]
        .iter()
        .filter_map(|s| std::fs::metadata(format!("{}{s}", db.display())).ok())
        .map(|m| m.len())
        .sum();
    println!("database size: {:.1} MiB", size as f64 / 1048576.0);
    for s in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{s}", db.display()));
    }
}
