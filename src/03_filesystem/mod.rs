//! Atomic filesystem operations and path safety.
pub(crate) mod constraints;
pub(crate) mod directory;
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
    fs::read_to_string(path).map_err(|error| Error::io_context("reading file", path, error))
}

/// Read a file to bytes.
pub(crate) fn read(path: &Path) -> Result<Vec<u8>, Error> {
    fs::read(path).map_err(|error| Error::io_context("reading file", path, error))
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
    let parent = fs::canonicalize(parent)
        .map_err(|error| Error::io_context("resolving directory", parent, error))?;
    let file_name = path.file_name().ok_or_else(|| {
        Error::io_context(
            "writing file",
            path,
            std::io::Error::new(ErrorKind::InvalidInput, "file path has no filename"),
        )
    })?;
    let destination = parent.join(file_name);
    let write_error = |error| Error::io_context("writing file", &destination, error);
    let mut temporary_file = tempfile::NamedTempFile::new_in(&parent)
        .map_err(|error| Error::io_context("creating temporary file", &parent, error))?;
    write_contents(temporary_file.as_file_mut())
        .map_err(|error| error.with_io_context("writing file", &destination))?;
    temporary_file.as_file_mut().flush().map_err(write_error)?;
    temporary_file
        .as_file_mut()
        .sync_all()
        .map_err(write_error)?;
    temporary_file
        .persist(&destination)
        .map_err(|error| Error::io_context("replacing file", &destination, error.error))?;
    // The rename is durable only once the directory entry reaches disk.
    #[cfg(unix)]
    File::open(&parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| Error::io_context("syncing directory", &parent, error))?;
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
    fs::remove_file(path).map_err(|error| Error::io_context("removing file", path, error))
}

/// Remove a directory and all contents.
pub(crate) fn remove_dir_all(path: &Path) -> Result<(), Error> {
    fs::remove_dir_all(path)
        .map_err(|error| Error::io_context("removing directory tree", path, error))
}

// ---------------------------------------------------------------------------
// JSON filesystem
// ---------------------------------------------------------------------------

/// Write a serializable value as pretty JSON to a file.
pub(crate) fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<(), Error> {
    write(path, serde_json::to_string_pretty(value)?)
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
}
