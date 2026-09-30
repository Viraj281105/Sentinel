//! Cleanup commands. Dry-run only: nothing here can modify the filesystem.

use std::sync::atomic::AtomicBool;

use sentinel_cleanup::{CleanupProvider, Preview, PreviewLimits, ProviderInfo, preview, providers};
use sentinel_safety::Policy;
use tauri::State;

use super::error::{CommandError, ErrorKind};
use crate::AppState;
use crate::db::now_ms;

#[tauri::command]
pub(crate) fn cleanup_providers() -> Vec<ProviderInfo> {
    providers::builtin().iter().map(|p| p.info()).collect()
}

/// Show what the provider would remove, without removing anything.
#[tauri::command]
pub(crate) async fn cleanup_preview(
    state: State<'_, AppState>,
    provider: String,
) -> Result<Preview, CommandError> {
    let policy = state.policy.clone();
    tauri::async_runtime::spawn_blocking(move || {
        run_preview(&providers::builtin(), &policy, &provider)
    })
    .await
    .map_err(|e| CommandError::internal("previewing cleanup", e))?
}

fn run_preview(
    all: &[Box<dyn CleanupProvider>],
    policy: &Policy,
    id: &str,
) -> Result<Preview, CommandError> {
    let p = all.iter().find(|p| p.info().id == id).ok_or_else(|| {
        CommandError::new(
            ErrorKind::NotFound,
            format!("There is no cleanup type called {id}."),
        )
    })?;
    Ok(preview(
        p.as_ref(),
        policy,
        now_ms(),
        PreviewLimits::default(),
        &AtomicBool::new(false),
    ))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use sentinel_cleanup::providers::UserTemp;
    use sentinel_safety::ProtectedSet;

    use super::*;

    #[test]
    fn previews_a_known_provider_and_rejects_unknown_ids() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("f.tmp"), b"x").unwrap();
        let all: Vec<Box<dyn CleanupProvider>> =
            vec![Box::new(UserTemp::with_root(dir.path().to_path_buf()))];
        let policy = Policy::new(ProtectedSet::new());
        let p = run_preview(&all, &policy, "user-temp").unwrap();
        assert!(p.dry_run);
        assert_eq!(p.items.len(), 1);
        let err = run_preview(&all, &policy, "nope").unwrap_err();
        assert_eq!(err.kind, ErrorKind::NotFound);
    }

    #[test]
    fn lists_builtin_providers() {
        let ids: Vec<_> = cleanup_providers().iter().map(|p| p.id).collect();
        assert_eq!(ids, ["user-temp"]);
    }
}
