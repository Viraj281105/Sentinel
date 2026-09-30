use std::path::PathBuf;

use sentinel_classify::Category;
use sentinel_safety::{Known, RiskLevel, known_folder};

use crate::{CleanupProvider, ProviderInfo};

/// The current user's temporary folder, `%LOCALAPPDATA%\Temp`.
///
/// The location comes from the shell's known-folder API, not from `%TEMP%`, which any
/// process can point somewhere else.
pub struct UserTemp {
    root: Option<PathBuf>,
}

impl UserTemp {
    pub fn for_system() -> Self {
        Self {
            root: known_folder(Known::LocalAppData).map(|p| p.join("Temp")),
        }
    }

    /// A provider rooted somewhere else. For tests and fixtures.
    pub fn with_root(root: PathBuf) -> Self {
        Self { root: Some(root) }
    }
}

impl CleanupProvider for UserTemp {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: "user-temp",
            name: "Temporary files for your account",
            category: Category::TemporaryFiles,
            risk: RiskLevel::Safe.into(),
            description: "Files that programs create in your temporary folder for short-lived \
                          use, such as installer leftovers and extracted archives.",
            on_removal: "Programs recreate temporary files when they need them. Anything \
                         changed in the last 7 days is left alone because a running program \
                         may still be using it.",
            min_age_days: 7,
            can_clean: true,
            note: None,
        }
    }

    fn roots(&self) -> Vec<PathBuf> {
        self.root.iter().cloned().collect()
    }
}
