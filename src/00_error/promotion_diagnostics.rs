//! Failure-time observations for recoverable directory publication.
use serde::Serialize;
use std::env::consts::OS;
use std::fs;
use std::fs::Metadata;
use std::io;
use std::path::Path;
use std::time::Instant;

#[derive(Debug, Serialize)]
pub struct DiagnosticIoError {
    pub kind: String,
    pub raw_os_error: Option<i32>,
    pub message: String,
}

impl DiagnosticIoError {
    pub(crate) fn capture(error: &io::Error) -> Self {
        Self {
            kind: format!("{:?}", error.kind()),
            raw_os_error: error.raw_os_error(),
            message: error.to_string(),
        }
    }
}

/// Step of a directory promotion. The last two run after the switch commits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PromotionPhase {
    PrepareDestinationParent,
    RecoverExistingBackup,
    WriteBackupOwner,
    MoveDestinationToBackup,
    PromoteStaging,
    RemoveBackupMarker,
    RemoveBackup,
}

impl PromotionPhase {
    /// True once the new directory is visible at the destination.
    pub fn is_committed(self) -> bool {
        matches!(self, Self::RemoveBackupMarker | Self::RemoveBackup)
    }

    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::PrepareDestinationParent => "preparing destination parent",
            Self::RecoverExistingBackup => "recovering existing backup",
            Self::WriteBackupOwner => "writing backup owner",
            Self::MoveDestinationToBackup => "moving destination to backup",
            Self::PromoteStaging => "promoting staging",
            Self::RemoveBackupMarker => "removing backup marker",
            Self::RemoveBackup => "removing backup",
        }
    }
}

/// Rollback step attempted after a failed promotion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PromotionRecoveryOperation {
    RestoreBackup,
    RemoveStaging,
}

/// What metadata inspection of a path found.
#[derive(Debug, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum PromotionPathState {
    Missing,
    Present {
        read_only: bool,
        is_directory: bool,
    },
    /// Inspection failed, so the path's existence is unknown.
    Unknown {
        inspection_error: DiagnosticIoError,
    },
}

#[derive(Debug, Serialize)]
pub struct PromotionPathObservation {
    pub path: String,
    #[serde(flatten)]
    pub state: PromotionPathState,
}

impl PromotionPathObservation {
    pub(crate) fn capture(path: &Path) -> Self {
        Self::from_metadata(path, fs::symlink_metadata(path))
    }

    fn from_metadata(path: &Path, metadata: io::Result<Metadata>) -> Self {
        let state = match metadata {
            Ok(metadata) => PromotionPathState::Present {
                read_only: metadata.permissions().readonly(),
                is_directory: metadata.is_dir(),
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => PromotionPathState::Missing,
            Err(error) => PromotionPathState::Unknown {
                inspection_error: DiagnosticIoError::capture(&error),
            },
        };
        Self {
            // Diagnostic paths remain serializable even when the native path is not UTF-8.
            path: path.to_string_lossy().into_owned(),
            state,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct PromotionRecoveryAttempt {
    pub operation: PromotionRecoveryOperation,
    pub succeeded: bool,
    pub error: Option<DiagnosticIoError>,
}

impl PromotionRecoveryAttempt {
    pub(crate) fn capture(operation: PromotionRecoveryOperation, result: io::Result<()>) -> Self {
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
    pub phase: PromotionPhase,
    pub elapsed_ms: u128,
    /// Derived from `phase`; kept in the report for readers of the JSON.
    pub committed: bool,
    pub primary_error: DiagnosticIoError,
    pub staging: PromotionPathObservation,
    pub destination: PromotionPathObservation,
    pub backup: PromotionPathObservation,
    pub recovery: Vec<PromotionRecoveryAttempt>,
}

impl DirectoryPromotionDiagnostic {
    pub(crate) fn capture(
        phase: PromotionPhase,
        started: Instant,
        error: &io::Error,
        staging: &Path,
        destination: &Path,
        backup: &Path,
    ) -> Self {
        Self {
            platform: OS,
            observed_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            phase,
            elapsed_ms: started.elapsed().as_millis(),
            committed: phase.is_committed(),
            primary_error: DiagnosticIoError::capture(error),
            staging: PromotionPathObservation::capture(staging),
            destination: PromotionPathObservation::capture(destination),
            backup: PromotionPathObservation::capture(backup),
            recovery: Vec::new(),
        }
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
        assert!(matches!(
            observation.state,
            PromotionPathState::Present {
                read_only: true,
                is_directory: false
            }
        ));
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
        let PromotionPathState::Unknown { inspection_error } = observation.state else {
            panic!("denied metadata must be unknown");
        };
        assert_eq!(inspection_error.kind, "PermissionDenied");
    }
}
