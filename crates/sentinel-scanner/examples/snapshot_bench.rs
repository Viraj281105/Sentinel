#![allow(clippy::expect_used)]
//! Cost of saving and restoring the retained scan tree, on a synthetic tree.
//!
//! No disk scan: a tree of N folders (default 400,000, the app's retention limit) is built in
//! memory, encoded, decoded and validated, and the sizes and times are printed.
//!
//!     cargo run --release -p sentinel-scanner --example snapshot_bench -- 400000

use std::path::PathBuf;
use std::time::Instant;

use sentinel_scanner::persist::{Snapshot, decode, encode};
use sentinel_scanner::scan::{DirNode, NodeStatus, ScanStats, ScanTree};
use sentinel_scanner::usn::JournalPosition;

fn synthetic(n: usize) -> ScanTree {
    let mut nodes: Vec<DirNode> = Vec::with_capacity(n);
    for i in 0..n {
        // A wide, shallow-ish tree: node i hangs off node (i - 1) / 8.
        let parent = (i > 0).then(|| ((i - 1) / 8) as u32);
        nodes.push(DirNode {
            name: format!("folder-{i:07}"),
            parent,
            children: Vec::new(),
            own_bytes: (i as u64 % 977) * 4096,
            total_bytes: (i as u64 % 977) * 4096,
            total_logical_bytes: (i as u64 % 977) * 4000,
            file_count: i as u64 % 40,
            dir_count: 0,
            own_newest_ms: Some(1_700_000_000_000 + i as i64),
            newest_ms: Some(1_700_000_000_000 + i as i64),
            status: NodeStatus::Complete,
        });
    }
    for i in 1..n {
        let p = (i - 1) / 8;
        nodes[p].children.push(i as u32);
    }
    ScanTree {
        root: PathBuf::from(r"D:\"),
        nodes,
        largest_files: Vec::new(),
        stats: ScanStats::default(),
    }
}

fn main() {
    let n: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(400_000);
    let t = Instant::now();
    let tree = synthetic(n);
    println!("built {n} folders in {:?}", t.elapsed());

    let snap = Snapshot {
        scan_id: 1,
        journal: JournalPosition {
            journal_id: 7,
            next_usn: 1000,
        },
        saved_at_ms: 0,
        tree,
    };
    let t = Instant::now();
    let bytes = encode(&snap).expect("encode");
    let enc = t.elapsed();
    let t = Instant::now();
    let back = decode(&bytes).expect("decode");
    let dec = t.elapsed();
    assert_eq!(back.tree.nodes.len(), n);
    println!(
        "snapshot: {:.1} MiB ({:.0} bytes per folder); encode {enc:?}, decode+validate {dec:?}",
        bytes.len() as f64 / (1024.0 * 1024.0),
        bytes.len() as f64 / n as f64,
    );
}
