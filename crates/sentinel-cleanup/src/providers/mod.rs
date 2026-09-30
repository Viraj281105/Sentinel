//! Built-in cleanup providers.

mod package_cache;
mod project_artifacts;
mod user_temp;

pub use package_cache::{CacheKind, PackageCache};
pub use project_artifacts::{INACTIVE_DAYS, ProjectArtifacts};
pub use user_temp::UserTemp;

use crate::CleanupProvider;

/// All built-in providers for this machine.
pub fn builtin() -> Vec<Box<dyn CleanupProvider>> {
    vec![
        Box::new(UserTemp::for_system()),
        Box::new(PackageCache::for_system(CacheKind::Npm)),
        Box::new(PackageCache::for_system(CacheKind::Yarn)),
        Box::new(PackageCache::for_system(CacheKind::Pip)),
        Box::new(PackageCache::for_system(CacheKind::Pnpm)),
    ]
}
