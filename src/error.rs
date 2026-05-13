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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display_bundle() {
        let e = Error::bundle("corrupt data");
        assert_eq!(e.to_string(), "Bundle error: corrupt data");
    }

    #[test]
    fn error_display_manifest() {
        let e = Error::manifest("missing field");
        assert_eq!(e.to_string(), "Manifest error: missing field");
    }

    #[test]
    fn error_display_verify() {
        let e = Error::verify("hash mismatch");
        assert_eq!(e.to_string(), "Verification error: hash mismatch");
    }

    #[test]
    fn error_display_reconstruct() {
        let e = Error::reconstruct("output dir missing");
        assert_eq!(e.to_string(), "Reconstruction error: output dir missing");
    }

    #[test]
    fn error_display_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let e = Error::from(io_err);
        assert!(e.to_string().contains("I/O error:"));
    }

    #[test]
    fn error_display_hash_mismatch() {
        let e = Error::HashMismatch {
            expected: "abc".into(),
            actual: "def".into(),
        };
        assert_eq!(e.to_string(), "Hash mismatch: expected abc, got def");
    }

    #[test]
    fn error_from_str() {
        let e = Error::from("test error");
        assert_eq!(e.to_string(), "Bundle error: test error");
    }

    #[test]
    fn error_from_string() {
        let e = Error::from(String::from("owned error"));
        assert_eq!(e.to_string(), "Bundle error: owned error");
    }

    #[test]
    fn error_display_chunk_not_found() {
        let e = Error::ChunkNotFound("abc123".into());
        assert_eq!(
            e.to_string(),
            "Chunk not found in any bundle: abc123"
        );
    }

    #[test]
    fn error_display_path_error() {
        let e = Error::PathError {
            path: PathBuf::from("/bad/path"),
            reason: "permission denied".into(),
        };
        assert!(e.to_string().contains("/bad/path"));
        assert!(e.to_string().contains("permission denied"));
    }

    #[test]
    fn error_is_debug() {
        let e = Error::bundle("debug test");
        let debug = format!("{e:?}");
        assert!(debug.contains("Bundle"));
        assert!(debug.contains("debug test"));
    }
}
