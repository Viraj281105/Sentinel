use sentinel_scanner::drives::{self, Drive};

use super::error::CommandError;

/// List drives with capacity figures. Runs on a blocking worker so a slow volume
/// never stalls the UI thread.
#[tauri::command]
pub(crate) async fn list_drives() -> Result<Vec<Drive>, CommandError> {
    tauri::async_runtime::spawn_blocking(drives::list_drives)
        .await
        .map_err(|e| CommandError::internal("listing drives", e))?
        .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_real_drives_including_the_system_drive() {
        let drives = tauri::async_runtime::block_on(list_drives()).unwrap_or_default();
        assert!(drives.iter().any(|d| d.is_system));
    }
}
