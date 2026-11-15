//! File reads, atomic writes and JSON helpers that report engine errors.

use crate::error::Error;
use crate::error::IoContext;
use std::fs;
use std::fs::File;
use std::fs::Permissions;
use std::io::ErrorKind;
use std::io::Write;
use std::path::Path;

/// Read a file to string.
pub(crate) fn read_to_string(path: &Path) -> Result<String, Error> {
    fs::read_to_string(path).io_context("reading file", path)
}

/// Metadata of the entry at `path` itself, or `None` when nothing is there.
pub(crate) fn symlink_metadata_if_exists(path: &Path) -> std::io::Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// True when any execute bit is set; always false where execute bits do not exist.
pub(crate) fn is_executable(metadata: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        false
    }
}

/// Read a file to bytes.
pub(crate) fn read(path: &Path) -> Result<Vec<u8>, Error> {
    fs::read(path).io_context("reading file", path)
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
    let parent = fs::canonicalize(parent).io_context("resolving directory", parent)?;
    let file_name = path.file_name().ok_or_else(|| {
        Error::io_context(
            "writing file",
            path,
            std::io::Error::new(ErrorKind::InvalidInput, "file path has no filename"),
        )
    })?;
    let destination = parent.join(file_name);
    let write_error = |error| Error::io_context("writing file", &destination, error);
    let mut temporary_file =
        tempfile::NamedTempFile::new_in(&parent).io_context("creating temporary file", &parent)?;
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
        .io_context("syncing directory", &parent)?;
    Ok(())
}

/// Create a directory and all missing ancestors.
pub(crate) fn create_dir_all(path: &Path) -> Result<(), Error> {
    fs::create_dir_all(path).io_context("creating directory tree", path)
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
    fs::remove_file(path).io_context("removing file", path)
}

/// Remove a directory and all contents.
pub(crate) fn remove_dir_all(path: &Path) -> Result<(), Error> {
    fs::remove_dir_all(path).io_context("removing directory tree", path)
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
#[path = "files_tests.rs"]
mod tests;
