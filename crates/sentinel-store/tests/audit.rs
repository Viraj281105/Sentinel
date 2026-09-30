#![allow(clippy::unwrap_used, clippy::expect_used)]

use sentinel_store::{Approval, AuditKind, GENESIS_HASH, NewAuditRecord, Outcome, Store};

fn rec(op: &str, items: u64) -> NewAuditRecord {
    NewAuditRecord {
        at_ms: 1_000 + items as i64,
        operation_id: op.into(),
        kind: AuditKind::Preview,
        provider: Some("user-temp".into()),
        user: "tester".into(),
        items,
        bytes: items * 100,
        policy: "dry run".into(),
        approval: Approval::NotRequired,
        outcome: Outcome::NoChanges,
        errors: vec![],
        details: serde_json::json!({ "eligibleItems": items }),
    }
}

fn file_store() -> (tempfile::TempDir, std::path::PathBuf, Store) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sentinel.db");
    let s = Store::open(&path).unwrap();
    (dir, path, s)
}

#[test]
fn appends_a_verifiable_chain() {
    let mut s = Store::open_in_memory().unwrap();
    assert_eq!(s.verify_audit().unwrap().records, 0);
    for i in 1..=3 {
        assert_eq!(
            s.append_audit(&rec(&format!("op-{i}"), i)).unwrap(),
            i as i64
        );
    }
    let v = s.verify_audit().unwrap();
    assert_eq!((v.records, v.first_bad_seq), (3, None));

    let recs = s.audit_records(10, None).unwrap();
    assert_eq!(recs.iter().map(|r| r.seq).collect::<Vec<_>>(), [3, 2, 1]);
    assert_eq!(recs[2].prev_hash, GENESIS_HASH);
    assert_eq!(recs[1].prev_hash, recs[2].hash);
    assert_eq!(recs[0].kind, AuditKind::Preview);
    assert_eq!(recs[0].details["eligibleItems"], 3);
    assert_eq!(s.audit_records(10, Some(2)).unwrap().len(), 1, "paging");
}

#[test]
fn database_refuses_updates_and_deletes() {
    let (_d, path, mut s) = file_store();
    s.append_audit(&rec("op-1", 1)).unwrap();
    let conn = rusqlite::Connection::open(&path).unwrap();
    let upd = conn.execute("UPDATE audit_log SET items = 99", []);
    assert!(upd.unwrap_err().to_string().contains("append-only"));
    let del = conn.execute("DELETE FROM audit_log", []);
    assert!(del.unwrap_err().to_string().contains("append-only"));
    assert!(s.verify_audit().unwrap().first_bad_seq.is_none());
}

#[test]
fn detects_an_edited_record() {
    let (_d, path, mut s) = file_store();
    for i in 1..=3 {
        s.append_audit(&rec(&format!("op-{i}"), i)).unwrap();
    }
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(
        "DROP TRIGGER audit_log_no_update; UPDATE audit_log SET bytes = 1 WHERE seq = 2;",
    )
    .unwrap();
    let v = s.verify_audit().unwrap();
    assert_eq!(v.first_bad_seq, Some(2));
    assert!(v.problem.unwrap().contains("changed"));
}

#[test]
fn detects_a_removed_record() {
    let (_d, path, mut s) = file_store();
    for i in 1..=3 {
        s.append_audit(&rec(&format!("op-{i}"), i)).unwrap();
    }
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch("DROP TRIGGER audit_log_no_delete; DELETE FROM audit_log WHERE seq = 2;")
        .unwrap();
    let v = s.verify_audit().unwrap();
    assert_eq!(v.first_bad_seq, Some(3));
    assert!(v.problem.unwrap().contains("missing"));
}

#[test]
fn detects_a_rehashed_record_that_breaks_the_link() {
    let (_d, path, mut s) = file_store();
    for i in 1..=2 {
        s.append_audit(&rec(&format!("op-{i}"), i)).unwrap();
    }
    // Replacing a record's own hash (to hide an edit) breaks the next record's link.
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(
        "DROP TRIGGER audit_log_no_update;
         UPDATE audit_log SET hash = 'ffff' WHERE seq = 1;",
    )
    .unwrap();
    let v = s.verify_audit().unwrap();
    assert_eq!(v.first_bad_seq, Some(1), "own hash no longer matches");
}

#[test]
fn chain_survives_reopen() {
    let (_d, path, mut s) = file_store();
    s.append_audit(&rec("op-1", 1)).unwrap();
    drop(s);
    let mut s = Store::open(&path).unwrap();
    s.append_audit(&rec("op-2", 2)).unwrap();
    let v = s.verify_audit().unwrap();
    assert_eq!((v.records, v.first_bad_seq), (2, None));
}
