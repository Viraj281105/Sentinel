//! Docker and WSL inspection. Read-only; Docker is asked through its CLI with fixed
//! arguments and is never started.

use sentinel_containers::{DockerOverview, WslInventory, inspect_docker, inspect_wsl};

use super::error::CommandError;

#[tauri::command]
pub(crate) async fn docker_overview() -> Result<DockerOverview, CommandError> {
    tauri::async_runtime::spawn_blocking(inspect_docker)
        .await
        .map_err(|e| CommandError::internal("inspecting Docker", e))
}

#[tauri::command]
pub(crate) async fn wsl_distros() -> Result<WslInventory, CommandError> {
    tauri::async_runtime::spawn_blocking(inspect_wsl)
        .await
        .map_err(|e| CommandError::internal("reading WSL distributions", e))
}
