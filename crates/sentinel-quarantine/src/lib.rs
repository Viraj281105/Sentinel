//! Quarantine executor: moves approved cleanup items into a private quarantine folder,
//! restores them, and removes them once expired.
//!
//! Guarantees:
//! - **Audit first.** A `Started` audit record is written before anything is touched;
//!   if it cannot be written, nothing happens. An outcome record follows.
//! - **Only what was approved, only if still eligible.** Each approved path is
//!   re-assessed with `sentinel_cleanup::assess` immediately before it moves (validation,
//!   protected descendants, age). Anything that changed since the preview is skipped.
//! - **Move by verified handle.** The item is opened with
//!   `ValidatedTarget::open_verified` and renamed through that handle, never replacing
//!   anything, never crossing volumes. Items in use by another program are skipped.
//! - **Manifest before move.** Each operation folder holds `manifest.json` (original
//!   path, time, provider, operation id, status), written before and after every move.
//! - **Restore never overwrites**, and re-checks the destination against the policy.
//! - **Purge** permanently removes only expired items inside the quarantine folder and
//!   never follows links.
//!
//! Location: `%LOCALAPPDATA%\Sentinel\.sentinel-quarantine`, which is private to the
//! user and on the same volume as the user's TEMP folder. Items on other volumes are
//! refused until a per-volume location with an owner-only ACL is designed.

mod rename;

use std::fs;
use std::io;
use std::os::windows::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

use sentinel_cleanup::{CleanupProvider, Decision, ItemKind, PreviewLimits, assess};
use sentinel_safety::{CanonicalPath, Known, Policy, SafetyError, is_within, known_folder};
use sentinel_store::{Approval, AuditKind, NewAuditRecord, Outcome};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::rename::{is_in_use, rename_by_handle};

pub const DIR_NAME: &str = ".sentinel-quarantine";
pub const RETENTION_MS: i64 = 14 * 24 * 60 * 60 * 1000;
const MANIFEST: &str = "manifest.json";
const MANIFEST_VERSION: u32 = 1;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

#[derive(Debug, thiserror::Error)]
pub enum QuarantineError {
    #[error("the audit log could not be written, so nothing was changed: {0}")]
    AuditUnavailable(String),
    #[error("there is no quarantine folder for this account")]
    NoLocation,
    #[error("the quarantine folder is not safe to use: {0}")]
    UnsafeLocation(String),
    #[error("{0}")]
    Refused(String),
    #[error("quarantine operation {0} was not found")]
    NotFound(String),
    #[error("the quarantine record is unreadable: {0}")]
    BadManifest(String),
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: io::Error },
}

