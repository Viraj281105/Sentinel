//! Developer tool: list installed software from the uninstall registry. Read-only.
//!
//! `cargo run -p sentinel-inventory --example software`

fn main() {
    let inv = sentinel_inventory::inventory_software();
    for s in &inv.software {
        println!(
            "{:<50} {:<18} {:<28} {:?} {:?} {:?}",
            s.name.chars().take(50).collect::<String>(),
            s.version.as_deref().unwrap_or("-"),
            s.publisher.as_deref().unwrap_or("-"),
            s.scope,
            s.install_date,
            s.reported_bytes
        );
    }
    for a in &inv.store_apps {
        println!(
            "STORE {:?} {} {} {}",
            a.kind, a.name, a.version, a.architecture
        );
    }
    println!(
        "{} store packages ({} skipped)",
        inv.store_apps.len(),
        inv.store_skipped
    );
    println!(
        "{} programs, {} skipped, {} bytes reported, warnings: {:?}",
        inv.software.len(),
        inv.skipped,
        inv.reported_bytes,
        inv.warnings
    );
}
