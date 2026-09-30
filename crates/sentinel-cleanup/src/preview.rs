//! Dry-run preview: what a provider would remove, and why each item would or would not
//! be removed. Read-only.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use sentinel_safety::{Policy, SafetyError, TargetKind};
use sentinel_scanner::dirent::{RawEntry, read_dir};
use serde::Serialize;
use ts_rs::TS;

use crate::{CleanupProvider, ProviderInfo};

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ItemKind {
    File,
    Folder,
    /// A junction or symbolic link. Only the link itself would be removed, never what
    /// it points to.
    Link,
}

/// The verdict for one candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase", tag = "state")]
#[ts(export)]
pub enum Decision {
    /// Would be removed (moved to quarantine) by a real run.
    Eligible,
    /// Something inside was modified within the provider's minimum age.
    TooRecent {
        #[ts(type = "number")]
        newest_modified_ms: i64,
    },
    /// The item is, or contains, a protected location.
    Protected { reason: String },
    /// Could not be verified completely, so it is left alone.
    Skipped { reason: String },
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PreviewItem {
    pub path: String,
    pub kind: ItemKind,
    /// Space used on disk (allocated bytes) by the item and everything inside it.
    #[ts(type = "number")]
    pub bytes: u64,
    #[ts(type = "number")]
    pub files: u64,
    /// Newest modification time found in the item, Unix ms.
    #[ts(type = "number | null")]
    pub newest_modified_ms: Option<i64>,
    pub decision: Decision,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase", tag = "state")]
#[ts(export)]
pub enum RootReport {
    Scanned {
        path: String,
    },
    /// The folder does not exist on this machine.
    Missing {
        path: String,
    },
    /// The folder exists but was refused or could not be listed.
    Unavailable {
        path: String,
        reason: String,
    },
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Preview {
    pub provider: ProviderInfo,
    /// Always true: nothing was changed to produce this preview.
    pub dry_run: bool,
    #[ts(type = "number")]
    pub generated_at_ms: i64,
    pub roots: Vec<RootReport>,
    /// Largest first.
    pub items: Vec<PreviewItem>,
    #[ts(type = "number")]
    pub eligible_bytes: u64,
    #[ts(type = "number")]
    pub eligible_files: u64,
    #[ts(type = "number")]
    pub eligible_items: u64,
    /// The preview stopped early (cancelled or hit an entry limit); totals are partial.
    pub incomplete: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct PreviewLimits {
    /// Items deeper than this inside a candidate make it `Skipped`.
    pub max_depth: u32,
    /// Total entries examined across the whole preview.
    pub max_entries: u64,
}

impl Default for PreviewLimits {
    fn default() -> Self {
        Self {
            max_depth: 128,
            max_entries: 2_000_000,
        }
    }
}

struct Measure {
    bytes: u64,
    files: u64,
    newest: i64,
}

enum Walk {
    Protected(String),
    Skipped(String),
}

struct Ctx<'a> {
    policy: &'a Policy,
    limits: PreviewLimits,
    cancel: &'a AtomicBool,
    entries: u64,
    out_of_budget: bool,
}

impl Ctx<'_> {
    /// Measure a folder's contents without following any link, checking every entry
    /// against the protected-path rules.
    fn walk(&mut self, dir: &Path, depth: u32, m: &mut Measure) -> Option<Walk> {
        if self.cancel.load(Ordering::Relaxed) {
            return Some(Walk::Skipped("preview was cancelled".into()));
        }
        if depth > self.limits.max_depth {
            return Some(Walk::Skipped(
                "folder is nested too deeply to verify".into(),
            ));
        }
        let entries = match read_dir(dir) {
            Ok(e) => e,
            Err(err) => {
                return Some(Walk::Skipped(format!(
                    "{} could not be read: {err}",
                    dir.display()
                )));
            }
        };
        self.entries += entries.len() as u64;
        if self.entries > self.limits.max_entries {
            self.out_of_budget = true;
            return Some(Walk::Skipped(
                "too many files to verify in one preview".into(),
            ));
        }
        for e in entries {
            let path = dir.join(&e.name);
            if let Err(err) = self.policy.check_protected(&path) {
                return Some(Walk::Protected(format!("contains a protected item: {err}")));
            }
            m.newest = m.newest.max(e.modified_ms);
            if e.is_dir() && !e.is_reparse_point() {
                if let Some(stop) = self.walk(&path, depth + 1, m) {
                    return Some(stop);
                }
            } else if !e.is_reparse_point() {
                m.files += 1;
                m.bytes += e.allocated_bytes;
            }
            // Links inside the folder are neither followed nor counted: removing the
            // folder would remove the link, never its target.
        }
        None
    }

    fn evaluate(
        &mut self,
        root: &sentinel_safety::AllowedRoot,
        entry: &RawEntry,
        min_age_ms: i64,
        now_ms: i64,
    ) -> PreviewItem {
        let candidate = root.path().join(&entry.name);
        let mut item = PreviewItem {
            path: candidate.display().to_string(),
            kind: ItemKind::File,
            bytes: 0,
            files: 0,
            newest_modified_ms: None,
            decision: Decision::Eligible,
        };
        let target = match self.policy.validate(root, &candidate) {
            Ok(t) => t,
            Err(err @ (SafetyError::Protected { .. } | SafetyError::ContainsProtected { .. })) => {
                item.decision = Decision::Protected {
                    reason: err.to_string(),
                };
                return item;
            }
            Err(err) => {
                item.decision = Decision::Skipped {
                    reason: err.to_string(),
                };
                return item;
            }
        };
        item.path = target.path().display().to_string();
        let mut m = Measure {
            bytes: 0,
            files: 0,
            newest: entry.modified_ms,
        };
        match target.kind() {
            TargetKind::Link => item.kind = ItemKind::Link,
            TargetKind::File => {
                m.bytes = entry.allocated_bytes;
                m.files = 1;
            }
            TargetKind::Directory => {
                item.kind = ItemKind::Folder;
                match self.walk(target.path(), 1, &mut m) {
                    None => {}
                    Some(Walk::Protected(reason)) => {
                        item.decision = Decision::Protected { reason };
                        return item;
                    }
                    Some(Walk::Skipped(reason)) => {
                        item.decision = Decision::Skipped { reason };
                        return item;
                    }
                }
            }
        }
        item.bytes = m.bytes;
        item.files = m.files;
        item.newest_modified_ms = Some(m.newest);
        if now_ms - m.newest < min_age_ms {
            item.decision = Decision::TooRecent {
                newest_modified_ms: m.newest,
            };
        }
        item
    }
}

/// Produce a dry-run preview for `provider`. Reads directory listings and metadata only.
pub fn preview(
    provider: &dyn CleanupProvider,
    policy: &Policy,
    now_ms: i64,
    limits: PreviewLimits,
    cancel: &AtomicBool,
) -> Preview {
    let info = provider.info();
    let min_age_ms = i64::from(info.min_age_days) * DAY_MS;
    let mut ctx = Ctx {
        policy,
        limits,
        cancel,
        entries: 0,
        out_of_budget: false,
    };
    let mut roots = Vec::new();
    let mut items = Vec::new();
    for root in provider.roots() {
        let shown = root.display().to_string();
        if !root.exists() {
            roots.push(RootReport::Missing { path: shown });
            continue;
        }
        let allowed = match policy.allowed_root(&root) {
            Ok(a) => a,
            Err(err) => {
                tracing::warn!(root = %shown, error = %err, "cleanup root refused");
                roots.push(RootReport::Unavailable {
                    path: shown,
                    reason: err.to_string(),
                });
                continue;
            }
        };
        let entries = match read_dir(allowed.path()) {
            Ok(e) => e,
            Err(err) => {
                roots.push(RootReport::Unavailable {
                    path: shown,
                    reason: err.to_string(),
                });
                continue;
            }
        };
        roots.push(RootReport::Scanned {
            path: allowed.path().display().to_string(),
        });
        for e in &entries {
            if cancel.load(Ordering::Relaxed) || ctx.out_of_budget {
                break;
            }
            items.push(ctx.evaluate(&allowed, e, min_age_ms, now_ms));
        }
    }
    items.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.path.cmp(&b.path)));
    let eligible = items.iter().filter(|i| i.decision == Decision::Eligible);
    let (eligible_bytes, eligible_files, eligible_items) =
        eligible.fold((0, 0, 0), |(b, f, n), i| (b + i.bytes, f + i.files, n + 1));
    let incomplete = cancel.load(Ordering::Relaxed) || ctx.out_of_budget;
    tracing::info!(
        provider = info.id,
        items = items.len(),
        eligible_items,
        eligible_bytes,
        incomplete,
        "dry-run preview finished"
    );
    Preview {
        provider: info,
        dry_run: true,
        generated_at_ms: now_ms,
        roots,
        items,
        eligible_bytes,
        eligible_files,
        eligible_items,
        incomplete,
    }
}
