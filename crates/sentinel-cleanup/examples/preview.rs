//! Developer tool: dry-run preview of the built-in providers on this machine.
//! Read-only: lists folders and reads metadata, never modifies anything.
//!
//! `cargo run -p sentinel-cleanup --example preview`

use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;
use std::time::{SystemTime, UNIX_EPOCH};

use sentinel_cleanup::{Decision, PreviewLimits, preview, providers};
use sentinel_safety::Policy;

fn main() {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let policy = Policy::for_system();
    for p in providers::builtin() {
        let pv = preview(
            p.as_ref(),
            &policy,
            now,
            PreviewLimits::default(),
            &AtomicBool::new(false),
        );
        println!("{} ({:?})", pv.provider.name, pv.roots);
        let mut by: BTreeMap<&str, (u64, u64)> = BTreeMap::new();
        for i in &pv.items {
            let k = match &i.decision {
                Decision::Eligible => "eligible",
                Decision::TooRecent { .. } => "too recent",
                Decision::Protected { .. } => "protected",
                Decision::Skipped { .. } => "skipped",
            };
            let e = by.entry(k).or_default();
            e.0 += 1;
            e.1 += i.bytes;
        }
        for (k, (n, b)) in by {
            println!("  {k:>10}: {n:>5} items, {:.1} MiB", b as f64 / 1048576.0);
        }
        for i in pv.items.iter().filter(|i| {
            matches!(
                i.decision,
                Decision::Skipped { .. } | Decision::Protected { .. }
            )
        }) {
            println!("    {:?}  {}", i.decision, i.path);
        }
    }
}
