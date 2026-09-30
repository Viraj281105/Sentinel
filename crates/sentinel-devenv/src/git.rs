//! Read-only check of whether Git tracks anything under a folder, by parsing the
//! repository's index file. Git itself is never run.
//!
//! Supports index versions 2, 3 and 4 and worktrees (`.git` file with `gitdir:`). A
//! split index (`link` extension) cannot be answered from the main index alone and is
//! reported as an error, which callers must treat as "possibly tracked".

use std::fs;
use std::path::{Path, PathBuf};

/// Index files larger than this are not read.
const MAX_INDEX_BYTES: u64 = 512 * 1024 * 1024;

/// The `.git` directory for the repository containing `dir`, and `dir`'s path relative
/// to the repository's working tree (with `/` separators). `None` if not in a repository.
fn find_repo(dir: &Path) -> Result<Option<(PathBuf, String)>, String> {
    let mut cur = Some(dir);
    let mut rel: Vec<String> = Vec::new();
    while let Some(d) = cur {
        let dot_git = d.join(".git");
        if let Ok(md) = fs::symlink_metadata(&dot_git) {
            let git_dir = if md.is_dir() {
                dot_git
            } else {
                // Worktree or submodule: `.git` is a file containing `gitdir: <path>`.
                let text = fs::read_to_string(&dot_git)
                    .map_err(|e| format!("{}: {e}", dot_git.display()))?;
                let target = text
                    .lines()
                    .find_map(|l| l.strip_prefix("gitdir:"))
                    .map(str::trim)
                    .ok_or_else(|| format!("{} does not name a gitdir", dot_git.display()))?;
                let p = PathBuf::from(target);
                if p.is_absolute() { p } else { d.join(p) }
            };
            rel.reverse();
            return Ok(Some((git_dir, rel.join("/"))));
        }
        if let Some(name) = d.file_name() {
            rel.push(name.to_string_lossy().into_owned());
        }
        cur = d.parent();
    }
    Ok(None)
}

fn be32(b: &[u8], at: usize) -> Result<u32, String> {
    b.get(at..at + 4)
        .map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or_else(|| "index is truncated".to_owned())
}

fn be16(b: &[u8], at: usize) -> Result<u16, String> {
    b.get(at..at + 2)
        .map(|s| u16::from_be_bytes([s[0], s[1]]))
        .ok_or_else(|| "index is truncated".to_owned())
}

/// Git's offset varint used by index v4 path compression.
fn varint(b: &[u8], at: &mut usize) -> Result<usize, String> {
    let mut byte = *b.get(*at).ok_or("index is truncated")?;
    *at += 1;
    let mut value = usize::from(byte & 0x7f);
    while byte & 0x80 != 0 {
        byte = *b.get(*at).ok_or("index is truncated")?;
        *at += 1;
        value = value
            .checked_add(1)
            .and_then(|v| v.checked_mul(128))
            .and_then(|v| v.checked_add(usize::from(byte & 0x7f)))
            .ok_or("index varint overflow")?;
    }
    Ok(value)
}

/// Every path recorded in an index file.
pub(crate) fn index_paths(bytes: &[u8]) -> Result<Vec<String>, String> {
    if bytes.get(0..4) != Some(b"DIRC") {
        return Err("not a Git index".into());
    }
    let version = be32(bytes, 4)?;
    if !(2..=4).contains(&version) {
        return Err(format!("unsupported index version {version}"));
    }
    let count = be32(bytes, 8)? as usize;
    let mut at = 12;
    let mut paths = Vec::with_capacity(count.min(1_000_000));
    let mut prev: Vec<u8> = Vec::new();
    for _ in 0..count {
        let start = at;
        let flags = be16(bytes, at + 60)?;
        let mut fixed = 62;
        if version >= 3 && flags & 0x4000 != 0 {
            fixed += 2;
        }
        at += fixed;
        let name: Vec<u8> = if version == 4 {
            let strip = varint(bytes, &mut at)?;
            if strip > prev.len() {
                return Err("index path compression is corrupt".into());
            }
            let end = bytes[at..]
                .iter()
                .position(|&c| c == 0)
                .ok_or("index is truncated")?;
            let mut n = prev[..prev.len() - strip].to_vec();
            n.extend_from_slice(&bytes[at..at + end]);
            at += end + 1;
            n
        } else {
            let end = bytes
                .get(at..)
                .and_then(|r| r.iter().position(|&c| c == 0))
                .ok_or("index is truncated")?;
            let n = bytes[at..at + end].to_vec();
            // Entries are NUL-padded to a multiple of 8 bytes (at least one NUL).
            at = start + ((fixed + end + 8) & !7);
            n
        };
        paths.push(String::from_utf8_lossy(&name).into_owned());
        prev = name;
    }
    // Extensions follow the entries; the last 20 bytes are the checksum.
    while at + 8 <= bytes.len().saturating_sub(20) {
        let sig = &bytes[at..at + 4];
        let size = be32(bytes, at + 4)? as usize;
        if sig == b"link" {
            return Err("split index: tracked files cannot be determined".into());
        }
        at = at.checked_add(8 + size).ok_or("index extension overflow")?;
    }
    Ok(paths)
}

/// Whether Git tracks `dir` itself or anything beneath it. `Ok(false)` when `dir` is not
/// inside a repository. An `Err` means the answer is unknown and the folder must be
/// treated as possibly tracked.
pub fn tracks_anything_under(dir: &Path) -> Result<bool, String> {
    let Some((git_dir, rel)) = find_repo(dir)? else {
        return Ok(false);
    };
    let index = git_dir.join("index");
    let md = match fs::metadata(&index) {
        Ok(md) => md,
        // A repository with no index yet (nothing ever staged) tracks nothing.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(format!("{}: {e}", index.display())),
    };
    if md.len() > MAX_INDEX_BYTES {
        return Err("the Git index is too large to check".into());
    }
    let bytes = fs::read(&index).map_err(|e| format!("{}: {e}", index.display()))?;
    let paths = index_paths(&bytes)?;
    if rel.is_empty() {
        return Ok(!paths.is_empty());
    }
    let rel = rel.to_lowercase();
    let prefix = format!("{rel}/");
    Ok(paths.iter().any(|p| {
        let p = p.to_lowercase();
        p == rel || p.starts_with(&prefix)
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_index_data() {
        assert!(index_paths(b"nope").is_err());
        assert!(index_paths(b"DIRC\0\0\0\x09\0\0\0\0").is_err());
    }

    #[test]
    fn empty_index_has_no_paths() {
        let mut b = b"DIRC\0\0\0\x02\0\0\0\0".to_vec();
        b.extend([0u8; 20]);
        assert!(index_paths(&b).unwrap_or_default().is_empty());
    }

    #[test]
    fn decodes_git_offset_varints() {
        let mut at = 0;
        assert_eq!(varint(&[0x05], &mut at), Ok(5));
        let mut at = 0;
        // 0x80 0x00 encodes 128 in Git's offset varint.
        assert_eq!(varint(&[0x80, 0x00], &mut at), Ok(128));
    }
}
