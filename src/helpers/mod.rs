//! Shared helpers for the gutch crate to reduce boilerplate.

use std::io::Write;
use std::path::Path;

use crate::error::Error;

// ---------------------------------------------------------------------------
// IO helpers
// ---------------------------------------------------------------------------

/// Read a file to string.
pub fn read_to_string(path: &Path) -> Result<String, Error> {
    Ok(std::fs::read_to_string(path)?)
}

/// Read a file to bytes.
pub fn read(path: &Path) -> Result<Vec<u8>, Error> {
    Ok(std::fs::read(path)?)
}

/// Write bytes to a file through a same-directory temporary file.
pub fn write(path: &Path, contents: impl AsRef<[u8]>) -> Result<(), Error> {
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

    let mut temporary_file = tempfile::NamedTempFile::new_in(parent)?;
    write_contents(temporary_file.as_file_mut())?;
    temporary_file.as_file_mut().flush()?;
    temporary_file.as_file_mut().sync_all()?;
    temporary_file
        .persist(path)
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

    let backup_dir = if destination_dir.exists() {
        let backup = tempfile::Builder::new()
            .prefix(".gutch-backup-")
            .tempdir_in(parent)?
            .keep();
        std::fs::remove_dir(&backup)?;
        std::fs::rename(destination_dir, &backup)?;
        Some(backup)
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
    }
    Ok(())
}

/// Create directory and all parents.
pub fn create_dir_all(path: &Path) -> Result<(), Error> {
    Ok(std::fs::create_dir_all(path)?)
}

/// Remove a file.
pub fn remove_file(path: &Path) -> Result<(), Error> {
    Ok(std::fs::remove_file(path)?)
}

/// Remove a directory and all contents.
pub fn remove_dir_all(path: &Path) -> Result<(), Error> {
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
pub fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<(), Error> {
    let json = to_json_pretty(value)?;
    write(path, json.as_bytes())
}

/// Parse JSON from string.
pub fn from_json_str<T: serde::de::DeserializeOwned>(json: &str) -> Result<T, Error> {
    Ok(serde_json::from_str(json)?)
}

/// Parse JSON from bytes.
pub fn from_json_bytes<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, Error> {
    Ok(serde_json::from_slice(bytes)?)
}

/// Percent-encode one URL query component using the unreserved URI characters.
pub fn encode_query_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push('%');
            encoded.push(char::from(b"0123456789ABCDEF"[(byte >> 4) as usize]));
            encoded.push(char::from(b"0123456789ABCDEF"[(byte & 0x0F) as usize]));
        }
    }
    encoded
}

/// Read a blocking HTTP response with a hard byte limit and cancellation hook.
pub fn read_response_bounded<F>(
    response: reqwest::blocking::Response,
    maximum_bytes: u64,
    checkpoint: F,
) -> Result<Vec<u8>, Error>
where
    F: FnMut() -> Result<(), Error>,
{
    if response
        .content_length()
        .is_some_and(|length| length > maximum_bytes)
    {
        return Err(Error::manifest("response exceeded the allowed size"));
    }
    let mut bytes = Vec::new();
    crate::transport::read_bounded_chunks(response, Some(maximum_bytes), checkpoint, |chunk| {
        bytes.extend_from_slice(chunk);
        Ok(())
    })?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::{encode_query_component, replace_directory, write, write_atomic_with};

    #[test]
    fn query_component_preserves_unreserved_bytes_and_encodes_delimiters() {
        assert_eq!(
            encode_query_component("bundle-a,bundle b?"),
            "bundle-a%2Cbundle%20b%3F"
        );
    }

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
