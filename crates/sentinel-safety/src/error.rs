use std::io;
use std::path::{Path, PathBuf};

/// Why a path was rejected. Messages are written for end users.
#[derive(Debug, thiserror::Error)]
pub enum SafetyError {
    #[error("invalid path {path}: {reason}")]
    InvalidPath { path: PathBuf, reason: &'static str },
    #[error("path does not exist: {0}")]
    NotFound(PathBuf),
    #[error("permission denied: {0}")]
    PermissionDenied(PathBuf),
    #[error("{path} is outside the allowed location {root}")]
    OutsideRoot { path: PathBuf, root: PathBuf },
    #[error("{0} is the allowed location itself; only items inside it may be selected")]
    TargetIsRoot(PathBuf),
    #[error("{0} is not in canonical form (it uses a symlink, junction or short 8.3 name)")]
    NotCanonical(PathBuf),
    #[error("{path} is protected: {reason}")]
    Protected { path: PathBuf, reason: String },
    #[error("{path} contains the protected location {protected}: {reason}")]
    ContainsProtected {
        path: PathBuf,
        protected: PathBuf,
        reason: String,
    },
    #[error("{0} is not a directory")]
    NotADirectory(PathBuf),
    #[error("{0} cannot be matched to exactly one directory entry")]
    AmbiguousName(PathBuf),
    #[error("{0} changed after it was validated; refusing to act on it")]
    IdentityChanged(PathBuf),
    #[error("I/O error on {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
}

impl SafetyError {
    pub(crate) fn io(path: &Path, source: io::Error) -> Self {
        match source.kind() {
            io::ErrorKind::NotFound => Self::NotFound(path.to_path_buf()),
            io::ErrorKind::PermissionDenied => Self::PermissionDenied(path.to_path_buf()),
            _ => Self::Io {
                path: path.to_path_buf(),
                source,
            },
        }
    }

    pub(crate) fn invalid(path: &Path, reason: &'static str) -> Self {
        Self::InvalidPath {
            path: path.to_path_buf(),
            reason,
        }
    }
}
