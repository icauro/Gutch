//! Recoverable directory replacement shared by repository and installation publication.
use super::{create_dir_all, read, write};
use crate::Error;
use crate::error::IoContext;
use crate::error::promotion_diagnostics::{
    DirectoryPromotionDiagnostic, PromotionPhase, PromotionRecoveryAttempt,
    PromotionRecoveryOperation,
};
use crate::progress::OperationControl;
use crate::progress::OperationWarning;
use std::fs;
use std::fs::File;
use std::fs::OpenOptions;
use std::fs::TryLockError;
use std::path::Path;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;
use std::time::Instant;

/// Marks a promotion backup as Gutch's own.
const BACKUP_OWNER_MARKER: &[u8] = b"GUTCH_PROMOTION_V1";
/// How often a blocked promotion lock checks for cancellation.
const LOCK_POLL_INTERVAL: Duration = Duration::from_millis(25);

fn invalid_path(path: &Path, reason: &str) -> Error {
    Error::InvalidPath {
        path: path.to_path_buf(),
        reason: reason.to_owned(),
    }
}

fn is_owner_marker(owner_path: &Path) -> bool {
    read(owner_path).is_ok_and(|owner| owner == BACKUP_OWNER_MARKER)
}
/// Publishes `staging_dir` as `destination_dir`. A cleanup failure after the
/// switch commits is reported to `control` as a warning, not as an error.
pub(crate) fn replace_directory(
    staging_dir: &Path,
    destination_dir: &Path,
    control: &OperationControl,
) -> Result<(), Error> {
    replace_directory_with_cleanup(staging_dir, destination_dir, control, |backup| {
        fs::remove_dir_all(backup)
    })
}

// Keep the cleanup boundary injectable to exercise failures after the rename commits.
fn replace_directory_with_cleanup(
    staging_dir: &Path,
    destination_dir: &Path,
    control: &OperationControl,
    remove_backup: impl FnOnce(&Path) -> std::io::Result<()>,
) -> Result<(), Error> {
    let warning = replace_directory_with_operations(
        staging_dir,
        destination_dir,
        |from, to| fs::rename(from, to),
        |staging| fs::remove_dir_all(staging),
        remove_backup,
    )?;
    if let Some(diagnostic) = warning {
        control.warn(OperationWarning::DirectoryCleanupFailed(Box::new(
            diagnostic,
        )));
    }
    Ok(())
}

