use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

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

    #[error("Verification error: {0}")]
    Verify(String),

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
    pub fn bundle(msg: impl Into<String>) -> Self {
        Error::Bundle(msg.into())
    }

    pub fn manifest(msg: impl Into<String>) -> Self {
        Error::Manifest(msg.into())
    }

    pub fn verify(msg: impl Into<String>) -> Self {
        Error::Verify(msg.into())
    }

    pub fn reconstruct(msg: impl Into<String>) -> Self {
        Error::Reconstruct(msg.into())
    }
}