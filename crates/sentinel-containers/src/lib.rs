//! Read-only inspection of Docker and WSL.
//!
//! Docker is asked through its command-line client with a fixed set of read-only
//! commands (`version`, `ps`, `images`, `volume ls`, `system df`). Nothing is started,
//! stopped, pruned or removed, and Sentinel never launches Docker Desktop: if the daemon
//! is not running it says so. Sizes printed by the CLI are rounded human figures
//! ("1.2GB"), so they are approximate and marked as such.
//!
//! WSL needs no command at all. Distributions are read from the registry and the size of
//! each virtual disk is its file size. A distribution's running state is not shown, since
//! that would mean running `wsl.exe`.
//!
//! Volumes and WSL disks hold user data and are never offered for deletion.

mod docker;
mod prune;
mod wsl;

pub use docker::{
    BuildCacheEntry, DockerContainer, DockerImage, DockerOverview, DockerState, DockerVolume,
    UsageRow, parse_size,
};
pub use prune::{PruneCategory, PruneItem, PrunePreview, PruneRisk, build_preview};
pub use wsl::{WslDisk, WslDistro, WslInventory};

#[cfg(windows)]
pub use docker::inspect_docker;
#[cfg(windows)]
pub use wsl::inspect_wsl;
