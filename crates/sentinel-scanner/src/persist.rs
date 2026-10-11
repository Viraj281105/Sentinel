//! Saving the last full tree to disk, so an incremental update still works after a restart.
//!
//! A snapshot is a [`ScanTree`] plus the facts needed to decide whether it can still be
//! trusted: which saved analysis it belongs to and where the volume's change journal stood
//! when the scan began (see [`crate::usn`]). It is only ever *read back* if the journal says
//! nothing changed on the volume since.
//!
//! The format is a small hand-written binary layout rather than a general serializer, because
//! the input is a file on disk and decoding must be strict: every read is bounds-checked, no
//! count is believed before it is checked against the bytes that remain, the structure is
//! validated (parents and children agree, every folder is reachable from the root exactly once)
//! and a checksum catches truncation and damage. Anything wrong is an error; nothing is
//! repaired. A bad snapshot only costs a full analysis.

use std::path::PathBuf;

use crate::scan::{
    DirNode, LargeFile, LinkKind, NodeId, NodeStatus, NotScannedReason, ScanStats, ScanTree,
};
use crate::usn::JournalPosition;

const MAGIC: &[u8; 8] = b"SNTLTREE";
const VERSION: u32 = 1;
const NONE_ID: u32 = u32::MAX;
/// Far above the app's own retention limit; stops a corrupt count from asking for gigabytes.
const MAX_NODES: usize = 4_000_000;
const MAX_FILES: usize = 100_000;
/// The least a node can occupy when encoded, to bound a node count by the bytes that remain.
const MIN_NODE_BYTES: usize = 4 + 4 + 4 + 5 * 8 + 2 + 2 + 1;
const MIN_FILE_BYTES: usize = 4 + 8 + 8 + 1;

