//! Recoverable directory replacement shared by repository and installation publication.
use super::{create_dir_all, read, remove_dir_all, write};
use crate::Error;
use std::path::Path;
pub(crate) fn replace_directory(staging_dir: &Path, destination_dir: &Path) -> Result<(), Error> {
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
        std::fs::rename(destination_dir, &backup_path)?;
        Some(backup_path)
    } else {
        None
    };

    if let Err(error) = std::fs::rename(staging_dir, destination_dir) {
        if let Some(backup) = &backup_dir {
            let _ = std::fs::rename(backup, destination_dir);
        }
        let _ = std::fs::remove_dir_all(staging_dir);
        return Err(Error::Io(error));
    }

    if let Some(backup) = backup_dir {
        remove_dir_all(&backup)?;
        std::fs::remove_file(backup.with_extension("owner"))?;
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
        .open(lock_path)?;
    control.checkpoint()?;
    loop {
        control.checkpoint()?;
        match file.try_lock() {
            Ok(()) => break,
            Err(std::fs::TryLockError::WouldBlock) => {
                std::thread::sleep(std::time::Duration::from_millis(25))
            }
            Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
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
        std::fs::remove_dir_all(&backup)?;
    } else {
        std::fs::rename(&backup, destination)?;
    }
    std::fs::remove_file(backup.with_extension("owner"))?;
    Ok(())
}