// Tests inject native I/O failures at the same boundaries as the real operation.
fn replace_directory_with_operations(
    staging_dir: &Path,
    destination_dir: &Path,
    mut rename: impl FnMut(&Path, &Path) -> std::io::Result<()>,
    remove_staging: impl FnOnce(&Path) -> std::io::Result<()>,
    remove_backup: impl FnOnce(&Path) -> std::io::Result<()>,
) -> Result<Option<DirectoryPromotionDiagnostic>, Error> {
    let started = Instant::now();
    let parent = destination_dir
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let backup_path = promotion_backup_path(destination_dir)?;
    create_dir_all(parent).map_err(|error| {
        attach_promotion_diagnostic(
            error,
            parent,
            PromotionPhase::PrepareDestinationParent,
            started,
            staging_dir,
            destination_dir,
            &backup_path,
        )
    })?;

    recover_directory_promotion(destination_dir).map_err(|error| {
        attach_promotion_diagnostic(
            error,
            destination_dir,
            PromotionPhase::RecoverExistingBackup,
            started,
            staging_dir,
            destination_dir,
            &backup_path,
        )
    })?;
    let backup_dir = if destination_dir.exists() {
        if backup_path.exists() {
            // Recovery just cleared it, so another process is promoting here.
            return Err(invalid_path(
                &backup_path,
                "a promotion backup appeared during promotion; another process may be writing this destination",
            ));
        }
        let owner_path = backup_path.with_extension("owner");
        write(&owner_path, BACKUP_OWNER_MARKER).map_err(|error| {
            attach_promotion_diagnostic(
                error,
                &owner_path,
                PromotionPhase::WriteBackupOwner,
                started,
                staging_dir,
                destination_dir,
                &backup_path,
            )
        })?;
        rename(destination_dir, &backup_path).map_err(|error| {
            // Nothing moved, so the marker would only vouch for a future stranger.
            let _ = fs::remove_file(&owner_path);
            let diagnostic = DirectoryPromotionDiagnostic::capture(
                PromotionPhase::MoveDestinationToBackup,
                started,
                &error,
                staging_dir,
                destination_dir,
                &backup_path,
            );
            promotion_error(
                format!(
                    "moving existing destination to backup {}",
                    backup_path.display()
                ),
                destination_dir,
                error,
                diagnostic,
            )
        })?;
        Some(&backup_path)
    } else {
        None
    };

    if let Err(error) = rename(staging_dir, destination_dir) {
        // Observe the failing boundary before rollback or cleanup alters any path.
        let mut diagnostic = DirectoryPromotionDiagnostic::capture(
            PromotionPhase::PromoteStaging,
            started,
            &error,
            staging_dir,
            destination_dir,
            &backup_path,
        );
        if let Some(backup) = &backup_dir {
            let restored = rename(backup, destination_dir);
            if restored.is_ok() {
                let _ = fs::remove_file(backup.with_extension("owner"));
            }
            diagnostic.recovery.push(PromotionRecoveryAttempt::capture(
                PromotionRecoveryOperation::RestoreBackup,
                restored,
            ));
        }
        diagnostic.recovery.push(PromotionRecoveryAttempt::capture(
            PromotionRecoveryOperation::RemoveStaging,
            remove_staging(staging_dir),
        ));
        return Err(promotion_error(
            format!("promoting staging directory {}", staging_dir.display()),
            destination_dir,
            error,
            diagnostic,
        ));
    }

    if let Some(backup) = backup_dir {
        // Promotion has committed. Keep the owner marker if cleanup fails so
        // the next promotion can retry without reporting this install as failed.
        match remove_backup(backup) {
            Ok(()) => {
                if let Err(error) = fs::remove_file(backup.with_extension("owner")) {
                    let diagnostic = DirectoryPromotionDiagnostic::capture(
                        PromotionPhase::RemoveBackupMarker,
                        started,
                        &error,
                        staging_dir,
                        destination_dir,
                        backup,
                    );
                    return Ok(Some(diagnostic));
                }
            }
            Err(error) => {
                let diagnostic = DirectoryPromotionDiagnostic::capture(
                    PromotionPhase::RemoveBackup,
                    started,
                    &error,
                    staging_dir,
                    destination_dir,
                    backup,
                );
                return Ok(Some(diagnostic));
            }
        }
    }
    Ok(None)
}

fn promotion_error(
    operation: String,
    path: &Path,
    source: std::io::Error,
    diagnostic: DirectoryPromotionDiagnostic,
) -> Error {
    Error::IoContext {
        operation,
        path: path.to_path_buf(),
        source,
        promotion: Some(Box::new(diagnostic)),
    }
}

fn attach_promotion_diagnostic(
    error: Error,
    context: &Path,
    phase: PromotionPhase,
    started: Instant,
    staging: &Path,
    destination: &Path,
    backup: &Path,
) -> Error {
    let (operation, path, source) = match error {
        Error::Io(source) => (
            phase.description().to_owned(),
            context.to_path_buf(),
            source,
        ),
        Error::IoContext {
            operation,
            path,
            source,
            ..
        } => (operation, path, source),
        // Invalid ownership and other typed contract failures are not native I/O errors.
        other => return other,
    };
    let diagnostic = DirectoryPromotionDiagnostic::capture(
        phase,
        started,
        &source,
        staging,
        destination,
        backup,
    );
    promotion_error(operation, &path, source, diagnostic)
}

