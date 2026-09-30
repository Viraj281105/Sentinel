//! Append-only, hash-chained audit log.
//!
//! Every record stores `hash = SHA-256(canonical JSON of the record including
//! prev_hash)`, where `prev_hash` is the previous record's hash (64 zeros for the first).
//! SQLite triggers reject `UPDATE` and `DELETE` on the table, so ordinary code cannot
//! rewrite history, and [`Store::verify_audit`] detects edited, reordered or removed
//! records.
//!
//! Limits, stated plainly: someone with write access to the database file can drop the
//! triggers and rebuild a consistent chain, and removing the newest records leaves a
//! valid shorter chain. The log is tamper-evident against accidents and casual edits,
//! not against a malicious local administrator (out of scope in the threat model).

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ts_rs::TS;

use crate::{Result, Store, StoreError, to_i, to_u};

pub const GENESIS_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub enum AuditKind {
    /// A dry run: nothing was changed.
    Preview,
    /// Items moved into quarantine.
    Quarantine,
    /// Items restored from quarantine.
    Restore,
    /// Expired quarantine items removed.
    Purge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub enum Approval {
    /// The operation changes nothing, so no approval applies.
    NotRequired,
    /// The user explicitly confirmed this operation.
    UserApproved,
    /// Ran under a rule the user enabled in advance.
    Automatic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub enum Outcome {
    NoChanges,
    Succeeded,
    PartiallySucceeded,
    Failed,
}

fn key<T: Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn from_key<T: for<'de> Deserialize<'de>>(s: &str) -> Result<T> {
    serde_json::from_value(serde_json::Value::String(s.to_owned()))
        .map_err(|e| StoreError::Corrupt(format!("audit value {s:?}: {e}")))
}

/// A record to append. `details` is free-form JSON describing the operation.
#[derive(Debug, Clone)]
pub struct NewAuditRecord {
    pub at_ms: i64,
    pub operation_id: String,
    pub kind: AuditKind,
    pub provider: Option<String>,
    pub user: String,
    pub items: u64,
    pub bytes: u64,
    /// The policy that decided the operation, in words.
    pub policy: String,
    pub approval: Approval,
    pub outcome: Outcome,
    pub errors: Vec<String>,
    pub details: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AuditRecord {
    pub seq: i64,
    pub at_ms: i64,
    pub operation_id: String,
    pub kind: AuditKind,
    pub provider: Option<String>,
    pub user: String,
    pub items: u64,
    pub bytes: u64,
    pub policy: String,
    pub approval: Approval,
    pub outcome: Outcome,
    pub errors: Vec<String>,
    pub details: serde_json::Value,
    pub prev_hash: String,
    pub hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditVerification {
    pub records: u64,
    /// `None` when the whole chain is intact.
    pub first_bad_seq: Option<i64>,
    pub problem: Option<String>,
}

/// The exact bytes that are hashed. Field order is fixed by this struct; `errors` and
/// `details` are hashed as the JSON text stored in the database.
#[derive(Serialize)]
struct Canonical<'a> {
    seq: i64,
    at_ms: i64,
    operation_id: &'a str,
    kind: &'a str,
    provider: Option<&'a str>,
    user: &'a str,
    items: i64,
    bytes: i64,
    policy: &'a str,
    approval: &'a str,
    outcome: &'a str,
    errors: &'a str,
    details: &'a str,
    prev_hash: &'a str,
}

fn digest(c: &Canonical<'_>) -> String {
    let json = serde_json::to_vec(c).unwrap_or_default();
    Sha256::digest(&json)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Raw row as stored; used for both reading and verification.
struct Row {
    seq: i64,
    at_ms: i64,
    operation_id: String,
    kind: String,
    provider: Option<String>,
    user: String,
    items: i64,
    bytes: i64,
    policy: String,
    approval: String,
    outcome: String,
    errors: String,
    details: String,
    prev_hash: String,
    hash: String,
}

impl Row {
    fn canonical(&self) -> Canonical<'_> {
        Canonical {
            seq: self.seq,
            at_ms: self.at_ms,
            operation_id: &self.operation_id,
            kind: &self.kind,
            provider: self.provider.as_deref(),
            user: &self.user,
            items: self.items,
            bytes: self.bytes,
            policy: &self.policy,
            approval: &self.approval,
            outcome: &self.outcome,
            errors: &self.errors,
            details: &self.details,
            prev_hash: &self.prev_hash,
        }
    }

    fn into_record(self) -> Result<AuditRecord> {
        Ok(AuditRecord {
            seq: self.seq,
            at_ms: self.at_ms,
            kind: from_key(&self.kind)?,
            approval: from_key(&self.approval)?,
            outcome: from_key(&self.outcome)?,
            errors: serde_json::from_str(&self.errors)
                .map_err(|e| StoreError::Corrupt(format!("audit errors: {e}")))?,
            details: serde_json::from_str(&self.details)
                .map_err(|e| StoreError::Corrupt(format!("audit details: {e}")))?,
            operation_id: self.operation_id,
            provider: self.provider,
            user: self.user,
            items: to_u(self.items),
            bytes: to_u(self.bytes),
            policy: self.policy,
            prev_hash: self.prev_hash,
            hash: self.hash,
        })
    }
}

const COLUMNS: &str = "seq, at_ms, operation_id, kind, provider, user, items, bytes, policy, \
                       approval, outcome, errors, details, prev_hash, hash";

fn read_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Row> {
    Ok(Row {
        seq: r.get(0)?,
        at_ms: r.get(1)?,
        operation_id: r.get(2)?,
        kind: r.get(3)?,
        provider: r.get(4)?,
        user: r.get(5)?,
        items: r.get(6)?,
        bytes: r.get(7)?,
        policy: r.get(8)?,
        approval: r.get(9)?,
        outcome: r.get(10)?,
        errors: r.get(11)?,
        details: r.get(12)?,
        prev_hash: r.get(13)?,
        hash: r.get(14)?,
    })
}

impl Store {
    /// Append a record to the audit log and return its sequence number.
    pub fn append_audit(&mut self, rec: &NewAuditRecord) -> Result<i64> {
        let tx = self.conn.transaction()?;
        let last: Option<(i64, String)> = tx
            .query_row(
                "SELECT seq, hash FROM audit_log ORDER BY seq DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let (prev_seq, prev_hash) = last.unwrap_or((0, GENESIS_HASH.to_owned()));
        let mut row = Row {
            seq: prev_seq + 1,
            at_ms: rec.at_ms,
            operation_id: rec.operation_id.clone(),
            kind: key(&rec.kind),
            provider: rec.provider.clone(),
            user: rec.user.clone(),
            items: to_i(rec.items),
            bytes: to_i(rec.bytes),
            policy: rec.policy.clone(),
            approval: key(&rec.approval),
            outcome: key(&rec.outcome),
            errors: serde_json::to_string(&rec.errors)
                .map_err(|e| StoreError::Corrupt(e.to_string()))?,
            details: serde_json::to_string(&rec.details)
                .map_err(|e| StoreError::Corrupt(e.to_string()))?,
            prev_hash,
            hash: String::new(),
        };
        row.hash = digest(&row.canonical());
        tx.execute(
            &format!(
                "INSERT INTO audit_log ({COLUMNS}) VALUES \
                 (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)"
            ),
            params![
                row.seq,
                row.at_ms,
                row.operation_id,
                row.kind,
                row.provider,
                row.user,
                row.items,
                row.bytes,
                row.policy,
                row.approval,
                row.outcome,
                row.errors,
                row.details,
                row.prev_hash,
                row.hash
            ],
        )?;
        tx.commit()?;
        tracing::info!(seq = row.seq, operation = %row.operation_id, kind = %row.kind, "audit record appended");
        Ok(row.seq)
    }

    /// Newest records first; `before_seq` pages backwards.
    pub fn audit_records(&self, limit: u32, before_seq: Option<i64>) -> Result<Vec<AuditRecord>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM audit_log WHERE seq < ?1 ORDER BY seq DESC LIMIT ?2"
        ))?;
        let rows = stmt.query_map(params![before_seq.unwrap_or(i64::MAX), limit], read_row)?;
        rows.map(|r| r?.into_record()).collect()
    }

    /// Recompute the whole chain.
    pub fn verify_audit(&self) -> Result<AuditVerification> {
        let mut stmt = self
            .conn
            .prepare(&format!("SELECT {COLUMNS} FROM audit_log ORDER BY seq"))?;
        let rows = stmt.query_map([], read_row)?;
        let mut expected_prev = GENESIS_HASH.to_owned();
        let mut count = 0;
        let bad = |seq: i64, problem: String, records: u64| AuditVerification {
            records,
            first_bad_seq: Some(seq),
            problem: Some(problem),
        };
        for (expected_seq, row) in (1_i64..).zip(rows) {
            let row = row?;
            count += 1;
            if row.seq != expected_seq {
                return Ok(bad(
                    row.seq,
                    format!("record {expected_seq} is missing"),
                    count,
                ));
            }
            if row.prev_hash != expected_prev {
                return Ok(bad(
                    row.seq,
                    "does not follow the previous record".into(),
                    count,
                ));
            }
            if digest(&row.canonical()) != row.hash {
                return Ok(bad(
                    row.seq,
                    "contents were changed after it was written".into(),
                    count,
                ));
            }
            expected_prev = row.hash;
        }
        Ok(AuditVerification {
            records: count,
            first_bad_seq: None,
            problem: None,
        })
    }
}
