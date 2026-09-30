//! Writing audit records for Sentinel's operations.
//!
//! Rule for anything that changes files (the future executor): if the audit record
//! cannot be written, the operation must not run. Previews change nothing, so a failed
//! write is logged and the preview is still shown.

use sentinel_cleanup::{Decision, Preview, RootReport};
use sentinel_store::{Approval, AuditKind, NewAuditRecord, Outcome, StoreError};
use windows::Win32::System::WindowsProgramming::GetUserNameW;
use windows::core::PWSTR;

use crate::db::Db;

/// The Windows account running Sentinel, from `GetUserNameW` (not `%USERNAME%`, which
/// any process can change).
pub(crate) fn current_user() -> String {
    let mut buf = [0u16; 257];
    let mut len = buf.len() as u32;
    // SAFETY: `buf` is valid for `len` UTF-16 units; the API writes at most that many.
    match unsafe { GetUserNameW(Some(PWSTR(buf.as_mut_ptr())), &mut len) } {
        Ok(()) => {
            let n = (len as usize).saturating_sub(1).min(buf.len());
            String::from_utf16_lossy(&buf[..n])
        }
        Err(err) => {
            tracing::warn!(error = %err, "could not read the Windows user name");
            "unknown".to_owned()
        }
    }
}

pub(crate) fn new_operation_id() -> String {
    format!("op-{}", uuid::Uuid::new_v4())
}

/// Build the audit record for a dry-run preview.
pub(crate) fn preview_record(
    p: &Preview,
    operation_id: &str,
    user: &str,
    at_ms: i64,
) -> NewAuditRecord {
    let kept = p.items.len() as u64 - p.eligible_items;
    let errors = p
        .roots
        .iter()
        .filter_map(|r| match r {
            RootReport::Unavailable { path, reason } => Some(format!("{path}: {reason}")),
            _ => None,
        })
        .collect();
    let count = |f: fn(&Decision) -> bool| p.items.iter().filter(|i| f(&i.decision)).count();
    NewAuditRecord {
        at_ms,
        operation_id: operation_id.to_owned(),
        kind: AuditKind::Preview,
        provider: Some(p.provider.id.to_owned()),
        user: user.to_owned(),
        items: p.eligible_items,
        bytes: p.eligible_bytes,
        policy: format!(
            "Dry run of '{}': only items unchanged for {} days; every item validated against \
             protected locations",
            p.provider.name, p.provider.min_age_days
        ),
        approval: Approval::NotRequired,
        outcome: Outcome::NoChanges,
        errors,
        details: serde_json::json!({
            "eligibleFiles": p.eligible_files,
            "keptItems": kept,
            "tooRecent": count(|d| matches!(d, Decision::TooRecent { .. })),
            "protected": count(|d| matches!(d, Decision::Protected { .. })),
            "skipped": count(|d| matches!(d, Decision::Skipped { .. })),
            "incomplete": p.incomplete,
        }),
    }
}

pub(crate) fn record(db: &Db, rec: &NewAuditRecord) -> Result<i64, StoreError> {
    db.lock().append_audit(rec)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::sync::atomic::AtomicBool;

    use sentinel_cleanup::providers::UserTemp;
    use sentinel_cleanup::{PreviewLimits, preview};
    use sentinel_safety::{Policy, ProtectedSet};

    use super::*;

    #[test]
    fn reads_a_real_user_name() {
        let u = current_user();
        assert!(!u.is_empty() && u != "unknown");
    }

    #[test]
    fn operation_ids_are_unique() {
        assert_ne!(new_operation_id(), new_operation_id());
    }

    #[test]
    fn records_a_preview_as_a_no_change_dry_run() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("fresh.tmp"), b"x").unwrap();
        let p = preview(
            &UserTemp::with_root(dir.path().to_path_buf()),
            &Policy::new(ProtectedSet::new()),
            crate::db::now_ms(),
            PreviewLimits::default(),
            &AtomicBool::new(false),
        );
        let db = Db::in_memory();
        let rec = preview_record(&p, "op-test", "tester", 5);
        let seq = record(&db, &rec).unwrap();
        let stored = &db.lock().audit_records(1, None).unwrap()[0];
        assert_eq!(stored.seq, seq);
        assert_eq!(stored.kind, AuditKind::Preview);
        assert_eq!(stored.outcome, Outcome::NoChanges);
        assert_eq!(stored.approval, Approval::NotRequired);
        assert_eq!(stored.provider.as_deref(), Some("user-temp"));
        assert_eq!(stored.details["tooRecent"], 1);
        assert!(db.lock().verify_audit().unwrap().first_bad_seq.is_none());
    }
}
