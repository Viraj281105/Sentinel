//! Cleanup providers and dry-run previews.
//!
//! **This crate cannot modify the filesystem.** It contains no call that deletes,
//! moves, renames or writes anything; it only lists directories and reads metadata.
//! Execution (quarantine, restore, audit) will live in a separate executor that accepts
//! only `sentinel_safety::ValidatedTarget`s and revalidates them.
//!
//! A provider declares *where* candidates live and *what they are*. The framework in
//! [`preview`] decides, deterministically, which candidates would be eligible: every
//! candidate is validated by `sentinel-safety`, every item inside it is checked against
//! the protected-path rules, and anything modified too recently is left alone.

mod preview;
pub mod providers;

use std::path::PathBuf;

use sentinel_classify::Category;
use sentinel_safety::RiskLevel;
use serde::Serialize;
use ts_rs::TS;

pub use preview::{
    Decision, ItemKind, Preview, PreviewItem, PreviewLimits, RootReport, assess, preview,
};

/// Risk shown to users; mirrors [`RiskLevel`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Risk {
    Safe,
    LowRisk,
    MediumRisk,
    HighRisk,
    Protected,
}

impl From<RiskLevel> for Risk {
    fn from(r: RiskLevel) -> Self {
        match r {
            RiskLevel::Safe => Self::Safe,
            RiskLevel::LowRisk => Self::LowRisk,
            RiskLevel::MediumRisk => Self::MediumRisk,
            RiskLevel::HighRisk => Self::HighRisk,
            RiskLevel::Protected => Self::Protected,
        }
    }
}

/// What a provider cleans and why it is considered removable.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProviderInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub category: Category,
    pub risk: Risk,
    /// What the items are.
    pub description: &'static str,
    /// What happens if they are removed, and whether they come back.
    pub on_removal: &'static str,
    /// Items with anything modified more recently than this are never eligible.
    pub min_age_days: u32,
}

/// A source of cleanup candidates.
///
/// Providers only describe; they never decide safety. Candidates are the direct
/// children of each root, and every one is validated by the policy engine.
pub trait CleanupProvider: Send + Sync {
    fn info(&self) -> ProviderInfo;

    /// Folders whose direct children are candidates. Missing folders are reported,
    /// not treated as errors.
    fn roots(&self) -> Vec<PathBuf>;
}
