use serde::Serialize;
use ts_rs::TS;

/// Error returned by fallible IPC commands. `message` is written for end users.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CommandError {
    pub kind: ErrorKind,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ErrorKind {
    /// The request itself was invalid (e.g. a path that is not a folder).
    InvalidInput,
    /// The requested item does not exist (e.g. no finished scan yet).
    NotFound,
    /// Another operation of the same kind is already running.
    Busy,
    /// A Windows API Sentinel depends on failed.
    System,
    /// A bug in Sentinel itself (e.g. a background task panicked).
    Internal,
}

impl CommandError {
    pub(crate) fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        let err = Self {
            kind,
            message: message.into(),
        };
        match kind {
            ErrorKind::System | ErrorKind::Internal => {
                tracing::error!(kind = ?err.kind, message = %err.message, "command failed");
            }
            _ => tracing::debug!(kind = ?err.kind, message = %err.message, "command rejected"),
        }
        err
    }

    pub(crate) fn internal(context: &str, err: impl std::fmt::Display) -> Self {
        Self::new(
            ErrorKind::Internal,
            format!("Sentinel hit an internal error while {context}: {err}"),
        )
    }
}

impl From<sentinel_scanner::drives::DiscoveryError> for CommandError {
    fn from(err: sentinel_scanner::drives::DiscoveryError) -> Self {
        Self::new(ErrorKind::System, err.to_string())
    }
}
