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
    println!(
        "{} programs, {} skipped, {} bytes reported, warnings: {:?}",
        inv.software.len(),
        inv.skipped,
        inv.reported_bytes,
        inv.warnings
    );
}
