//! Engine failures shared by operations and embedding hosts.

pub(crate) mod metadata_conflict;
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

    #[error("Walkdir error: {0}")]
    Walkdir(#[from] walkdir::Error),

    /// The command line did not parse; the text is the parser's complete,
    /// already formatted message including usage hints.
    #[error("{}", .0.trim_end())]
    Usage(String),

    /// Caller-supplied arguments, configuration or source selection is unusable.
    #[error("Invalid input: {0}")]
    InvalidInput(String),

    /// Stored or received artifact bytes are malformed, unsupported or inconsistent.
    #[error("Invalid artifact: {0}")]
    Artifact(String),

    /// Snapshot manifest, blueprint or tag data is malformed or inconsistent.
    #[error("Manifest error: {0}")]
    Manifest(String),

    /// Repository layout, metadata, coordination or synchronization failed.
    #[error("Repository error: {0}")]
    Repository(String),

    /// Source content could not be captured as a snapshot.
    #[error("Capture error: {0}")]
    Capture(String),

    /// The source tree changed while capture read it; a fresh scan may succeed.
    #[error(
        "Capture error: source changed during capture{}",
        path.as_ref().map(|path| format!(": {}", path.display())).unwrap_or_default()
    )]
    SourceChanged { path: Option<PathBuf> },

    #[error("Download error: {message}")]
    Download { message: String, retryable: bool },

    #[error("Verification error: {0}")]
    Verify(String),

    #[error(
        "Stored snapshot data does not match its content-derived ID; the repository may be corrupt: {snapshot_id}"
    )]
    SnapshotIntegrityMismatch { snapshot_id: String },

    /// Capture produced an existing snapshot ID with different descriptive metadata.
    #[error(
        "snapshot_id: {snapshot_id}{}\nCapture did not change the existing snapshot. Use an explicit metadata update for this snapshot ID.",
        metadata_conflict::MetadataDifferences(differences)
    )]
    SnapshotMetadataConflict {
        snapshot_id: String,
        differences: Vec<metadata_conflict::MetadataFieldDifference>,
    },

    #[error("Reconstruction error: {0}")]
    Reconstruct(String),

    #[error("Hash mismatch: expected {expected}, got {actual}")]
    HashMismatch { expected: String, actual: String },

    #[error("Invalid path {path}: {reason}")]
    InvalidPath { path: PathBuf, reason: String },
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

    /// The conventional process exit code: 2 for command-line usage errors,
    /// 1 for everything else.
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Usage(_) => 2,
            _ => 1,
        }
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

    pub fn invalid_input(message: impl Into<String>) -> Self {
        Error::InvalidInput(message.into())
    }

    pub fn artifact(message: impl Into<String>) -> Self {
        Error::Artifact(message.into())
    }

    pub fn manifest(message: impl Into<String>) -> Self {
        Error::Manifest(message.into())
    }

    pub fn repository(message: impl Into<String>) -> Self {
        Error::Repository(message.into())
    }

    pub fn capture(message: impl Into<String>) -> Self {
        Error::Capture(message.into())
    }

    pub fn download(msg: impl Into<String>, retryable: bool) -> Self {
        Error::Download {
            message: msg.into(),
            retryable,
        }
    }

    /// True only for download failures the transfer layer marked retryable.
    /// Local I/O failures are never retried.
    pub fn is_retryable_download(&self) -> bool {
        matches!(
            self,
            Error::Download {
                retryable: true,
                ..
            }
        )
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
