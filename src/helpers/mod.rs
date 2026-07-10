//! Shared helpers for the gutch crate to reduce boilerplate.

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

/// Write bytes to a file.
pub fn write(path: &Path, contents: impl AsRef<[u8]>) -> Result<(), Error> {
    Ok(std::fs::write(path, contents)?)
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
