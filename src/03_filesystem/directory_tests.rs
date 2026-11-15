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

#[cfg(windows)]
mod windows {
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
