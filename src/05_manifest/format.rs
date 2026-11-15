//! Format versions and file names of stored documents, and their schema-marker check.

pub const PUBLIC_METADATA_FORMAT_VERSION: &str = "1.0.0";
pub const CONTENT_REPO_FORMAT: &str = "gutch_repo";
pub const REPO_METADATA_FILE: &str = "repo_metadata.json";

/// Read a schema marker and reject any value other than `expected`, so an
/// unsupported document never loads as if it were current.
pub(super) fn deserialize_schema_marker<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
    field: &str,
    expected: &str,
) -> Result<String, D::Error> {
    let value = <String as serde::Deserialize>::deserialize(deserializer)?;
    if value != expected {
        return Err(serde::de::Error::custom(format!(
            "unsupported {field} '{value}'; expected '{expected}'"
        )));
    }
    Ok(value)
}

pub(crate) fn deserialize_document_version<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<String, D::Error> {
    deserialize_schema_marker(
        deserializer,
        "document version",
        PUBLIC_METADATA_FORMAT_VERSION,
    )
}