fn io_err(path: &Path) -> impl FnOnce(io::Error) -> QuarantineError + '_ {
    move |source| QuarantineError::Io {
        path: path.to_path_buf(),
        source,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "state"
)]
#[ts(export)]
pub enum EntryStatus {
    /// Approved; not moved yet.
    Pending,
    Quarantined,
    Restored,
    /// Permanently removed after expiry.
    Purged,
    /// Left in place, e.g. it changed since the preview or is in use.
    Skipped {
        reason: String,
    },
    Failed {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ManifestEntry {
    pub index: u32,
    pub original_path: String,
    pub kind: ItemKind,
    #[ts(type = "number")]
    pub bytes: u64,
    #[ts(type = "number")]
    pub files: u64,
    pub status: EntryStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Manifest {
    pub version: u32,
    pub operation_id: String,
    pub provider: String,
    pub user: String,
    #[ts(type = "number")]
    pub created_at_ms: i64,
    #[ts(type = "number")]
    pub expires_at_ms: i64,
    pub entries: Vec<ManifestEntry>,
}

/// Who and when, for audit records and manifests.
pub struct Context<'a> {
    pub operation_id: &'a str,
    pub user: &'a str,
    pub now_ms: i64,
}

/// Where audit records go. Returning an error from a `Started` record stops the
/// operation before anything is touched.
pub type AuditSink<'a> = dyn FnMut(NewAuditRecord) -> Result<i64, String> + 'a;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PurgeReport {
    pub operations_purged: u32,
    pub entries_purged: u32,
    pub bytes_purged: u64,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Quarantine {
    root: PathBuf,
}

fn drive_of(p: &Path) -> Option<String> {
    match p.components().next() {
        Some(Component::Prefix(pre)) => Some(
            pre.as_os_str()
                .to_string_lossy()
                .trim_start_matches(r"\\?\")
                .to_lowercase(),
        ),
        _ => None,
    }
}

fn valid_operation_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn is_reparse(md: &fs::Metadata) -> bool {
    md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

impl Quarantine {
    /// The current user's quarantine: `%LOCALAPPDATA%\Sentinel\.sentinel-quarantine`.
    pub fn for_current_user() -> Option<Self> {
        known_folder(Known::LocalAppData).map(|p| Self::at(p.join("Sentinel").join(DIR_NAME)))
    }

    /// A quarantine at an explicit location. For tests and fixtures.
    pub fn at(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Create the folder if needed and confirm that it and its parent (both folders
    /// Sentinel creates) are plain directories, not links. Returns the canonical path,
    /// which is what every later operation uses.
    ///
    /// Comparing the configured path with its canonical form would be wrong: a path
    /// spelled with 8.3 short names (e.g. `C:\Users\RUNNER~1\...`) differs from its
    /// canonical form without involving any link.
    fn ensure_root(&self) -> Result<PathBuf, QuarantineError> {
        fs::create_dir_all(&self.root).map_err(io_err(&self.root))?;
        for dir in [Some(self.root.as_path()), self.root.parent()]
            .into_iter()
            .flatten()
        {
            let md = fs::symlink_metadata(dir).map_err(io_err(dir))?;
            if !md.is_dir() || is_reparse(&md) {
                return Err(QuarantineError::UnsafeLocation(format!(
                    "{} is not a plain folder",
                    dir.display()
                )));
            }
        }
        let canon = CanonicalPath::resolve(&self.root)
            .map_err(|e| QuarantineError::UnsafeLocation(e.to_string()))?;
        Ok(canon.as_path().to_path_buf())
    }

    fn operation_dir(&self, operation_id: &str) -> Result<PathBuf, QuarantineError> {
        if !valid_operation_id(operation_id) {
            return Err(QuarantineError::NotFound(operation_id.to_owned()));
        }
        Ok(self.root.join(operation_id))
    }

    fn read_manifest(dir: &Path) -> Result<Manifest, QuarantineError> {
        let path = dir.join(MANIFEST);
        let text = fs::read_to_string(&path).map_err(io_err(&path))?;
        let m: Manifest = serde_json::from_str(&text)
            .map_err(|e| QuarantineError::BadManifest(format!("{}: {e}", path.display())))?;
        if m.version != MANIFEST_VERSION {
            return Err(QuarantineError::BadManifest(format!(
                "unsupported manifest version {}",
                m.version
            )));
        }
        Ok(m)
    }

    /// Write the manifest atomically (temporary file, then replace).
    fn write_manifest(dir: &Path, m: &Manifest) -> Result<(), QuarantineError> {
        let tmp = dir.join("manifest.json.tmp");
        let text = serde_json::to_vec_pretty(m)
            .map_err(|e| QuarantineError::BadManifest(e.to_string()))?;
        fs::write(&tmp, text).map_err(io_err(&tmp))?;
        let path = dir.join(MANIFEST);
        fs::rename(&tmp, &path).map_err(io_err(&path))
    }

    /// Move the approved items into quarantine.
    ///
    /// `approved` are paths the user approved from a preview of `provider`. Each must be
    /// a direct child of one of the provider's roots; anything else is skipped untouched.
    pub fn quarantine(
        &self,
        policy: &Policy,
        provider: &dyn CleanupProvider,
        approved: &[PathBuf],
        ctx: &Context<'_>,
        audit: &mut AuditSink<'_>,
    ) -> Result<Manifest, QuarantineError> {
        let info = provider.info();
        audit(NewAuditRecord {
            at_ms: ctx.now_ms,
            operation_id: ctx.operation_id.to_owned(),
            kind: AuditKind::Quarantine,
            provider: Some(info.id.to_owned()),
            user: ctx.user.to_owned(),
            items: approved.len() as u64,
            bytes: 0,
            policy: format!(
                "Quarantine items approved from a preview of '{}'; each is re-checked \
                 (protected locations, {}-day age) immediately before it moves",
                info.name, info.min_age_days
            ),
            approval: Approval::UserApproved,
            outcome: Outcome::Started,
            errors: vec![],
            details: serde_json::json!({ "approved": approved }),
        })
        .map_err(QuarantineError::AuditUnavailable)?;

        let result = self.move_items(policy, provider, approved, ctx);
        let (manifest, errors) = match &result {
            Ok(m) => (Some(m), vec![]),
            Err(e) => (None, vec![e.to_string()]),
        };
        let moved: Vec<&ManifestEntry> = manifest
            .map(|m| {
                m.entries
                    .iter()
                    .filter(|e| e.status == EntryStatus::Quarantined)
                    .collect()
            })
            .unwrap_or_default();
        let failed = manifest.is_some_and(|m| {
            m.entries
                .iter()
                .any(|e| matches!(e.status, EntryStatus::Failed { .. }))
        });
        let outcome = match (moved.len(), approved.len(), failed || manifest.is_none()) {
            (0, _, true) => Outcome::Failed,
            (0, _, false) => Outcome::NoChanges,
            (m, a, false) if m == a => Outcome::Succeeded,
            _ => Outcome::PartiallySucceeded,
        };
        let record = NewAuditRecord {
            at_ms: ctx.now_ms,
            operation_id: ctx.operation_id.to_owned(),
            kind: AuditKind::Quarantine,
            provider: Some(info.id.to_owned()),
            user: ctx.user.to_owned(),
            items: moved.len() as u64,
            bytes: moved.iter().map(|e| e.bytes).sum(),
            policy: "Result of the quarantine operation started above".into(),
            approval: Approval::UserApproved,
            outcome,
            errors,
            details: serde_json::json!({ "entries": manifest.map(|m| &m.entries) }),
        };
        if let Err(err) = audit(record) {
            // The moves already happened and the manifest records them; the Started
            // record shows the operation. Report loudly rather than undo.
            tracing::error!(operation = ctx.operation_id, error = %err, "could not record quarantine outcome");
        }
        result
    }

    fn move_items(
        &self,
        policy: &Policy,
        provider: &dyn CleanupProvider,
        approved: &[PathBuf],
        ctx: &Context<'_>,
    ) -> Result<Manifest, QuarantineError> {
        let info = provider.info();
        let root = self.ensure_root()?;
        let op_dir = root.join(
            self.operation_dir(ctx.operation_id)?
                .file_name()
                .unwrap_or_default(),
        );
        fs::create_dir(&op_dir).map_err(io_err(&op_dir))?;

        let mut manifest = Manifest {
            version: MANIFEST_VERSION,
            operation_id: ctx.operation_id.to_owned(),
            provider: info.id.to_owned(),
            user: ctx.user.to_owned(),
            created_at_ms: ctx.now_ms,
            expires_at_ms: ctx.now_ms + RETENTION_MS,
            entries: approved
                .iter()
                .enumerate()
                .map(|(i, p)| ManifestEntry {
                    index: u32::try_from(i).unwrap_or(u32::MAX),
                    original_path: p.display().to_string(),
                    kind: ItemKind::File,
                    bytes: 0,
                    files: 0,
                    status: EntryStatus::Pending,
                })
                .collect(),
        };
        Self::write_manifest(&op_dir, &manifest)?;

        let provider_roots: Vec<PathBuf> = provider
            .roots()
            .iter()
            .filter_map(|r| CanonicalPath::resolve(r).ok())
            .map(|c| c.as_path().to_path_buf())
            .collect();
        let volume = drive_of(&root);

        for (i, path) in approved.iter().enumerate() {
            let status = self.move_one(
                policy,
                &info,
                &provider_roots,
                volume.as_deref(),
                &op_dir,
                path,
                i,
                ctx.now_ms,
                &mut manifest.entries[i],
            );
            manifest.entries[i].status = status;
            Self::write_manifest(&op_dir, &manifest)?;
        }

        let any_moved = manifest
            .entries
            .iter()
            .any(|e| e.status == EntryStatus::Quarantined);
        if !any_moved {
            // Nothing to keep: remove the empty operation folder.
            let _ = fs::remove_file(op_dir.join(MANIFEST));
            let _ = fs::remove_dir(&op_dir);
        }
        tracing::info!(
            operation = ctx.operation_id,
            moved = manifest
                .entries
                .iter()
                .filter(|e| e.status == EntryStatus::Quarantined)
                .count(),
            approved = approved.len(),
            "quarantine finished"
        );
        Ok(manifest)
    }

    #[allow(clippy::too_many_arguments)]
    fn move_one(
        &self,
        policy: &Policy,
        info: &sentinel_cleanup::ProviderInfo,
        provider_roots: &[PathBuf],
        volume: Option<&str>,
        op_dir: &Path,
        path: &Path,
        index: usize,
        now_ms: i64,
        entry: &mut ManifestEntry,
    ) -> EntryStatus {
        let skip = |reason: &str| EntryStatus::Skipped {
            reason: reason.to_owned(),
        };
        let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
            return skip("not a valid item path");
        };
        let Some(root) = provider_roots
            .iter()
            .find(|r| is_within(parent, r) && is_within(r, parent))
        else {
            return skip("not inside this cleanup's folder");
        };
        let allowed = match policy.allowed_root(root) {
            Ok(a) => a,
            Err(e) => return skip(&e.to_string()),
        };
        let (item, target) = assess(
            policy,
            &allowed,
            &name.to_string_lossy(),
            info.min_age_days,
            now_ms,
            PreviewLimits::default(),
        );
        entry.kind = item.kind;
        entry.bytes = item.bytes;
        entry.files = item.files;
        let Some(target) = target else {
            return skip(&match item.decision {
                Decision::TooRecent { .. } => "it changed recently".to_owned(),
                Decision::Protected { reason } => format!("protected: {reason}"),
                Decision::Skipped { reason } => reason,
                Decision::Eligible => "no longer eligible".to_owned(),
            });
        };
        if drive_of(target.path()).as_deref() != volume {
            return skip("it is on a different drive from the quarantine folder");
        }
        let handle = match target.open_verified(policy) {
            Ok(h) => h,
            Err(SafetyError::Io { source, .. }) if is_in_use(&source) => {
                return skip("it is in use by another program");
            }
            Err(SafetyError::PermissionDenied(_)) => {
                return skip("it is in use or not accessible");
            }
            Err(e) => return skip(&e.to_string()),
        };
        let dest = op_dir.join(index.to_string());
        match rename_by_handle(&handle, &dest) {
            Ok(()) => EntryStatus::Quarantined,
            Err(e) if is_in_use(&e) => skip("it is in use by another program"),
            Err(e) => EntryStatus::Failed {
                reason: e.to_string(),
            },
        }
    }

    /// All operations currently in quarantine, newest first. Unreadable manifests are
    /// reported as errors in the second list rather than hidden.
    pub fn operations(&self) -> Result<(Vec<Manifest>, Vec<String>), QuarantineError> {
        let mut ops = Vec::new();
        let mut problems = Vec::new();
        let entries = match fs::read_dir(&self.root) {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok((ops, problems)),
            Err(e) => return Err(io_err(&self.root)(e)),
        };
        for e in entries.flatten() {
            let Ok(md) = fs::symlink_metadata(e.path()) else {
                continue;
            };
            if !md.is_dir() || is_reparse(&md) {
                continue;
            }
            match Self::read_manifest(&e.path()) {
                Ok(m) => ops.push(m),
                Err(err) => problems.push(err.to_string()),
            }
        }
        ops.sort_by_key(|m| std::cmp::Reverse(m.created_at_ms));
        Ok((ops, problems))
    }

    /// Put one quarantined item back where it came from. Refuses if anything exists at
    /// the original location or the location is protected.
    pub fn restore(
        &self,
        policy: &Policy,
        operation_id: &str,
        index: u32,
        ctx: &Context<'_>,
        audit: &mut AuditSink<'_>,
    ) -> Result<ManifestEntry, QuarantineError> {
        let op_dir = self.operation_dir(operation_id)?;
        let mut manifest = Self::read_manifest(&op_dir)?;
        let pos = manifest
            .entries
            .iter()
            .position(|e| e.index == index)
            .ok_or_else(|| QuarantineError::NotFound(format!("{operation_id} item {index}")))?;
        let entry = manifest.entries[pos].clone();
        if entry.status != EntryStatus::Quarantined {
            return Err(QuarantineError::Refused(
                "that item is not in quarantine".into(),
            ));
        }
        let src = op_dir.join(index.to_string());
        let dest = PathBuf::from(&entry.original_path);
        if fs::symlink_metadata(&dest).is_ok() {
            return Err(QuarantineError::Refused(format!(
                "something already exists at {}; it will not be overwritten",
                dest.display()
            )));
        }
        let parent = dest
            .parent()
            .ok_or_else(|| QuarantineError::Refused("invalid original location".into()))?;
        policy
            .allowed_root(parent)
            .map_err(|e| QuarantineError::Refused(e.to_string()))?;
        policy
            .check_protected(&dest)
            .map_err(|e| QuarantineError::Refused(e.to_string()))?;

        audit(NewAuditRecord {
            at_ms: ctx.now_ms,
            operation_id: ctx.operation_id.to_owned(),
            kind: AuditKind::Restore,
            provider: Some(manifest.provider.clone()),
            user: ctx.user.to_owned(),
            items: 1,
            bytes: entry.bytes,
            policy: format!(
                "Restore item {index} of quarantine operation {operation_id} to its original \
                 location; never overwrites"
            ),
            approval: Approval::UserApproved,
            outcome: Outcome::Started,
            errors: vec![],
            details: serde_json::json!({ "originalPath": entry.original_path }),
        })
        .map_err(QuarantineError::AuditUnavailable)?;

        let result = open_quarantined(&src).and_then(|h| {
            rename_by_handle(&h, &dest).map_err(|e| QuarantineError::Io {
                path: dest.clone(),
                source: e,
            })
        });
        let outcome = if result.is_ok() {
            manifest.entries[pos].status = EntryStatus::Restored;
            Self::write_manifest(&op_dir, &manifest)?;
            Outcome::Succeeded
        } else {
            Outcome::Failed
        };
        let record = NewAuditRecord {
            at_ms: ctx.now_ms,
            operation_id: ctx.operation_id.to_owned(),
            kind: AuditKind::Restore,
            provider: Some(manifest.provider.clone()),
            user: ctx.user.to_owned(),
            items: u64::from(result.is_ok()),
            bytes: if result.is_ok() { entry.bytes } else { 0 },
            policy: "Result of the restore started above".into(),
            approval: Approval::UserApproved,
            outcome,
            errors: result
                .as_ref()
                .err()
                .map(|e| e.to_string())
                .into_iter()
                .collect(),
            details: serde_json::json!({ "originalPath": entry.original_path }),
        };
        if let Err(err) = audit(record) {
            tracing::error!(operation = ctx.operation_id, error = %err, "could not record restore outcome");
        }
        result.map(|()| manifest.entries[pos].clone())
    }

    /// Permanently remove operations whose retention period has ended.
    pub fn purge_expired(
        &self,
        ctx: &Context<'_>,
        audit: &mut AuditSink<'_>,
    ) -> Result<PurgeReport, QuarantineError> {
        let mut report = PurgeReport::default();
        let (ops, problems) = self.operations()?;
        report.errors.extend(problems);
        for mut m in ops.into_iter().filter(|m| m.expires_at_ms <= ctx.now_ms) {
            let op_dir = self.operation_dir(&m.operation_id)?;
            let doomed: Vec<usize> = m
                .entries
                .iter()
                .enumerate()
                .filter(|(_, e)| e.status == EntryStatus::Quarantined)
                .map(|(i, _)| i)
                .collect();
            let bytes: u64 = doomed.iter().map(|&i| m.entries[i].bytes).sum();
            audit(NewAuditRecord {
                at_ms: ctx.now_ms,
                operation_id: ctx.operation_id.to_owned(),
                kind: AuditKind::Purge,
                provider: Some(m.provider.clone()),
                user: ctx.user.to_owned(),
                items: doomed.len() as u64,
                bytes,
                policy: format!(
                    "Permanently remove items of quarantine operation {} after the 14-day \
                     retention period",
                    m.operation_id
                ),
                approval: Approval::Automatic,
                outcome: Outcome::Started,
                errors: vec![],
                details: serde_json::json!({ "quarantineOperation": m.operation_id }),
            })
            .map_err(QuarantineError::AuditUnavailable)?;

            let mut errors = Vec::new();
            for &i in &doomed {
                let p = op_dir.join(m.entries[i].index.to_string());
                match remove_without_following(&p) {
                    Ok(()) => m.entries[i].status = EntryStatus::Purged,
                    Err(e) => {
                        errors.push(format!("{}: {e}", p.display()));
                        m.entries[i].status = EntryStatus::Failed {
                            reason: e.to_string(),
                        };
                    }
                }
            }
            if errors.is_empty() {
                let _ = fs::remove_file(op_dir.join(MANIFEST));
                if let Err(e) = fs::remove_dir(&op_dir) {
                    errors.push(format!("{}: {e}", op_dir.display()));
                }
            } else {
                Self::write_manifest(&op_dir, &m)?;
            }
            let purged = m
                .entries
                .iter()
                .filter(|e| e.status == EntryStatus::Purged)
                .count();
            report.operations_purged += u32::from(errors.is_empty());
            report.entries_purged += u32::try_from(purged).unwrap_or(u32::MAX);
            report.bytes_purged += if errors.is_empty() { bytes } else { 0 };
            let record = NewAuditRecord {
                at_ms: ctx.now_ms,
                operation_id: ctx.operation_id.to_owned(),
                kind: AuditKind::Purge,
                provider: Some(m.provider.clone()),
                user: ctx.user.to_owned(),
                items: purged as u64,
                bytes: if errors.is_empty() { bytes } else { 0 },
                policy: "Result of the purge started above".into(),
                approval: Approval::Automatic,
                outcome: if errors.is_empty() {
                    Outcome::Succeeded
                } else if purged > 0 {
                    Outcome::PartiallySucceeded
                } else {
                    Outcome::Failed
                },
                errors: errors.clone(),
                details: serde_json::json!({ "quarantineOperation": m.operation_id }),
            };
            if let Err(err) = audit(record) {
                tracing::error!(error = %err, "could not record purge outcome");
            }
            report.errors.extend(errors);
        }
        Ok(report)
    }
}

/// Open a quarantined item for renaming (the object itself, never a link target).
fn open_quarantined(path: &Path) -> Result<fs::File, QuarantineError> {
    use std::os::windows::fs::OpenOptionsExt;
    const DELETE: u32 = 0x0001_0000;
    const FILE_READ_ATTRIBUTES: u32 = 0x0080;
    const SYNCHRONIZE: u32 = 0x0010_0000;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    fs::OpenOptions::new()
        .access_mode(DELETE | FILE_READ_ATTRIBUTES | SYNCHRONIZE)
        .share_mode(0x7)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map_err(io_err(path))
}

/// Delete a file, a link (the link only) or a folder tree without following links.
/// `std::fs::remove_dir_all` does not traverse junctions or symlinks on Windows.
fn remove_without_following(path: &Path) -> io::Result<()> {
    let md = fs::symlink_metadata(path)?;
    if md.is_dir() {
        if is_reparse(&md) {
            fs::remove_dir(path)
        } else {
            fs::remove_dir_all(path)
        }
    } else {
        fs::remove_file(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_ids_cannot_escape_the_folder() {
        for bad in ["", "..", r"..\x", "a/b", r"C:\x", "op id"] {
            assert!(!valid_operation_id(bad), "{bad:?}");
        }
        assert!(valid_operation_id(
            "op-2ba4f228-e484-4fbc-8ce9-01a1d516b729"
        ));
    }

    #[test]
    fn drive_comparison_ignores_case_and_verbatim_prefix() {
        assert_eq!(
            drive_of(Path::new(r"\\?\C:\x")),
            drive_of(Path::new(r"c:\y"))
        );
        assert_ne!(drive_of(Path::new(r"C:\x")), drive_of(Path::new(r"D:\x")));
    }

    #[test]
    fn current_user_location_is_private_and_protected_by_name() {
        let q = Quarantine::for_current_user().unwrap_or_else(|| Quarantine::at(PathBuf::new()));
        assert!(q.root().ends_with(r"Sentinel\.sentinel-quarantine"));
        assert!(
            Policy::new(sentinel_safety::ProtectedSet::new())
                .check_protected(&q.root().join("op-1").join("0"))
                .is_err()
        );
    }
}
