//! Local SQLite persistence for Sentinel.
//!
//! Stores scan summaries, a pruned copy of each scan's folder tree, the largest files,
//! and drive capacity snapshots. Only metadata is stored: folder names, sizes and
//! counts. File contents are never read, so they can never be stored.
//!
//! Schema changes are append-only migrations tracked with `PRAGMA user_version`.

mod audit;

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use sentinel_scanner::scan::{LargeFile, NodeId, NodeStatus, ScanStats, ScanTree};

pub use audit::{
    Approval, AuditKind, AuditRecord, AuditVerification, GENESIS_HASH, NewAuditRecord, Outcome,
};

pub type ScanId = i64;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("database was created by a newer Sentinel (schema {found}, this build knows {known})")]
    FutureSchema { found: i64, known: i64 },
    #[error("database contains unreadable data: {0}")]
    Corrupt(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// Ordered, append-only schema migrations. Never edit a shipped entry.
const MIGRATIONS: &[&str] = &[
    r"
CREATE TABLE scans (
    id                  INTEGER PRIMARY KEY,
    root                TEXT    NOT NULL COLLATE NOCASE,
    started_at_ms       INTEGER NOT NULL,
    finished_at_ms      INTEGER NOT NULL,
    total_bytes         INTEGER NOT NULL,
    total_logical_bytes INTEGER NOT NULL,
    files               INTEGER NOT NULL,
    dirs                INTEGER NOT NULL,
    access_denied       INTEGER NOT NULL,
    errors              INTEGER NOT NULL,
    links_skipped       INTEGER NOT NULL,
    excluded            INTEGER NOT NULL,
    cancelled           INTEGER NOT NULL,
    truncated           INTEGER NOT NULL,
    elapsed_ms          INTEGER NOT NULL,
    min_node_bytes      INTEGER NOT NULL
);
CREATE INDEX scans_root ON scans(root, finished_at_ms);

CREATE TABLE scan_nodes (
    scan_id         INTEGER NOT NULL REFERENCES scans(id) ON DELETE CASCADE,
    node            INTEGER NOT NULL,
    parent          INTEGER,
    name            TEXT    NOT NULL COLLATE NOCASE,
    total_bytes     INTEGER NOT NULL,
    own_bytes       INTEGER NOT NULL,
    file_count      INTEGER NOT NULL,
    own_files       INTEGER NOT NULL,
    child_count     INTEGER NOT NULL,
    pruned_children INTEGER NOT NULL,
    pruned_bytes    INTEGER NOT NULL,
    status          TEXT    NOT NULL,
    PRIMARY KEY (scan_id, node)
) WITHOUT ROWID;
CREATE INDEX scan_nodes_children ON scan_nodes(scan_id, parent, name);

CREATE TABLE scan_largest_files (
    scan_id       INTEGER NOT NULL REFERENCES scans(id) ON DELETE CASCADE,
    rank          INTEGER NOT NULL,
    path          TEXT    NOT NULL,
    bytes         INTEGER NOT NULL,
    logical_bytes INTEGER NOT NULL,
    PRIMARY KEY (scan_id, rank)
) WITHOUT ROWID;

CREATE TABLE drive_snapshots (
    root        TEXT    NOT NULL COLLATE NOCASE,
    taken_at_ms INTEGER NOT NULL,
    total_bytes INTEGER NOT NULL,
    free_bytes  INTEGER NOT NULL,
    PRIMARY KEY (root, taken_at_ms)
) WITHOUT ROWID;
",
    r"
CREATE TABLE scan_categories (
    scan_id  INTEGER NOT NULL REFERENCES scans(id) ON DELETE CASCADE,
    category TEXT    NOT NULL,
    bytes    INTEGER NOT NULL,
    PRIMARY KEY (scan_id, category)
) WITHOUT ROWID;
",
    r"
CREATE TABLE audit_log (
    seq          INTEGER PRIMARY KEY,
    at_ms        INTEGER NOT NULL,
    operation_id TEXT    NOT NULL,
    kind         TEXT    NOT NULL,
    provider     TEXT,
    user         TEXT    NOT NULL,
    items        INTEGER NOT NULL,
    bytes        INTEGER NOT NULL,
    policy       TEXT    NOT NULL,
    approval     TEXT    NOT NULL,
    outcome      TEXT    NOT NULL,
    errors       TEXT    NOT NULL,
    details      TEXT    NOT NULL,
    prev_hash    TEXT    NOT NULL,
    hash         TEXT    NOT NULL
);
CREATE INDEX audit_log_operation ON audit_log(operation_id);
CREATE TRIGGER audit_log_no_update BEFORE UPDATE ON audit_log
BEGIN SELECT RAISE(ABORT, 'the audit log is append-only'); END;
CREATE TRIGGER audit_log_no_delete BEFORE DELETE ON audit_log
BEGIN SELECT RAISE(ABORT, 'the audit log is append-only'); END;
",
    r"
CREATE TABLE project_searches (
    root           TEXT    PRIMARY KEY COLLATE NOCASE,
    searched_at_ms INTEGER NOT NULL,
    result         TEXT    NOT NULL
) WITHOUT ROWID;
",
];

/// How much of each scan to keep.
#[derive(Debug, Clone, Copy)]
pub struct Retention {
    /// Folders smaller than this are not stored individually; each parent records how
    /// many were dropped and their combined size.
    pub min_node_bytes: u64,
    /// Older scans of the same root beyond this count are deleted.
    pub keep_scans_per_root: u32,
}

impl Default for Retention {
    fn default() -> Self {
        Self {
            min_node_bytes: 1024 * 1024,
            keep_scans_per_root: 10,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanRecord {
    pub id: ScanId,
    pub root: String,
    pub started_at_ms: i64,
    pub finished_at_ms: i64,
    pub stats: ScanStats,
    pub min_node_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredNode {
    pub id: NodeId,
    pub parent: Option<NodeId>,
    pub name: String,
    pub total_bytes: u64,
    /// Bytes of files directly in this folder.
    pub own_bytes: u64,
    /// Files in the whole subtree.
    pub file_count: u64,
    /// Files directly in this folder.
    pub own_files: u64,
    /// Subfolders at scan time, including pruned ones.
    pub child_count: u64,
    pub pruned_children: u64,
    pub pruned_bytes: u64,
    pub status: NodeStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DriveSnapshot {
    pub taken_at_ms: i64,
    pub total_bytes: u64,
    pub free_bytes: u64,
}

fn to_i(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

fn to_u(v: i64) -> u64 {
    u64::try_from(v).unwrap_or(0)
}

pub struct Store {
    conn: Connection,
}

impl Store {
    /// Open (creating if needed) the database at `path` and apply migrations.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        Self::init(Connection::open(path)?)
    }

    /// A private, non-persistent database (used when the file cannot be opened).
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let mut store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&mut self) -> Result<()> {
        let known = MIGRATIONS.len() as i64;
        let found: i64 = self
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))?;
        if found > known {
            return Err(StoreError::FutureSchema { found, known });
        }
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(found as usize) {
            let tx = self.conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", i as i64 + 1)?;
            tx.commit()?;
            tracing::info!(version = i + 1, "database migrated");
        }
        Ok(())
    }

    pub fn schema_version(&self) -> Result<i64> {
        Ok(self
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))?)
    }

    /// Persist a finished scan with its per-category byte totals (category keys are
    /// opaque to the store) and apply retention. Returns the new scan id.
    pub fn save_scan(
        &mut self,
        tree: &ScanTree,
        categories: &[(&str, u64)],
        started_at_ms: i64,
        finished_at_ms: i64,
        retention: Retention,
    ) -> Result<ScanId> {
        let tx = self.conn.transaction()?;
        let root = tree.root.display().to_string();
        let s = &tree.stats;
        tx.execute(
            "INSERT INTO scans (root, started_at_ms, finished_at_ms, total_bytes,
                total_logical_bytes, files, dirs, access_denied, errors, links_skipped,
                excluded, cancelled, truncated, elapsed_ms, min_node_bytes)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                root,
                started_at_ms,
                finished_at_ms,
                to_i(s.total_bytes),
                to_i(s.total_logical_bytes),
                to_i(s.files),
                to_i(s.dirs),
                to_i(s.access_denied),
                to_i(s.errors),
                to_i(s.links_skipped),
                to_i(s.excluded),
                s.cancelled,
                s.truncated,
                to_i(s.elapsed_ms),
                to_i(retention.min_node_bytes),
            ],
        )?;
        let id = tx.last_insert_rowid();
        insert_nodes(&tx, id, tree, retention.min_node_bytes)?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO scan_categories (scan_id, category, bytes) VALUES (?1, ?2, ?3)",
            )?;
            for (category, bytes) in categories {
                stmt.execute(params![id, category, to_i(*bytes)])?;
            }
        }
        {
            let mut stmt = tx.prepare(
                "INSERT INTO scan_largest_files (scan_id, rank, path, bytes, logical_bytes)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for (rank, f) in tree.largest_files.iter().enumerate() {
                stmt.execute(params![
                    id,
                    rank as i64,
                    f.path,
                    to_i(f.bytes),
                    to_i(f.logical_bytes)
                ])?;
            }
        }
        let removed = tx.execute(
            "DELETE FROM scans WHERE root = ?1 AND id NOT IN (
                 SELECT id FROM scans WHERE root = ?1
                 ORDER BY finished_at_ms DESC, id DESC LIMIT ?2)",
            params![root, retention.keep_scans_per_root],
        )?;
        tx.commit()?;
        tracing::info!(id, root, removed_old = removed, "scan saved");
        Ok(id)
    }

    fn scan_where(&self, clause: &str, p: impl rusqlite::Params) -> Result<Option<ScanRecord>> {
        let sql = format!(
            "SELECT id, root, started_at_ms, finished_at_ms, total_bytes, total_logical_bytes,
                    files, dirs, access_denied, errors, links_skipped, excluded, cancelled,
                    truncated, elapsed_ms, min_node_bytes
             FROM scans {clause} LIMIT 1"
        );
        Ok(self
            .conn
            .query_row(&sql, p, |r| {
                Ok(ScanRecord {
                    id: r.get(0)?,
                    root: r.get(1)?,
                    started_at_ms: r.get(2)?,
                    finished_at_ms: r.get(3)?,
                    stats: ScanStats {
                        total_bytes: to_u(r.get(4)?),
                        total_logical_bytes: to_u(r.get(5)?),
                        files: to_u(r.get(6)?),
                        dirs: to_u(r.get(7)?),
                        access_denied: to_u(r.get(8)?),
                        errors: to_u(r.get(9)?),
                        links_skipped: to_u(r.get(10)?),
                        excluded: to_u(r.get(11)?),
                        cancelled: r.get(12)?,
                        truncated: r.get(13)?,
                        elapsed_ms: to_u(r.get(14)?),
                    },
                    min_node_bytes: to_u(r.get(15)?),
                })
            })
            .optional()?)
    }

    pub fn scan(&self, id: ScanId) -> Result<Option<ScanRecord>> {
        self.scan_where("WHERE id = ?1", [id])
    }

    /// Most recent scan overall.
    pub fn latest_scan(&self) -> Result<Option<ScanRecord>> {
        self.scan_where("ORDER BY finished_at_ms DESC, id DESC", [])
    }

    /// The scan of the same root that came immediately before `id`.
    pub fn previous_scan(&self, id: ScanId) -> Result<Option<ScanRecord>> {
        self.scan_where(
            "WHERE root = (SELECT root FROM scans WHERE id = ?1)
               AND (finished_at_ms, id) < (SELECT finished_at_ms, id FROM scans WHERE id = ?1)
             ORDER BY finished_at_ms DESC, id DESC",
            [id],
        )
    }

    fn node_where(&self, clause: &str, p: impl rusqlite::Params) -> Result<Vec<StoredNode>> {
        let sql = format!(
            "SELECT node, parent, name, total_bytes, own_bytes, file_count, own_files,
                    child_count, pruned_children, pruned_bytes, status
             FROM scan_nodes {clause}"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(p, |r| {
            Ok((
                StoredNode {
                    id: r.get(0)?,
                    parent: r.get(1)?,
                    name: r.get(2)?,
                    total_bytes: to_u(r.get(3)?),
                    own_bytes: to_u(r.get(4)?),
                    file_count: to_u(r.get(5)?),
                    own_files: to_u(r.get(6)?),
                    child_count: to_u(r.get(7)?),
                    pruned_children: to_u(r.get(8)?),
                    pruned_bytes: to_u(r.get(9)?),
                    status: NodeStatus::Complete,
                },
                r.get::<_, String>(10)?,
            ))
        })?;
        rows.map(|row| {
            let (mut node, status) = row?;
            node.status = serde_json::from_str(&status)
                .map_err(|e| StoreError::Corrupt(format!("node status {status:?}: {e}")))?;
            Ok(node)
        })
        .collect()
    }

    pub fn node(&self, scan: ScanId, node: NodeId) -> Result<Option<StoredNode>> {
        Ok(self
            .node_where("WHERE scan_id = ?1 AND node = ?2", params![scan, node])?
            .pop())
    }

    /// Stored children of a node, largest first, at most `limit`.
    pub fn children(&self, scan: ScanId, node: NodeId, limit: u32) -> Result<Vec<StoredNode>> {
        self.node_where(
            "WHERE scan_id = ?1 AND parent = ?2 ORDER BY total_bytes DESC, name LIMIT ?3",
            params![scan, node, limit],
        )
    }

    /// Number and total size of stored children beyond the first `skip` (largest first).
    pub fn children_beyond(&self, scan: ScanId, node: NodeId, skip: u32) -> Result<(u64, u64)> {
        let (n, b): (i64, i64) = self.conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(total_bytes), 0) FROM (
                 SELECT total_bytes FROM scan_nodes WHERE scan_id = ?1 AND parent = ?2
                 ORDER BY total_bytes DESC, name LIMIT -1 OFFSET ?3)",
            params![scan, node, skip],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        Ok((to_u(n), to_u(b)))
    }

    /// Ancestors of `node` from the root down to the node itself.
    pub fn crumbs(&self, scan: ScanId, node: NodeId) -> Result<Vec<StoredNode>> {
        let mut out = Vec::new();
        let mut cur = Some(node);
        while let Some(id) = cur {
            let Some(n) = self.node(scan, id)? else {
                break;
            };
            cur = n.parent;
            out.push(n);
            if out.len() > 4096 {
                return Err(StoreError::Corrupt("folder parent chain is cyclic".into()));
            }
        }
        out.reverse();
        Ok(out)
    }

    /// Find the node reached from the root by following child `names`.
    pub fn find_by_names(&self, scan: ScanId, names: &[&str]) -> Result<Option<NodeId>> {
        let mut cur: NodeId = 0;
        if self.node(scan, cur)?.is_none() {
            return Ok(None);
        }
        for name in names {
            let next: Option<NodeId> = self
                .conn
                .query_row(
                    "SELECT node FROM scan_nodes WHERE scan_id = ?1 AND parent = ?2 AND name = ?3",
                    params![scan, cur, name],
                    |r| r.get(0),
                )
                .optional()?;
            match next {
                Some(n) => cur = n,
                None => return Ok(None),
            }
        }
        Ok(Some(cur))
    }

    /// Per-category byte totals saved with a scan, largest first. Empty for scans saved
    /// before classification existed.
    pub fn scan_categories(&self, scan: ScanId) -> Result<Vec<(String, u64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT category, bytes FROM scan_categories WHERE scan_id = ?1
             ORDER BY bytes DESC, category",
        )?;
        let rows = stmt.query_map([scan], |r| Ok((r.get(0)?, to_u(r.get(1)?))))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn largest_files(&self, scan: ScanId) -> Result<Vec<LargeFile>> {
        let mut stmt = self.conn.prepare(
            "SELECT path, bytes, logical_bytes FROM scan_largest_files
             WHERE scan_id = ?1 ORDER BY rank",
        )?;
        let rows = stmt.query_map([scan], |r| {
            Ok(LargeFile {
                path: r.get(0)?,
                bytes: to_u(r.get(1)?),
                logical_bytes: to_u(r.get(2)?),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Record a drive capacity snapshot unless one was taken within `min_interval_ms`.
    /// Returns whether a snapshot was written.
    pub fn record_drive_snapshot(
        &self,
        root: &str,
        snapshot: DriveSnapshot,
        min_interval_ms: i64,
    ) -> Result<bool> {
        let last: Option<i64> = self.conn.query_row(
            "SELECT MAX(taken_at_ms) FROM drive_snapshots WHERE root = ?1",
            [root],
            |r| r.get(0),
        )?;
        if last.is_some_and(|t| snapshot.taken_at_ms - t < min_interval_ms) {
            return Ok(false);
        }
        self.conn.execute(
            "INSERT OR REPLACE INTO drive_snapshots (root, taken_at_ms, total_bytes, free_bytes)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                root,
                snapshot.taken_at_ms,
                to_i(snapshot.total_bytes),
                to_i(snapshot.free_bytes)
            ],
        )?;
        Ok(true)
    }

    /// Drive roots that have at least one snapshot.
    pub fn snapshot_roots(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT root FROM drive_snapshots ORDER BY root")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Snapshots for a drive, oldest first, taken at or after `since_ms`.
    pub fn drive_history(&self, root: &str, since_ms: i64) -> Result<Vec<DriveSnapshot>> {
        let mut stmt = self.conn.prepare(
            "SELECT taken_at_ms, total_bytes, free_bytes FROM drive_snapshots
             WHERE root = ?1 AND taken_at_ms >= ?2 ORDER BY taken_at_ms",
        )?;
        let rows = stmt.query_map(params![root, since_ms], |r| {
            Ok(DriveSnapshot {
                taken_at_ms: r.get(0)?,
                total_bytes: to_u(r.get(1)?),
                free_bytes: to_u(r.get(2)?),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

/// Insert the pruned tree. Arena order is pre-order (parents before children), so a
/// single forward pass decides what to keep. Subtree sizes never grow away from the
/// root, so every kept node's parent is also kept.
fn insert_nodes(tx: &Transaction<'_>, scan: ScanId, tree: &ScanTree, min: u64) -> Result<()> {
    let n = tree.nodes.len();
    let mut keep = vec![false; n];
    let mut pruned = vec![(0u64, 0u64); n];
    for (i, node) in tree.nodes.iter().enumerate() {
        keep[i] = match node.parent {
            None => true,
            Some(p) => {
                let p = p as usize;
                let k = keep[p]
                    && (node.total_bytes >= min || !matches!(node.status, NodeStatus::Complete));
                if keep[p] && !k {
                    pruned[p].0 += 1;
                    pruned[p].1 += node.total_bytes;
                }
                k
            }
        };
    }
    let mut stmt = tx.prepare(
        "INSERT INTO scan_nodes (scan_id, node, parent, name, total_bytes, own_bytes,
             file_count, own_files, child_count, pruned_children, pruned_bytes, status)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
    )?;
    let mut stored = 0usize;
    for (i, node) in tree.nodes.iter().enumerate() {
        if !keep[i] {
            continue;
        }
        let child_files: u64 = node
            .children
            .iter()
            .map(|&c| tree.nodes[c as usize].file_count)
            .sum();
        let status = serde_json::to_string(&node.status)
            .map_err(|e| StoreError::Corrupt(format!("node status: {e}")))?;
        stmt.execute(params![
            scan,
            i as i64,
            node.parent,
            node.name,
            to_i(node.total_bytes),
            to_i(node.own_bytes),
            to_i(node.file_count),
            to_i(node.file_count.saturating_sub(child_files)),
            node.children.len() as i64,
            to_i(pruned[i].0),
            to_i(pruned[i].1),
            status,
        ])?;
        stored += 1;
    }
    tracing::debug!(scan, stored, total = n, "scan tree stored");
    Ok(())
}

/// A saved project search: the folder searched, when, and the result as JSON (opaque to
/// the store).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectSearchRow {
    pub root: String,
    pub searched_at_ms: i64,
    pub result_json: String,
}

impl Store {
    /// Save (or replace) the latest search of `root`.
    pub fn save_project_search(&self, root: &str, at_ms: i64, result_json: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO project_searches (root, searched_at_ms, result) VALUES (?1, ?2, ?3)
             ON CONFLICT(root) DO UPDATE SET searched_at_ms = ?2, result = ?3",
            params![root, at_ms, result_json],
        )?;
        Ok(())
    }

    pub fn project_searches(&self) -> Result<Vec<ProjectSearchRow>> {
        let mut stmt = self
            .conn
            .prepare("SELECT root, searched_at_ms, result FROM project_searches ORDER BY root")?;
        let rows = stmt.query_map([], |r| {
            Ok(ProjectSearchRow {
                root: r.get(0)?,
                searched_at_ms: r.get(1)?,
                result_json: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Forget a searched folder. Returns whether it existed.
    pub fn remove_project_search(&self, root: &str) -> Result<bool> {
        Ok(self
            .conn
            .execute("DELETE FROM project_searches WHERE root = ?1", [root])?
            > 0)
    }
}
