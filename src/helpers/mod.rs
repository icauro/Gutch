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
    mut response: reqwest::blocking::Response,
    maximum_bytes: u64,
    mut checkpoint: F,
) -> Result<Vec<u8>, Error>
where
    F: FnMut() -> Result<(), Error>,
{
    checkpoint()?;
    if response
        .content_length()
        .is_some_and(|length| length > maximum_bytes)
    {
        return Err(Error::manifest("response exceeded the allowed size"));
    }
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        checkpoint()?;
        let read = std::io::Read::read(&mut response, &mut buffer)?;
        if read == 0 {
            break;
        }
        if bytes.len().saturating_add(read) as u64 > maximum_bytes {
            return Err(Error::manifest("response exceeded the allowed size"));
        }
        bytes.extend_from_slice(&buffer[..read]);
    }
    checkpoint()?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::encode_query_component;

    #[test]
    fn query_component_preserves_unreserved_bytes_and_encodes_delimiters() {
        assert_eq!(
            encode_query_component("bundle-a,bundle b?"),
            "bundle-a%2Cbundle%20b%3F"
        );
    }
}
