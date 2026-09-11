//! Shared helpers for the gutch crate to reduce boilerplate.

use std::io::Write;
use std::path::Path;

use crate::error::Error;

// ---------------------------------------------------------------------------
// IO helpers
// ---------------------------------------------------------------------------

/// Read a file to string.
pub(crate) fn read_to_string(path: &Path) -> Result<String, Error> {
    Ok(std::fs::read_to_string(path)?)
}

/// Read a file to bytes.
pub(crate) fn read(path: &Path) -> Result<Vec<u8>, Error> {
    Ok(std::fs::read(path)?)
}

/// Write bytes to a file through a same-directory temporary file.
pub(crate) fn write(path: &Path, contents: impl AsRef<[u8]>) -> Result<(), Error> {
    let contents = contents.as_ref();
    write_atomic_with(path, |file| {
        file.write_all(contents)?;
        Ok(())
    })
}

/// Write a file through a same-directory temporary file and atomically replace the target.
pub(crate) fn write_atomic_with<F>(path: &Path, write_contents: F) -> Result<(), Error>
where
    F: FnOnce(&mut std::fs::File) -> Result<(), Error>,
{
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    create_dir_all(parent)?;
    // Canonical Windows parents carry the extended-length prefix required by
    // tempfile's native rename; std::fs alone accepts unprefixed long paths.
    let parent = std::fs::canonicalize(parent)?;
    let file_name = path.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "file path has no filename",
        )
    })?;
    let destination = parent.join(file_name);
    let mut temporary_file = tempfile::NamedTempFile::new_in(&parent)?;
    write_contents(temporary_file.as_file_mut())?;
    temporary_file.as_file_mut().flush()?;
    temporary_file.as_file_mut().sync_all()?;
    temporary_file
        .persist(destination)
        .map_err(|error| Error::Io(error.error))?;
    Ok(())
}

/// Replace a directory with a fully prepared same-parent directory.
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
    control: &crate::GutchOperationControl,
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

/// Create directory and all parents.
pub(crate) fn create_dir_all(path: &Path) -> Result<(), Error> {
    Ok(std::fs::create_dir_all(path)?)
}

/// Remove a file.
pub(crate) fn remove_file(path: &Path) -> Result<(), Error> {
    Ok(std::fs::remove_file(path)?)
}

/// Remove a directory and all contents.
pub(crate) fn remove_dir_all(path: &Path) -> Result<(), Error> {
    Ok(std::fs::remove_dir_all(path)?)
}

// ---------------------------------------------------------------------------
// JSON helpers
// ---------------------------------------------------------------------------

/// Serialize to pretty JSON string.
fn to_json_pretty<T: serde::Serialize>(value: &T) -> Result<String, Error> {
    Ok(serde_json::to_string_pretty(value)?)
}

/// Write a serializable value as pretty JSON to a file.
pub(crate) fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<(), Error> {
    let json = to_json_pretty(value)?;
    write(path, json.as_bytes())
}

/// Parse JSON from string.
pub(crate) fn from_json_str<T: serde::de::DeserializeOwned>(json: &str) -> Result<T, Error> {
    Ok(serde_json::from_str(json)?)
}

/// Parse JSON from bytes.
pub(crate) fn from_json_bytes<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, Error> {
    Ok(serde_json::from_slice(bytes)?)
}

#[cfg(test)]
mod tests {
    use super::{replace_directory, write, write_atomic_with};

    #[test]
    fn atomic_write_preserves_existing_file_when_writer_fails() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("artifact");
        write(&path, b"original").unwrap();

        let result = write_atomic_with(&path, |_file| {
            Err(crate::Error::manifest("simulated write failure"))
        });

        assert!(result.is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    #[cfg(windows)]
    fn atomic_write_supports_long_windows_artifact_paths() {
        let directory = tempfile::tempdir().unwrap();
        let parent = directory.path().join("snapshot-cache".repeat(14));
        let path = parent.join("snapshot_product_metadata.json");
        assert!(path.as_os_str().len() > 260);
        write(&path, b"original").unwrap();
        write(&path, b"replacement").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"replacement");
        assert_eq!(std::fs::read_dir(parent).unwrap().count(), 1);
    }

    #[test]
    fn atomic_write_replaces_existing_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("artifact");
        write(&path, b"original").unwrap();

        write(&path, b"replacement").unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"replacement");
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn directory_replacement_promotes_staged_contents_and_removes_backup() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("destination");
        let staging = directory.path().join("staging");
        std::fs::create_dir_all(&destination).unwrap();
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(destination.join("old"), b"old").unwrap();
        std::fs::write(staging.join("new"), b"new").unwrap();

        replace_directory(&staging, &destination).unwrap();

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
}
