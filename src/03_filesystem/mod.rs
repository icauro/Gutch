//! Atomic filesystem operations and path safety.
pub(crate) mod constraints;
pub(crate) mod paths;

use std::io::Write;
use std::path::Path;

use crate::error::Error;
use std::fs;
use std::fs::File;
use std::fs::Permissions;
use std::io::ErrorKind;

// ---------------------------------------------------------------------------
// IO filesystem
// ---------------------------------------------------------------------------

/// Read a file to string.
pub(crate) fn read_to_string(path: &Path) -> Result<String, Error> {
    Ok(fs::read_to_string(path)?)
}

/// Read a file to bytes.
pub(crate) fn read(path: &Path) -> Result<Vec<u8>, Error> {
    Ok(fs::read(path)?)
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
    F: FnOnce(&mut File) -> Result<(), Error>,
{
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    create_dir_all(parent)?;
    // Canonical Windows parents carry the extended-length prefix required by
    // tempfile's native rename; std::fs alone accepts unprefixed long paths.
    let parent = fs::canonicalize(parent)?;
    let file_name = path
        .file_name()
        .ok_or_else(|| std::io::Error::new(ErrorKind::InvalidInput, "file path has no filename"))?;
    let destination = parent.join(file_name);
    let mut temporary_file = tempfile::NamedTempFile::new_in(&parent)?;
    write_contents(temporary_file.as_file_mut())?;
    temporary_file.as_file_mut().flush()?;
    temporary_file.as_file_mut().sync_all()?;
    temporary_file
        .persist(destination)
        .map_err(|error| Error::Io(error.error))?;
    // The rename is durable only once the directory entry reaches disk.
    #[cfg(unix)]
    File::open(&parent)?.sync_all()?;
    Ok(())
}

/// Create a directory and all missing ancestors.
pub(crate) fn create_dir_all(path: &Path) -> Result<(), Error> {
    fs::create_dir_all(path).map_err(|e| Error::io_context("creating directory tree", path, e))
}

/// Set or clear the read-only flag. On Unix, clearing restores only the owner
/// write bit; `Permissions::set_readonly(false)` would make the entry world-writable.
pub(crate) fn set_read_only(permissions: &mut Permissions, read_only: bool) {
    #[cfg(unix)]
    if !read_only {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(permissions.mode() | 0o200);
        return;
    }
    permissions.set_readonly(read_only);
}

/// Remove a file.
pub(crate) fn remove_file(path: &Path) -> Result<(), Error> {
    Ok(fs::remove_file(path)?)
}

/// Remove a directory and all contents.
pub(crate) fn remove_dir_all(path: &Path) -> Result<(), Error> {
    Ok(fs::remove_dir_all(path)?)
}

// ---------------------------------------------------------------------------
// JSON filesystem
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
    use super::write;
    use super::write_atomic_with;
    use crate::filesystem::directory::replace_directory;

    #[test]
    fn atomic_write_preserves_existing_file_when_writer_fails() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("artifact");
        write(&path, b"original").unwrap();

        let result = write_atomic_with(&path, |_file| {
            Err(crate::Error::repository("simulated write failure"))
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

        replace_directory(
            &staging,
            &destination,
            &crate::progress::OperationControl::default(),
        )
        .unwrap();

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

pub(crate) mod directory;
