//! Installed software inventory. Read-only.

use sentinel_inventory::{SoftwareInventory, inventory_software};

use super::error::CommandError;

/// Installed desktop software from the Windows uninstall registry.
#[tauri::command]
pub(crate) async fn software_inventory() -> Result<SoftwareInventory, CommandError> {
    tauri::async_runtime::spawn_blocking(inventory_software)
        .await
        .map_err(|e| CommandError::internal("reading installed software", e))
}
