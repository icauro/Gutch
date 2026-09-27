//! Recoverable directory replacement shared by repository and installation publication.
use super::{create_dir_all, read, write};
use crate::Error;
use std::path::Path;
pub(crate) fn replace_directory(staging_dir: &Path, destination_dir: &Path) -> Result<(), Error> {
    replace_directory_with_cleanup(staging_dir, destination_dir, |backup| {
        std::fs::remove_dir_all(backup)
    })
}

// Keep the cleanup boundary injectable to exercise failures after the rename commits.
fn replace_directory_with_cleanup(
    staging_dir: &Path,
    destination_dir: &Path,
    remove_backup: impl FnOnce(&Path) -> std::io::Result<()>,
) -> Result<(), Error> {
    let parent = destination_dir
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    create_dir_all(parent)?;

    recover_directory_promotion(destination_dir)?;
    let backup_path = promotion_backup_path(destination_dir)?;
    let backup_dir = if destination_dir.exists() {
        if backup_path.exists() {
            return Err(Error::manifest("unowned promotion backup exists"));
        }
        write(&backup_path.with_extension("owner"), b"GUTCH_PROMOTION_V1")?;
        std::fs::rename(destination_dir, &backup_path).map_err(|error| {
            Error::io_context(
                format!(
                    "moving existing destination to backup {}",
                    backup_path.display()
                ),
                destination_dir,
                error,
            )
        })?;
        Some(backup_path)
    } else {
        None
    };

    if let Err(error) = std::fs::rename(staging_dir, destination_dir) {
        if let Some(backup) = &backup_dir {
            let _ = std::fs::rename(backup, destination_dir);
        }
        let _ = std::fs::remove_dir_all(staging_dir);
        return Err(Error::io_context(
            format!("promoting staging directory {}", staging_dir.display()),
            destination_dir,
            error,
        ));
    }

    if let Some(backup) = backup_dir {
        // Promotion has committed. Keep the owner marker if cleanup fails so
        // the next promotion can retry without reporting this install as failed.
        if remove_backup(&backup).is_ok() {
            let _ = std::fs::remove_file(backup.with_extension("owner"));
        }
    }
    Ok(())
}

/// Held across staging and promotion, after the operation's path checks.
pub(crate) fn lock_directory_promotion(
    destination: &Path,
    control: &crate::progress::GutchOperationControl,
) -> Result<std::fs::File, Error> {
    let backup = promotion_backup_path(destination)?;
    let lock_path = backup.with_extension("lock");
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if std::fs::symlink_metadata(&lock_path).is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        return Err(Error::manifest(
            "promotion lock must not be a symbolic link",
        ));
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|e| Error::io_context("opening promotion lock", &lock_path, e))?;
    control.checkpoint()?;
    loop {
        control.checkpoint()?;
        match file.try_lock() {
            Ok(()) => break,
            Err(std::fs::TryLockError::WouldBlock) => {
                std::thread::sleep(std::time::Duration::from_millis(25))
            }
            Err(std::fs::TryLockError::Error(error)) => {
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

fn promotion_backup_path(destination: &Path) -> Result<std::path::PathBuf, Error> {
    let parent = destination
        .parent()
        .ok_or_else(|| Error::manifest("destination has no parent"))?;
    let name = destination
        .file_name()
        .ok_or_else(|| Error::manifest("destination has no filename"))?;
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
        && read(&path.with_extension("owner")).is_ok_and(|owner| owner == b"GUTCH_PROMOTION_V1")
}

/// A deterministic same-parent backup makes an interrupted promotion recoverable.
/// Existing destination means promotion completed; absence means restore the backup.
pub(crate) fn recover_directory_promotion(destination: &Path) -> Result<(), Error> {
    let backup = promotion_backup_path(destination)?;
    if !backup.exists() {
        return Ok(());
    }
    if read(&backup.with_extension("owner"))? != b"GUTCH_PROMOTION_V1" {
        return Err(Error::manifest("unowned promotion backup exists"));
    }
    if std::fs::symlink_metadata(&backup)?.file_type().is_symlink() {
        return Err(Error::manifest(
            "promotion backup must not be a symbolic link",
        ));
    }
    if destination.exists() {
        std::fs::remove_dir_all(&backup)
            .map_err(|e| Error::io_context("removing recovered promotion backup", &backup, e))?;
    } else {
        std::fs::rename(&backup, destination).map_err(|error| {
            Error::io_context(
                format!("recovering backup {}", backup.display()),
                destination,
                error,
            )
        })?;
    }
    std::fs::remove_file(backup.with_extension("owner"))?;
    Ok(())
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

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

        let result = replace_directory_with_cleanup(&staging, &destination, |backup| {
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

    #[test]
    fn promotion_failure_names_both_paths_and_restores_existing_destination() {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("destination");
        std::fs::create_dir(&destination).unwrap();
        std::fs::write(destination.join("old"), b"original").unwrap();
        let staging = root.path().join("missing-staging");
        let error = replace_directory(&staging, &destination).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("promoting staging directory"));
        assert!(message.contains(&staging.display().to_string()));
        assert!(message.contains(&destination.display().to_string()));
        assert_eq!(std::fs::read(destination.join("old")).unwrap(), b"original");
        assert!(matches!(error, Error::IoContext { .. }));
    }
}
