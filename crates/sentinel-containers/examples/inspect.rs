//! Developer tool: inspect Docker (through its CLI) and WSL. Read-only.
//!
//! `cargo run -p sentinel-containers --example inspect`

fn main() {
    let d = sentinel_containers::inspect_docker();
    println!(
        "docker: {:?} cli={:?} version={:?}",
        d.state, d.cli_path, d.server_version
    );
    for u in &d.usage {
        println!("  {u:?}");
    }
    println!(
        "  {} containers, {} images, {} volumes, warnings {:?}",
        d.containers.len(),
        d.images.len(),
        d.volumes.len(),
        d.warnings
    );
    if let Some(p) = &d.prune_preview {
        for c in &p.categories {
            println!(
                "PREVIEW {:<32} {:?} items={} item_bytes={:?} docker_reclaimable={:?}  [{}]",
                c.title,
                c.risk,
                c.items.len(),
                c.item_bytes,
                c.docker_reclaimable_bytes,
                c.command
            );
        }
    }
    let w = sentinel_containers::inspect_wsl();
    for x in &w.distros {
        println!(
            "wsl: {} v{:?} default={} docker={} {} bytes in {} disks",
            x.name,
            x.version,
            x.is_default,
            x.docker_desktop,
            x.disk_bytes,
            x.disks.len()
        );
    }
    println!(
        "wsl total {} bytes, warnings {:?}",
        w.total_disk_bytes, w.warnings
    );
}
