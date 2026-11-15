//! Failure-time observations for recoverable directory publication.
use serde::Serialize;
use std::path::Path;
use std::time::Instant;

#[derive(Debug, Serialize)]
pub struct DiagnosticIoError {
    pub kind: String,
    pub raw_os_error: Option<i32>,
    pub message: String,
}

impl DiagnosticIoError {
    pub(crate) fn capture(error: &std::io::Error) -> Self {
        Self {
            kind: format!("{:?}", error.kind()),
            raw_os_error: error.raw_os_error(),
            message: error.to_string(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct PromotionPathObservation {
    pub path: String,
    pub exists: Option<bool>,
    pub read_only: Option<bool>,
    pub is_directory: Option<bool>,
    pub inspection_error: Option<DiagnosticIoError>,
}

impl PromotionPathObservation {
    pub(crate) fn capture(path: &Path) -> Self {
        Self::from_metadata(path, std::fs::symlink_metadata(path))
    }

    fn from_metadata(path: &Path, metadata: std::io::Result<std::fs::Metadata>) -> Self {
        let mut observation = Self {
            // Diagnostic paths remain serializable even when the native path is not UTF-8.
            path: path.to_string_lossy().into_owned(),
            exists: None,
            read_only: None,
            is_directory: None,
            inspection_error: None,
        };
        match metadata {
            Ok(metadata) => {
                observation.exists = Some(true);
                observation.read_only = Some(metadata.permissions().readonly());
                observation.is_directory = Some(metadata.is_dir());
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                observation.exists = Some(false);
            }
            Err(error) => observation.inspection_error = Some(DiagnosticIoError::capture(&error)),
        }
        observation
    }
}

#[derive(Debug, Serialize)]
pub struct PromotionRecoveryAttempt {
    pub operation: &'static str,
    pub succeeded: bool,
    pub error: Option<DiagnosticIoError>,
}

impl PromotionRecoveryAttempt {
    pub(crate) fn capture(operation: &'static str, result: std::io::Result<()>) -> Self {
        Self {
            operation,
            succeeded: result.is_ok(),
            error: result.as_ref().err().map(DiagnosticIoError::capture),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct DirectoryPromotionDiagnostic {
    pub platform: &'static str,
    pub observed_at: String,
    pub phase: &'static str,
    pub elapsed_ms: u128,
    pub committed: bool,
    pub primary_error: DiagnosticIoError,
    pub staging: PromotionPathObservation,
    pub destination: PromotionPathObservation,
    pub backup: PromotionPathObservation,
    pub recovery: Vec<PromotionRecoveryAttempt>,
}

impl DirectoryPromotionDiagnostic {
    pub(crate) fn capture(
        phase: &'static str,
        started: Instant,
        error: &std::io::Error,
        staging: &Path,
        destination: &Path,
        backup: &Path,
    ) -> Self {
        Self {
            platform: std::env::consts::OS,
            observed_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            phase,
            elapsed_ms: started.elapsed().as_millis(),
            committed: false,
            primary_error: DiagnosticIoError::capture(error),
            staging: PromotionPathObservation::capture(staging),
            destination: PromotionPathObservation::capture(destination),
            backup: PromotionPathObservation::capture(backup),
            recovery: Vec::new(),
        }
    }

    pub(crate) fn warn_committed_cleanup(&self) {
        use std::io::Write;
        // Reporting must never turn a committed publication into an error, including
        // when the caller's stderr is closed. Routine successful publication is silent.
        let mut stderr = std::io::stderr().lock();
        let _ = writeln!(
            stderr,
            "Warning: directory promotion committed; cleanup failed"
        );
        let _ = serde_json::to_writer(&mut stderr, self);
        let _ = writeln!(stderr);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observation_records_read_only_without_changing_permissions() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("read-only");
        std::fs::write(&path, b"content").unwrap();
        let original = std::fs::metadata(&path).unwrap().permissions();
        let mut permissions = original.clone();
        permissions.set_readonly(true);
        std::fs::set_permissions(&path, permissions).unwrap();
        let observation = PromotionPathObservation::capture(&path);
        assert_eq!(observation.exists, Some(true));
        assert_eq!(observation.read_only, Some(true));
        assert!(std::fs::metadata(&path).unwrap().permissions().readonly());
        std::fs::set_permissions(&path, original).unwrap();
    }

    #[test]
    fn denied_metadata_is_unknown_and_retains_the_inspection_error() {
        let observation = PromotionPathObservation::from_metadata(
            Path::new("restricted"),
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "denied",
            )),
        );
        assert_eq!(observation.exists, None);
        assert_eq!(observation.read_only, None);
        assert_eq!(
            observation.inspection_error.unwrap().kind,
            "PermissionDenied"
        );
    }
}
