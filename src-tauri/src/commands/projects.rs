//! Project detection commands. Read-only with respect to the searched folders.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use sentinel_devenv::{DetectOptions, Detection, detect};
use serde::Serialize;
use tauri::State;
use ts_rs::TS;

use super::error::{CommandError, ErrorKind};
use crate::AppState;
use crate::db::{Db, now_ms};

/// The latest search of one folder.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProjectSearch {
    pub root: String,
    #[ts(type = "number")]
    pub searched_at_ms: i64,
    pub detection: Detection,
}

fn saved_searches(db: &Db) -> Result<Vec<ProjectSearch>, CommandError> {
    let rows = db.lock().project_searches()?;
    Ok(rows
        .into_iter()
        .filter_map(|r| match serde_json::from_str::<Detection>(&r.result_json) {
            Ok(detection) => Some(ProjectSearch {
                root: r.root,
                searched_at_ms: r.searched_at_ms,
                detection,
            }),
            Err(err) => {
                tracing::warn!(root = %r.root, error = %err, "saved project search unreadable; ignored");
                None
            }
        })
        .collect())
}

#[tauri::command]
pub(crate) fn project_searches(
    state: State<'_, AppState>,
) -> Result<Vec<ProjectSearch>, CommandError> {
    saved_searches(&state.db)
}

fn search(db: &Db, root: &str, opts: DetectOptions) -> Result<ProjectSearch, CommandError> {
    let path = PathBuf::from(root);
    if !path.is_absolute() || !path.is_dir() {
        return Err(CommandError::new(
            ErrorKind::InvalidInput,
            format!("{root} is not a folder that can be searched."),
        ));
    }
    let detection = detect(&path, opts, &AtomicBool::new(false))
        .map_err(|e| CommandError::new(ErrorKind::InvalidInput, e.to_string()))?;
    let at = now_ms();
    let json = serde_json::to_string(&detection)
        .map_err(|e| CommandError::internal("saving the project search", e))?;
    db.lock().save_project_search(&detection.root, at, &json)?;
    Ok(ProjectSearch {
        root: detection.root.clone(),
        searched_at_ms: at,
        detection,
    })
}

/// Search a folder for projects and save the result, replacing any earlier search of it.
#[tauri::command]
pub(crate) async fn find_projects(
    state: State<'_, AppState>,
    root: String,
) -> Result<ProjectSearch, CommandError> {
    let db = state.db.clone();
    tauri::async_runtime::spawn_blocking(move || search(&db, &root, DetectOptions::default()))
        .await
        .map_err(|e| CommandError::internal("searching for projects", e))?
}

/// Forget a searched folder (nothing on disk is touched).
#[tauri::command]
pub(crate) fn remove_project_search(
    state: State<'_, AppState>,
    root: String,
) -> Result<bool, CommandError> {
    Ok(state.db.lock().remove_project_search(&root)?)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn searches_save_and_reload() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("web")).unwrap();
        std::fs::write(
            dir.path().join("web").join("package.json"),
            r#"{"name":"web"}"#,
        )
        .unwrap();
        let db = Db::in_memory();
        let opts = DetectOptions {
            measure: false,
            ..DetectOptions::default()
        };
        let s = search(&db, dir.path().to_str().unwrap(), opts).unwrap();
        assert_eq!(s.detection.projects.len(), 1);
        let saved = saved_searches(&db).unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].detection.projects[0].name, "web");
    }

    #[test]
    fn rejects_relative_and_missing_folders() {
        let db = Db::in_memory();
        for bad in ["relative", r"Z:\definitely\not\here"] {
            let err = search(&db, bad, DetectOptions::default()).unwrap_err();
            assert_eq!(err.kind, ErrorKind::InvalidInput);
        }
    }
}