/// Held across staging and promotion, after the operation's path checks.
pub(crate) fn lock_directory_promotion(
    destination: &Path,
    control: &OperationControl,
) -> Result<File, Error> {
    let backup = promotion_backup_path(destination)?;
    let lock_path = backup.with_extension("lock");
    if let Some(parent) = lock_path.parent() {
        create_dir_all(parent)?;
    }
    if fs::symlink_metadata(&lock_path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(invalid_path(
            &lock_path,
            "promotion lock must not be a symbolic link",
        ));
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .io_context("opening promotion lock", &lock_path)?;
    loop {
        control.checkpoint()?;
        match file.try_lock() {
            Ok(()) => break,
            Err(TryLockError::WouldBlock) => thread::sleep(LOCK_POLL_INTERVAL),
            Err(TryLockError::Error(error)) => {
                return Err(Error::io_context(
                    "locking directory promotion",
                    &lock_path,
                    error,
                ));
            }
        }
    }
    Ok(file)
}

fn promotion_backup_path(destination: &Path) -> Result<PathBuf, Error> {
    let parent = destination
        .parent()
        .ok_or_else(|| invalid_path(destination, "destination has no parent directory"))?;
    let name = destination
        .file_name()
        .ok_or_else(|| invalid_path(destination, "destination has no file name"))?;
    #[cfg(windows)]
    let name = name.to_string_lossy().to_lowercase();
    #[cfg(windows)]
    let hash = blake3::hash(name.as_bytes()).to_hex();
    #[cfg(not(windows))]
    let hash = blake3::hash(name.as_encoded_bytes()).to_hex();
    Ok(parent.join(format!(".gutch-backup-{hash}")))
}

/// Only owned promotion backups may be excluded from repository inventories.
pub(crate) fn is_owned_promotion_backup(path: &Path) -> bool {
    let Some(hash) = path
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix(".gutch-backup-"))
    else {
        return false;
    };
    hash.len() == 64
        && hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        && is_owner_marker(&path.with_extension("owner"))
}

/// A deterministic same-parent backup makes an interrupted promotion recoverable.
/// Existing destination means promotion completed; absence means restore the backup.
pub(crate) fn recover_directory_promotion(destination: &Path) -> Result<(), Error> {
    let backup = promotion_backup_path(destination)?;
    let owner_path = backup.with_extension("owner");
    if !backup.exists() {
        // An orphaned marker would make a later unowned backup look owned.
        // Only a marker file is Gutch's; anything else is left for the owner write to report.
        if fs::symlink_metadata(&owner_path).is_ok_and(|metadata| metadata.is_file()) {
            fs::remove_file(&owner_path).map_err(|error| {
                Error::io_context("removing orphaned promotion marker", &owner_path, error)
            })?;
        }
        return Ok(());
    }
    // A missing marker means Gutch did not create this backup; report it as unowned.
    if !is_owner_marker(&owner_path) {
        return Err(invalid_path(
            &backup,
            "a promotion backup exists that Gutch did not create; move or remove it",
        ));
    }
    let metadata =
        fs::symlink_metadata(&backup).io_context("inspecting promotion backup", &backup)?;
    if metadata.file_type().is_symlink() {
        return Err(invalid_path(
            &backup,
            "promotion backup must not be a symbolic link",
        ));
    }
    if destination.exists() {
        fs::remove_dir_all(&backup).io_context("removing recovered promotion backup", &backup)?;
    } else {
        fs::rename(&backup, destination).map_err(|error| {
            Error::io_context(
                format!("recovering backup {}", backup.display()),
                destination,
                error,
            )
        })?;
    }
    fs::remove_file(&owner_path).io_context("removing promotion marker", &owner_path)?;
    Ok(())
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::progress::WarningSink;
    use std::sync::Arc;
    use std::sync::Mutex;

    #[derive(Debug, Default)]
    struct RecordedWarnings(Mutex<Vec<String>>);

    impl WarningSink for RecordedWarnings {
        fn warn(&self, warning: OperationWarning) {
            self.0.lock().unwrap().push(warning.to_string());
        }
    }

    #[test]
    fn committed_replacement_survives_backup_cleanup_failure_and_retries_cleanup() {
        use std::os::windows::fs::OpenOptionsExt;
        let root = tempfile::tempdir().unwrap();
        let snapshot_id = "a".repeat(64);
        let destination = root.path().join("snapshots").join(&snapshot_id);
        let staging = root.path().join("staging");
        std::fs::create_dir_all(&destination).unwrap();
        std::fs::create_dir(&staging).unwrap();
        let old = destination.join("old");
        std::fs::write(&old, b"old installation").unwrap();
        std::fs::write(staging.join("new"), b"new installation").unwrap();
        std::fs::write(destination.join("snapshot_metadata.json"), b"{}").unwrap();
        std::fs::write(staging.join("snapshot_metadata.json"), b"{}").unwrap();

        let warnings = Arc::new(RecordedWarnings::default());
        let control = OperationControl::default().with_warnings(warnings.clone());
        let result = replace_directory_with_cleanup(&staging, &destination, &control, |backup| {
            let _lock = std::fs::OpenOptions::new()
                .read(true)
                .share_mode(1 | 2)
                .open(backup.join("old"))?;
            let cleanup = std::fs::remove_dir_all(backup);
            assert!(cleanup.is_err());
            cleanup
        });
        let backup = promotion_backup_path(&destination).unwrap();
        result.unwrap();
        assert_eq!(
            *warnings.0.lock().unwrap(),
            ["directory promotion committed; cleanup failed"]
        );
        assert_eq!(
            std::fs::read(destination.join("new")).unwrap(),
            b"new installation"
        );
        assert!(backup.exists());
        assert!(backup.with_extension("owner").exists());
        assert!(is_owned_promotion_backup(&backup));
        recover_directory_promotion(&destination).unwrap();
        assert!(!backup.exists());
        assert!(!backup.with_extension("owner").exists());
        assert_eq!(
            std::fs::read(destination.join("new")).unwrap(),
            b"new installation"
        );
    }
}

