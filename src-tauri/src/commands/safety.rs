use sentinel_safety::Policy;
use serde::Serialize;
use tauri::State;
use ts_rs::TS;

use crate::AppState;

/// A location Sentinel will never modify, with the reason it is protected.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProtectedLocation {
    pub path: String,
    pub reason: String,
}

#[tauri::command]
pub(crate) fn protected_locations(state: State<'_, AppState>) -> Vec<ProtectedLocation> {
    list_protected(&state.policy)
}

fn list_protected(policy: &Policy) -> Vec<ProtectedLocation> {
    let mut out: Vec<_> = policy
        .protected()
        .roots()
        .map(|(path, reason)| ProtectedLocation {
            path: path.display().to_string(),
            reason: reason.to_owned(),
        })
        .collect();
    out.sort_by_key(|l| l.path.to_lowercase());
    out.dedup_by(|a, b| a.path.eq_ignore_ascii_case(&b.path));
    out
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use sentinel_safety::ProtectedSet;

    use super::*;

    #[test]
    fn lists_registered_roots_sorted_and_deduplicated() {
        let mut set = ProtectedSet::new();
        set.add_root(Path::new(r"C:\Zeta"), "z");
        set.add_root(Path::new(r"C:\alpha"), "a");
        set.add_root(Path::new(r"C:\ALPHA"), "a again");
        let got = list_protected(&Policy::new(set));
        let paths: Vec<_> = got.iter().map(|l| l.path.as_str()).collect();
        assert_eq!(paths, [r"C:\alpha", r"C:\Zeta"]);
    }

    #[test]
    fn system_policy_lists_windows_directory() {
        let got = list_protected(&Policy::for_system());
        assert!(
            got.iter()
                .any(|l| l.path.eq_ignore_ascii_case(r"C:\Windows"))
        );
    }
}
