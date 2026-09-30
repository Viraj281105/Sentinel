//! Audit log (Activity) commands. Read-only.

use sentinel_store::{Approval, AuditKind, AuditRecord, Outcome};
use serde::Serialize;
use tauri::State;
use ts_rs::TS;

use super::error::CommandError;
use crate::AppState;

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AuditEntry {
    #[ts(type = "number")]
    pub seq: i64,
    #[ts(type = "number")]
    pub at_ms: i64,
    pub operation_id: String,
    pub kind: AuditKind,
    pub provider: Option<String>,
    pub user: String,
    #[ts(type = "number")]
    pub items: u64,
    #[ts(type = "number")]
    pub bytes: u64,
    pub policy: String,
    pub approval: Approval,
    pub outcome: Outcome,
    pub errors: Vec<String>,
    #[ts(type = "Record<string, unknown>")]
    pub details: serde_json::Value,
    pub hash: String,
}

impl From<AuditRecord> for AuditEntry {
    fn from(r: AuditRecord) -> Self {
        Self {
            seq: r.seq,
            at_ms: r.at_ms,
            operation_id: r.operation_id,
            kind: r.kind,
            provider: r.provider,
            user: r.user,
            items: r.items,
            bytes: r.bytes,
            policy: r.policy,
            approval: r.approval,
            outcome: r.outcome,
            errors: r.errors,
            details: r.details,
            hash: r.hash,
        }
    }
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AuditIntegrity {
    #[ts(type = "number")]
    pub records: u64,
    pub intact: bool,
    #[ts(type = "number | null")]
    pub first_bad_seq: Option<i64>,
    pub problem: Option<String>,
}

/// Newest entries first. `before` is a sequence number for paging.
#[tauri::command]
pub(crate) fn audit_log(
    state: State<'_, AppState>,
    limit: u32,
    before: Option<i64>,
) -> Result<Vec<AuditEntry>, CommandError> {
    let recs = state.db.lock().audit_records(limit.min(500), before)?;
    Ok(recs.into_iter().map(AuditEntry::from).collect())
}

/// Recompute the audit log's hash chain.
#[tauri::command]
pub(crate) fn audit_verify(state: State<'_, AppState>) -> Result<AuditIntegrity, CommandError> {
    let v = state.db.lock().verify_audit()?;
    if let Some(seq) = v.first_bad_seq {
        tracing::error!(seq, problem = ?v.problem, "audit log integrity check failed");
    }
    Ok(AuditIntegrity {
        records: v.records,
        intact: v.first_bad_seq.is_none(),
        first_bad_seq: v.first_bad_seq,
        problem: v.problem,
    })
}
