//! Path safety core for Sentinel.
//!
//! This crate is the security-critical heart of the cleanup engine. It answers one
//! question: *may this exact filesystem object be acted upon?* Nothing here mutates
//! the filesystem.
//!
//! The only way to obtain a [`ValidatedTarget`] is [`Policy::validate`]; the executor
//! must accept nothing else.

#[cfg(not(windows))]
compile_error!("sentinel-safety only supports Windows");

mod error;
mod known_folders;
mod path;
mod protected;
mod risk;
mod validate;

pub use error::SafetyError;
pub use known_folders::{Known, path_of as known_folder};
pub use path::{CanonicalPath, is_reparse_point, is_within};
pub use protected::ProtectedSet;
pub use risk::RiskLevel;
pub use validate::{AllowedRoot, FileIdentity, Policy, TargetKind, ValidatedTarget};
