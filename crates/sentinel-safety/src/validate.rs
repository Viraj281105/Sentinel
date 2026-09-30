use std::fs::{self, OpenOptions};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    GetFileInformationByHandle,
};

use crate::error::SafetyError;
use crate::path::{CanonicalPath, check_lexical, fold, is_within};
use crate::protected::ProtectedSet;

const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
const FILE_SHARE_ALL: u32 = 0x7;

/// What kind of filesystem object a validated target is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    File,
    Directory,
    /// A symlink or junction. Only the link itself may be acted upon, never its target.
    Link,
}

/// Volume serial number and file index: the identity of a filesystem object,
/// independent of its path. Used to detect a path being swapped after validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileIdentity {
    volume_serial: u32,
    file_index: u64,
}

fn identity_of(path: &Path) -> Result<FileIdentity, SafetyError> {
    // Query-only access; open the object itself (not a link target).
    let file = OpenOptions::new()
        .access_mode(0)
        .share_mode(FILE_SHARE_ALL)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0 | FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)
        .map_err(|e| SafetyError::io(path, e))?;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: the handle is valid for the lifetime of `file`; `info` is a valid out-pointer.
    unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info) }
        .map_err(|e| SafetyError::io(path, e.into()))?;
    Ok(FileIdentity {
        volume_serial: info.dwVolumeSerialNumber,
        file_index: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
    })
}

/// A directory a cleanup provider is allowed to operate inside.
///
/// Constructed via [`Policy::allowed_root`]; the root itself must not be protected.
#[derive(Debug, Clone)]
pub struct AllowedRoot {
    path: CanonicalPath,
}

impl AllowedRoot {
    pub fn path(&self) -> &Path {
        self.path.as_path()
    }
}

/// A filesystem object that passed every safety check.
///
/// Fields are private and the type has no public constructor: the only source is
/// [`Policy::validate`]. The executor must call [`ValidatedTarget::revalidate`]
/// immediately before acting.
#[derive(Debug, Clone)]
pub struct ValidatedTarget {
    root: AllowedRoot,
    path: CanonicalPath,
    kind: TargetKind,
    identity: FileIdentity,
}

impl ValidatedTarget {
    pub fn path(&self) -> &Path {
        self.path.as_path()
    }

    pub fn kind(&self) -> TargetKind {
        self.kind
    }

    pub fn root(&self) -> &AllowedRoot {
        &self.root
    }

    /// Re-run all checks and confirm the object is the same one that was validated.
    /// Call this immediately before a mutation to narrow the check-to-use window.
    pub fn revalidate(&self, policy: &Policy) -> Result<(), SafetyError> {
        let again = policy.validate(&self.root, self.path.as_path())?;
        if again.identity != self.identity || again.kind != self.kind {
            return Err(SafetyError::IdentityChanged(
                self.path.as_path().to_path_buf(),
            ));
        }
        Ok(())
    }
}

/// The deterministic policy engine for filesystem targets.
#[derive(Debug, Clone)]
pub struct Policy {
    protected: ProtectedSet,
}

impl Policy {
    pub fn new(protected: ProtectedSet) -> Self {
        Self { protected }
    }

    /// Policy using the protection set derived from this machine's known folders.
    pub fn for_system() -> Self {
        Self::new(ProtectedSet::from_system())
    }

    pub fn protected(&self) -> &ProtectedSet {
        &self.protected
    }

    /// Check an arbitrary path (e.g. a descendant found while enumerating a target)
    /// against protection rules. Does not touch the filesystem.
    pub fn check_protected(&self, path: &Path) -> Result<(), SafetyError> {
        self.protected.check(path)
    }

    /// Establish a directory that a provider may act inside. The directory is resolved
    /// (links followed) and must exist, be a directory, and not be protected.
    pub fn allowed_root(&self, path: &Path) -> Result<AllowedRoot, SafetyError> {
        let canon = CanonicalPath::resolve(path)?;
        let md = fs::metadata(canon.as_path()).map_err(|e| SafetyError::io(canon.as_path(), e))?;
        if !md.is_dir() {
            return Err(SafetyError::NotADirectory(canon.as_path().to_path_buf()));
        }
        self.protected.check_location(canon.as_path())?;
        Ok(AllowedRoot { path: canon })
    }

    /// Validate `candidate` as an actionable target inside `root`.
    ///
    /// Symlinks and junctions are never followed. Every ancestor between the root and
    /// the target must be a real directory; the final component may itself be a link
    /// (reported as [`TargetKind::Link`]) so the link can be removed without touching
    /// what it points to.
    pub fn validate(
        &self,
        root: &AllowedRoot,
        candidate: &Path,
    ) -> Result<ValidatedTarget, SafetyError> {
        check_lexical(candidate)?;
        if !is_within(candidate, root.path()) {
            return Err(SafetyError::OutsideRoot {
                path: candidate.to_path_buf(),
                root: root.path().to_path_buf(),
            });
        }
        if is_within(root.path(), candidate) {
            return Err(SafetyError::TargetIsRoot(candidate.to_path_buf()));
        }
        let (Some(parent), Some(name)) = (candidate.parent(), candidate.file_name()) else {
            return Err(SafetyError::invalid(candidate, "path has no parent"));
        };

        // The parent must already be in canonical form: this proves that no ancestor
        // below the root is a link/junction and no component is a short-name alias.
        let canon_parent = CanonicalPath::resolve(parent)?;
        if !same_path(canon_parent.as_path(), parent) {
            return Err(SafetyError::NotCanonical(candidate.to_path_buf()));
        }
        if !is_within(canon_parent.as_path(), root.path()) {
            return Err(SafetyError::OutsideRoot {
                path: candidate.to_path_buf(),
                root: root.path().to_path_buf(),
            });
        }

        let md = fs::symlink_metadata(candidate).map_err(|e| SafetyError::io(candidate, e))?;
        let is_link = md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0;
        let kind = if is_link {
            TargetKind::Link
        } else if md.is_dir() {
            TargetKind::Directory
        } else {
            TargetKind::File
        };

        let final_name: PathBuf = if is_link {
            // Cannot canonicalize a link without following it. Short (8.3) aliases
            // always contain '~', so refuse those rather than trust the spelling.
            if name.to_string_lossy().contains('~') {
                return Err(SafetyError::NotCanonical(candidate.to_path_buf()));
            }
            name.into()
        } else {
            let full = CanonicalPath::resolve(candidate)?;
            let Some(actual) = full.as_path().file_name() else {
                return Err(SafetyError::NotCanonical(candidate.to_path_buf()));
            };
            if fold(actual) != fold(name)
                || !same_path(
                    full.as_path().parent().unwrap_or(Path::new("")),
                    canon_parent.as_path(),
                )
            {
                return Err(SafetyError::NotCanonical(candidate.to_path_buf()));
            }
            actual.into()
        };

        let path = CanonicalPath::from_verified(canon_parent.as_path().join(final_name));
        self.protected.check(path.as_path())?;
        let identity = identity_of(path.as_path())?;
        Ok(ValidatedTarget {
            root: root.clone(),
            path,
            kind,
            identity,
        })
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    is_within(a, b) && is_within(b, a)
}