#[cfg(test)]
mod diagnostic_tests {
    use super::*;
    use crate::error::promotion_diagnostics::PromotionPathState;

    #[test]
    fn directory_replacement_promotes_staged_contents_and_removes_backup() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("destination");
        let staging = directory.path().join("staging");
        std::fs::create_dir_all(&destination).unwrap();
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(destination.join("old"), b"old").unwrap();
        std::fs::write(staging.join("new"), b"new").unwrap();

        replace_directory(&staging, &destination, &OperationControl::default()).unwrap();

        assert!(!destination.join("old").exists());
        assert_eq!(std::fs::read(destination.join("new")).unwrap(), b"new");
        assert_eq!(
            std::fs::read_dir(directory.path())
                .unwrap()
                .filter_map(|entry| entry.ok())
                .filter(|entry| entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".gutch-backup-"))
                .count(),
            0
        );
    }

    #[test]
    fn backup_owner_write_failure_keeps_the_destination_and_reports_its_phase() {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("destination");
        let staging = root.path().join("staging");
        std::fs::create_dir(&destination).unwrap();
        std::fs::create_dir(&staging).unwrap();
        std::fs::write(destination.join("old"), b"original").unwrap();
        let backup = promotion_backup_path(&destination).unwrap();
        std::fs::create_dir(backup.with_extension("owner")).unwrap();
        let error =
            replace_directory(&staging, &destination, &OperationControl::default()).unwrap_err();
        let report = error.diagnostic_report();
        assert_eq!(report["code"], "directory_promotion_failed");
        assert_eq!(report["details"]["phase"], "write_backup_owner");
        assert_eq!(report["details"]["destination"]["state"], "present");
        assert_eq!(report["details"]["staging"]["state"], "present");
        assert_eq!(report["details"]["backup"]["state"], "missing");
        assert_eq!(std::fs::read(destination.join("old")).unwrap(), b"original");
    }

    #[test]
    fn promotion_failure_names_both_paths_and_restores_existing_destination() {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("destination");
        std::fs::create_dir(&destination).unwrap();
        std::fs::write(destination.join("old"), b"original").unwrap();
        let staging = root.path().join("missing-staging");
        let error =
            replace_directory(&staging, &destination, &OperationControl::default()).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("promoting staging directory"));
        assert!(message.contains(&staging.display().to_string()));
        assert!(message.contains(&destination.display().to_string()));
        assert_eq!(std::fs::read(destination.join("old")).unwrap(), b"original");
        assert!(matches!(error, Error::IoContext { .. }));
        let report = error.diagnostic_report();
        assert_eq!(report["code"], "directory_promotion_failed");
        let details = &report["details"];
        assert_eq!(details["phase"], "promote_staging");
        assert_eq!(details["staging"]["state"], "missing");
        assert_eq!(details["destination"]["state"], "missing");
        assert_eq!(details["backup"]["state"], "present");
        assert_eq!(details["recovery"][0]["operation"], "restore_backup");
        assert_eq!(details["recovery"][0]["succeeded"], true);
        assert_eq!(details["recovery"][1]["operation"], "remove_staging");
        assert_eq!(details["recovery"][1]["succeeded"], false);
        assert!(details["observed_at"].as_str().unwrap().ends_with('Z'));
        let backup = promotion_backup_path(&destination).unwrap();
        assert!(!backup.with_extension("owner").exists());
    }

    #[test]
    fn recovery_removes_an_orphaned_owner_marker() {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("destination");
        let owner = promotion_backup_path(&destination)
            .unwrap()
            .with_extension("owner");
        std::fs::write(&owner, b"GUTCH_PROMOTION_V1").unwrap();
        recover_directory_promotion(&destination).unwrap();
        assert!(!owner.exists());
    }

    #[test]
    fn promotion_retains_primary_rollback_and_cleanup_failures_and_original_state() {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("destination");
        let staging = root.path().join("staging");
        std::fs::create_dir(&destination).unwrap();
        std::fs::create_dir(&staging).unwrap();
        std::fs::write(destination.join("old"), b"original").unwrap();
        std::fs::write(staging.join("new"), b"replacement").unwrap();
        let backup = promotion_backup_path(&destination).unwrap();
        let error = replace_directory_with_operations(
            &staging,
            &destination,
            |from, to| {
                if from == staging {
                    Err(std::io::Error::from_raw_os_error(5))
                } else if from == backup {
                    Err(std::io::Error::from_raw_os_error(13))
                } else {
                    std::fs::rename(from, to)
                }
            },
            |_| Err(std::io::Error::from_raw_os_error(32)),
            |backup| std::fs::remove_dir_all(backup),
        )
        .unwrap_err();
        let Error::IoContext {
            source,
            promotion: Some(details),
            ..
        } = error
        else {
            panic!("expected native promotion diagnostics");
        };
        assert_eq!(source.raw_os_error(), Some(5));
        assert_eq!(details.primary_error.raw_os_error, Some(5));
        assert!(matches!(
            details.staging.state,
            PromotionPathState::Present {
                read_only: false,
                ..
            }
        ));
        assert!(matches!(
            details.destination.state,
            PromotionPathState::Missing
        ));
        assert!(matches!(
            details.backup.state,
            PromotionPathState::Present { .. }
        ));
        assert_eq!(
            details.recovery[0].error.as_ref().unwrap().raw_os_error,
            Some(13)
        );
        assert_eq!(
            details.recovery[1].error.as_ref().unwrap().raw_os_error,
            Some(32)
        );
        assert_eq!(std::fs::read(backup.join("old")).unwrap(), b"original");
        assert!(is_owned_promotion_backup(&backup));
        // The existing recovery policy still restores this retained backup.
        recover_directory_promotion(&destination).unwrap();
        assert_eq!(std::fs::read(destination.join("old")).unwrap(), b"original");
    }

    #[test]
    fn committed_backup_cleanup_failure_returns_a_warning_without_rollback() {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("destination");
        let staging = root.path().join("staging");
        std::fs::create_dir(&destination).unwrap();
        std::fs::create_dir(&staging).unwrap();
        std::fs::write(destination.join("old"), b"original").unwrap();
        std::fs::write(staging.join("new"), b"replacement").unwrap();
        let warning = replace_directory_with_operations(
            &staging,
            &destination,
            |from, to| std::fs::rename(from, to),
            |staging| std::fs::remove_dir_all(staging),
            |_| Err(std::io::Error::from_raw_os_error(5)),
        )
        .unwrap()
        .unwrap();
        assert!(warning.committed);
        assert_eq!(warning.phase, PromotionPhase::RemoveBackup);
        assert_eq!(warning.primary_error.raw_os_error, Some(5));
        assert!(matches!(
            warning.destination.state,
            PromotionPathState::Present { .. }
        ));
        assert!(matches!(
            warning.backup.state,
            PromotionPathState::Present { .. }
        ));
        assert!(warning.recovery.is_empty());
        assert_eq!(
            std::fs::read(destination.join("new")).unwrap(),
            b"replacement"
        );
        recover_directory_promotion(&destination).unwrap();
    }
}