#[derive(Debug, Clone)]
pub struct Snapshot {
    /// The saved analysis this tree is the full version of.
    pub scan_id: i64,
    /// The journal position captured *before* the scan began.
    pub journal: JournalPosition,
    pub saved_at_ms: i64,
    pub tree: ScanTree,
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum SnapshotError {
    #[error("not a Sentinel tree snapshot")]
    NotASnapshot,
    #[error("written by a different version of Sentinel")]
    Version,
    #[error("the snapshot is damaged: {0}")]
    Damaged(&'static str),
    #[error("the tree cannot be saved: {0}")]
    Unsupported(&'static str),
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

struct Writer(Vec<u8>);

impl Writer {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn i64(&mut self, v: i64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn opt_i64(&mut self, v: Option<i64>) {
        match v {
            Some(v) => {
                self.u8(1);
                self.i64(v);
            }
            None => self.u8(0),
        }
    }
    fn str(&mut self, s: &str) -> Result<(), SnapshotError> {
        let len =
            u32::try_from(s.len()).map_err(|_| SnapshotError::Unsupported("a name is too long"))?;
        self.u32(len);
        self.0.extend_from_slice(s.as_bytes());
        Ok(())
    }
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], SnapshotError> {
        let end = self
            .at
            .checked_add(n)
            .ok_or(SnapshotError::Damaged("length"))?;
        let s = self
            .b
            .get(self.at..end)
            .ok_or(SnapshotError::Damaged("truncated"))?;
        self.at = end;
        Ok(s)
    }
    fn remaining(&self) -> usize {
        self.b.len() - self.at
    }
    fn u8(&mut self) -> Result<u8, SnapshotError> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32, SnapshotError> {
        self.take(4)?
            .try_into()
            .map(u32::from_le_bytes)
            .map_err(|_| SnapshotError::Damaged("u32"))
    }
    fn u64(&mut self) -> Result<u64, SnapshotError> {
        self.take(8)?
            .try_into()
            .map(u64::from_le_bytes)
            .map_err(|_| SnapshotError::Damaged("u64"))
    }
    fn i64(&mut self) -> Result<i64, SnapshotError> {
        self.take(8)?
            .try_into()
            .map(i64::from_le_bytes)
            .map_err(|_| SnapshotError::Damaged("i64"))
    }
    fn bool(&mut self) -> Result<bool, SnapshotError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(SnapshotError::Damaged("flag")),
        }
    }
    fn opt_i64(&mut self) -> Result<Option<i64>, SnapshotError> {
        if self.bool()? {
            self.i64().map(Some)
        } else {
            Ok(None)
        }
    }
    fn str(&mut self) -> Result<String, SnapshotError> {
        let len = self.u32()? as usize;
        let bytes = self.take(len)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| SnapshotError::Damaged("text"))
    }
    /// A count of items each at least `min_each` bytes: refused if the remaining bytes
    /// could not hold that many, so a corrupt count never allocates.
    fn count(&mut self, min_each: usize, max: usize) -> Result<usize, SnapshotError> {
        let n = self.u32()? as usize;
        if n > max || n.saturating_mul(min_each) > self.remaining() {
            return Err(SnapshotError::Damaged("count"));
        }
        Ok(n)
    }
}

fn write_status(w: &mut Writer, s: &NodeStatus) -> Result<(), SnapshotError> {
    match s {
        NodeStatus::Complete => w.u8(0),
        NodeStatus::AccessDenied => w.u8(1),
        NodeStatus::Error { message } => {
            w.u8(2);
            w.str(message)?;
        }
        NodeStatus::Link { kind } => {
            w.u8(3);
            w.u8(match kind {
                LinkKind::Junction => 0,
                LinkKind::Symlink => 1,
                LinkKind::Other => 2,
            });
        }
        NodeStatus::Excluded => w.u8(4),
        NodeStatus::NotScanned { reason } => {
            w.u8(5);
            w.u8(match reason {
                NotScannedReason::Cancelled => 0,
                NotScannedReason::DepthLimit => 1,
                NotScannedReason::EntryLimit => 2,
                NotScannedReason::OnlineOnly => 3,
            });
        }
    }
    Ok(())
}

fn read_status(r: &mut Reader<'_>) -> Result<NodeStatus, SnapshotError> {
    Ok(match r.u8()? {
        0 => NodeStatus::Complete,
        1 => NodeStatus::AccessDenied,
        2 => NodeStatus::Error { message: r.str()? },
        3 => NodeStatus::Link {
            kind: match r.u8()? {
                0 => LinkKind::Junction,
                1 => LinkKind::Symlink,
                2 => LinkKind::Other,
                _ => return Err(SnapshotError::Damaged("link kind")),
            },
        },
        4 => NodeStatus::Excluded,
        5 => NodeStatus::NotScanned {
            reason: match r.u8()? {
                0 => NotScannedReason::Cancelled,
                1 => NotScannedReason::DepthLimit,
                2 => NotScannedReason::EntryLimit,
                3 => NotScannedReason::OnlineOnly,
                _ => return Err(SnapshotError::Damaged("reason")),
            },
        },
        _ => return Err(SnapshotError::Damaged("status")),
    })
}

pub fn encode(s: &Snapshot) -> Result<Vec<u8>, SnapshotError> {
    let mut w = Writer(Vec::with_capacity(64 + s.tree.nodes.len() * 64));
    w.0.extend_from_slice(MAGIC);
    w.u32(VERSION);
    w.i64(s.scan_id);
    w.u64(s.journal.journal_id);
    w.u64(s.journal.next_usn);
    w.i64(s.saved_at_ms);
    w.str(
        s.tree
            .root
            .to_str()
            .ok_or(SnapshotError::Unsupported("the root is not valid text"))?,
    )?;

    let t = &s.tree;
    let n =
        u32::try_from(t.nodes.len()).map_err(|_| SnapshotError::Unsupported("too many folders"))?;
    w.u32(n);
    for node in &t.nodes {
        w.str(&node.name)?;
        w.u32(node.parent.unwrap_or(NONE_ID));
        let kids = u32::try_from(node.children.len())
            .map_err(|_| SnapshotError::Unsupported("too many children"))?;
        w.u32(kids);
        for c in &node.children {
            w.u32(*c);
        }
        w.u64(node.own_bytes);
        w.u64(node.total_bytes);
        w.u64(node.total_logical_bytes);
        w.u64(node.file_count);
        w.u64(node.dir_count);
        w.opt_i64(node.own_newest_ms);
        w.opt_i64(node.newest_ms);
        write_status(&mut w, &node.status)?;
    }

    let files = u32::try_from(t.largest_files.len())
        .map_err(|_| SnapshotError::Unsupported("too many files"))?;
    w.u32(files);
    for f in &t.largest_files {
        w.str(&f.path)?;
        w.u64(f.bytes);
        w.u64(f.logical_bytes);
        w.opt_i64(f.modified_ms);
    }

    let st = &t.stats;
    for v in [
        st.dirs,
        st.files,
        st.total_bytes,
        st.total_logical_bytes,
        st.access_denied,
        st.errors,
        st.links_skipped,
        st.excluded,
        st.elapsed_ms,
    ] {
        w.u64(v);
    }
    w.u8(u8::from(st.cancelled));
    w.u8(u8::from(st.truncated));

    let sum = fnv1a(&w.0);
    w.u64(sum);
    Ok(w.0)
}

pub fn decode(bytes: &[u8]) -> Result<Snapshot, SnapshotError> {
    if bytes.len() < MAGIC.len() + 4 + 8 || &bytes[..MAGIC.len()] != MAGIC {
        return Err(SnapshotError::NotASnapshot);
    }
    let (body, sum) = bytes.split_at(bytes.len() - 8);
    let stored = u64::from_le_bytes(
        sum.try_into()
            .map_err(|_| SnapshotError::Damaged("checksum"))?,
    );
    let mut r = Reader {
        b: body,
        at: MAGIC.len(),
    };
    if r.u32()? != VERSION {
        return Err(SnapshotError::Version);
    }
    if fnv1a(body) != stored {
        return Err(SnapshotError::Damaged("checksum does not match"));
    }
    let scan_id = r.i64()?;
    let journal = JournalPosition {
        journal_id: r.u64()?,
        next_usn: r.u64()?,
    };
    let saved_at_ms = r.i64()?;
    let root = PathBuf::from(r.str()?);

    let n = r.count(MIN_NODE_BYTES, MAX_NODES)?;
    let mut nodes = Vec::with_capacity(n);
    for _ in 0..n {
        let name = r.str()?;
        let parent = match r.u32()? {
            NONE_ID => None,
            p => Some(p),
        };
        let kids = r.count(4, MAX_NODES)?;
        let mut children = Vec::with_capacity(kids);
        for _ in 0..kids {
            children.push(r.u32()?);
        }
        nodes.push(DirNode {
            name,
            parent,
            children,
            own_bytes: r.u64()?,
            total_bytes: r.u64()?,
            total_logical_bytes: r.u64()?,
            file_count: r.u64()?,
            dir_count: r.u64()?,
            own_newest_ms: r.opt_i64()?,
            newest_ms: r.opt_i64()?,
            status: read_status(&mut r)?,
        });
    }
    validate_structure(&nodes)?;

    let f = r.count(MIN_FILE_BYTES, MAX_FILES)?;
    let mut largest_files = Vec::with_capacity(f);
    for _ in 0..f {
        largest_files.push(LargeFile {
            path: r.str()?,
            bytes: r.u64()?,
            logical_bytes: r.u64()?,
            modified_ms: r.opt_i64()?,
        });
    }
    let stats = ScanStats {
        dirs: r.u64()?,
        files: r.u64()?,
        total_bytes: r.u64()?,
        total_logical_bytes: r.u64()?,
        access_denied: r.u64()?,
        errors: r.u64()?,
        links_skipped: r.u64()?,
        excluded: r.u64()?,
        elapsed_ms: r.u64()?,
        cancelled: r.bool()?,
        truncated: r.bool()?,
    };
    if r.remaining() != 0 {
        return Err(SnapshotError::Damaged("trailing bytes"));
    }
    Ok(Snapshot {
        scan_id,
        journal,
        saved_at_ms,
        tree: ScanTree {
            root,
            nodes,
            largest_files,
            stats,
        },
    })
}

/// The arena must be a proper tree: node 0 is the only root, parents and children agree, and
/// every node is reachable from the root exactly once (so there are no cycles or strays).
fn validate_structure(nodes: &[DirNode]) -> Result<(), SnapshotError> {
    let bad = |m| Err(SnapshotError::Damaged(m));
    let Some(first) = nodes.first() else {
        return bad("no root");
    };
    if first.parent.is_some() {
        return bad("the root has a parent");
    }
    let n = nodes.len();
    let mut seen = vec![false; n];
    seen[0] = true;
    let mut stack: Vec<NodeId> = vec![0];
    let mut visited = 1usize;
    while let Some(id) = stack.pop() {
        for &c in &nodes[id as usize].children {
            let ci = c as usize;
            if ci >= n || ci == 0 {
                return bad("a child is out of range");
            }
            if nodes[ci].parent != Some(id) {
                return bad("a child disagrees about its parent");
            }
            if std::mem::replace(&mut seen[ci], true) {
                return bad("a folder appears twice");
            }
            visited += 1;
            stack.push(c);
        }
    }
    if visited != n {
        return bad("some folders are unreachable");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::scan::{ScanControl, ScanOptions, scan};

    fn real_tree() -> ScanTree {
        let dir = tempfile::tempdir().unwrap();
        for d in ["a/b/c", "a/d", "e"] {
            std::fs::create_dir_all(dir.path().join(d)).unwrap();
        }
        for (f, n) in [
            ("a/b/c/x.bin", 5000),
            ("a/d/y.bin", 100),
            ("e/z.bin", 90_000),
            ("top.bin", 7),
        ] {
            std::fs::write(dir.path().join(f), vec![1u8; n]).unwrap();
        }
        let mut t = scan(dir.path(), &ScanOptions::default(), &ScanControl::new()).unwrap();
        // Cover every status variant, which a scan of ordinary folders does not produce.
        let statuses = [
            NodeStatus::AccessDenied,
            NodeStatus::Error {
                message: "bad ünïcode ✓".into(),
            },
            NodeStatus::Link {
                kind: LinkKind::Symlink,
            },
            NodeStatus::Excluded,
            NodeStatus::NotScanned {
                reason: NotScannedReason::OnlineOnly,
            },
        ];
        for (i, s) in statuses.into_iter().enumerate() {
            t.nodes[i + 1].status = s;
        }
        t.nodes[1].newest_ms = None;
        t.stats.truncated = true;
        t
    }

    fn snap(tree: ScanTree) -> Snapshot {
        Snapshot {
            scan_id: 42,
            journal: JournalPosition {
                journal_id: 0xDEAD_BEEF_0000_0001,
                next_usn: 123_456_789,
            },
            saved_at_ms: 1_700_000_000_000,
            tree,
        }
    }

    #[test]
    fn a_tree_survives_the_round_trip_exactly() {
        let s = snap(real_tree());
        let back = decode(&encode(&s).unwrap()).unwrap();
        assert_eq!(back.scan_id, 42);
        assert_eq!(back.journal, s.journal);
        assert_eq!(back.saved_at_ms, s.saved_at_ms);
        assert_eq!(format!("{:?}", back.tree), format!("{:?}", s.tree));
        assert!(back.tree.nodes.len() >= 6);
    }

    #[test]
    fn every_truncation_and_every_flipped_byte_is_refused_never_a_panic() {
        let bytes = encode(&snap(real_tree())).unwrap();
        for cut in 0..bytes.len() {
            assert!(decode(&bytes[..cut]).is_err(), "cut at {cut}");
        }
        for i in 0..bytes.len() {
            let mut b = bytes.clone();
            b[i] ^= 0x41;
            assert!(decode(&b).is_err(), "flip at {i}");
        }
        assert_eq!(
            decode(b"definitely not a snapshot at all").unwrap_err(),
            SnapshotError::NotASnapshot
        );
    }

    #[test]
    fn a_different_version_is_recognized_not_misread() {
        let mut b = encode(&snap(real_tree())).unwrap();
        b[8] = 9; // version
        let sum = fnv1a(&b[..b.len() - 8]);
        let n = b.len();
        b[n - 8..].copy_from_slice(&sum.to_le_bytes());
        assert_eq!(decode(&b).unwrap_err(), SnapshotError::Version);
    }

    /// A well-formed file (valid checksum) describing an impossible tree must be refused.
    fn refused_after(mutate: impl FnOnce(&mut ScanTree)) {
        let mut t = real_tree();
        mutate(&mut t);
        let b = encode(&snap(t)).unwrap();
        assert!(matches!(decode(&b), Err(SnapshotError::Damaged(_))));
    }

    #[test]
    fn impossible_structures_are_refused_even_with_a_valid_checksum() {
        refused_after(|t| t.nodes[0].parent = Some(1)); // root with a parent
        refused_after(|t| t.nodes[1].parent = Some(2)); // child disagrees about its parent
        refused_after(|t| t.nodes[0].children.push(999)); // out of range
        refused_after(|t| t.nodes[0].children.push(0)); // root as a child
        refused_after(|t| {
            let dup = t.nodes[0].children[0];
            t.nodes[0].children.push(dup); // appears twice
        });
        refused_after(|t| {
            // Detached subtree: no longer reachable from the root.
            let c = t.nodes[0].children.remove(0);
            t.nodes[c as usize].parent = Some(c);
        });
        refused_after(|t| t.nodes.clear()); // no root
    }

    #[test]
    fn an_enormous_count_does_not_allocate() {
        let mut b = encode(&snap(real_tree())).unwrap();
        // The node count sits after magic(8) version(4) id(8) journal(16) time(8) rootlen+text.
        let root_len = u32::from_le_bytes(b[44..48].try_into().unwrap()) as usize;
        let at = 48 + root_len;
        b[at..at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        let sum = fnv1a(&b[..b.len() - 8]);
        let n = b.len();
        b[n - 8..].copy_from_slice(&sum.to_le_bytes());
        assert!(matches!(decode(&b), Err(SnapshotError::Damaged("count"))));
    }
}
