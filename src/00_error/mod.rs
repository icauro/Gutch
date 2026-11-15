//! Engine failures shared by operations and embedding hosts.

pub(crate) mod capture_outcome;
pub(crate) mod path_validation;
pub(crate) mod promotion_diagnostics;

use std::path::Path;
use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    PathValidation(path_validation::PathValidationFailure),

    #[error("Operation cancelled")]
    Cancelled,

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("I/O error while {operation} at '{path}': {source}")]
    IoContext {
        operation: String,
        path: PathBuf,
        #[source]
        source: std::io::Error,
        promotion: Option<Box<promotion_diagnostics::DirectoryPromotionDiagnostic>>,
    },

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Zstd error: {0}")]
    Zstd(String),

    #[error("Walkdir error: {0}")]
    Walkdir(#[from] walkdir::Error),

    #[error("Bundle error: {0}")]
    Bundle(String),

    #[error("Manifest error: {0}")]
    Manifest(String),

    #[error("Download error: {message}")]
    Download { message: String, retryable: bool },

    #[error("Verification error: {0}")]
    Verify(String),

    #[error("Source snapshot destination did not verify: {0}")]
    SourceSnapshotNotVerified(String),

    #[error("Stored snapshot data does not match its content-derived ID: {snapshot_id}")]
    SnapshotIntegrityMismatch { snapshot_id: String },

    #[error("{outcome}")]
    SnapshotCaptureConflict {
        outcome: capture_outcome::SnapshotCaptureOutcome,
    },

    #[error("Reconstruction error: {0}")]
    Reconstruct(String),

    #[error("Chunk error: {0}")]
    Chunk(String),

    #[error("Invalid bundle: {0}")]
    InvalidBundle(String),

    #[error("Missing chunk: {0}")]
    MissingChunk(String),

    #[error("Hash mismatch: expected {expected}, got {actual}")]
    HashMismatch { expected: String, actual: String },

    #[error("Chunk not found in any bundle: {0}")]
    ChunkNotFound(String),

    #[error("Path error for {path}: {reason}")]
    PathError { path: PathBuf, reason: String },
}

impl From<&str> for Error {
    fn from(s: &str) -> Self {
        Error::Bundle(s.to_string())
    }
}

impl From<String> for Error {
    fn from(s: String) -> Self {
        Error::Bundle(s)
    }
}

impl From<zstd::zstd_safe::ErrorCode> for Error {
    fn from(code: zstd::zstd_safe::ErrorCode) -> Self {
        Error::Zstd(format!("zstd error code: {:?}", code))
    }
}

impl Error {
    pub(crate) fn io_context(
        operation: impl Into<String>,
        path: &Path,
        source: std::io::Error,
    ) -> Self {
        Self::IoContext {
            operation: operation.into(),
            path: path.to_path_buf(),
            source,
            promotion: None,
        }
    }

    /// Attach the owning operation without hiding typed integrity or path errors.
    pub(crate) fn with_io_context(self, operation: &str, path: &Path) -> Self {
        match self {
            Self::Io(source) => Self::io_context(operation, path, source),
            other => other,
        }
    }

    pub fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
    }

    pub(crate) fn diagnostic_report(&self) -> serde_json::Value {
        match self {
            Self::PathValidation(failure) => serde_json::json!({
                "code": "filename_constraints", "message": self.to_string(), "details": failure,
            }),
            Self::IoContext {
                promotion: Some(details),
                ..
            } => serde_json::json!({
                "code": "directory_promotion_failed", "message": self.to_string(), "details": details,
            }),
            _ => serde_json::json!({"code": "operation_failed", "message": self.to_string()}),
        }
    }

    pub fn bundle(msg: impl Into<String>) -> Self {
        Error::Bundle(msg.into())
    }

    pub fn manifest(msg: impl Into<String>) -> Self {
        Error::Manifest(msg.into())
    }

    pub fn download(msg: impl Into<String>, retryable: bool) -> Self {
        Error::Download {
            message: msg.into(),
            retryable,
        }
    }

    pub fn is_retryable_download(&self) -> bool {
        match self {
            Error::Download { retryable, .. } => *retryable,
            Error::Io(_) | Error::IoContext { .. } => true,
            _ => false,
        }
    }

    pub fn verify(msg: impl Into<String>) -> Self {
        Error::Verify(msg.into())
    }

    pub fn reconstruct(msg: impl Into<String>) -> Self {
        Error::Reconstruct(msg.into())
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
